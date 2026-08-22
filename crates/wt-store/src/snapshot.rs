//! Whole-directory snapshots (fast-hydration ticket 08, Phase 2 of
//! AGENT_HANDOFF_PLAN_REVISED.md).
//!
//! One snapshot per heavy directory lives under
//! `<root>/snapshots/<64-hex-manifest-hash>/`:
//!
//! ```text
//! manifest.tsv        canonical manifest (schema below)
//! .complete           schema version + manifest hash
//! tree/               the hydrated tree: regular files hardlinked
//!                     to object blobs
//! ```
//!
//! The tree sits under `tree/` rather than beside the metadata so a
//! single recursive `clonefile(2)` of `tree/` produces EXACTLY the
//! heavy directory — no metadata files leaking into the worktree
//! (layout deviation from the handoff sketch, recorded in ADR-0005).
//!
//! Snapshots are rebuildable caches, not GC roots: a snapshot survives
//! only while some live store mirror names it (ADR-0004). The tree's
//! files share inodes with object blobs until the whole tree is cloned
//! out with one APFS `clonefile(2)`, which hands the worktree fresh
//! private writable inodes.
//!
//! Canonical manifest format (`<TAB>` marks a literal tab byte):
//!
//! ```text
//! v1<TAB>manifest-sha256<TAB><64-hex-hash>
//! entry<TAB><escaped-relpath><TAB><kind><TAB><octal-mode><TAB><escaped-ref>
//! ```
//!
//! - `kind` is exactly `file`, `symlink`, or `dir`.
//! - `file` ref is the 64-hex blob id; `symlink` ref is the escaped
//!   symlink target (targets are recorded, never stored as blobs);
//!   `dir` ref is literally `-`.
//! - Relpaths use forward slashes, no empty component, no `.` or
//!   `..`, never absolute. Escaping is [`crate::mirror`]'s percent
//!   escaping, reused verbatim.
//! - Entries are sorted by raw canonical path bytes, then kind, so
//!   the order is total and platform-independent.
//! - The manifest hash is SHA-256 of the exact serialized ENTRY bytes,
//!   excluding the header line. Empty directories appear explicitly;
//!   an empty heavy directory is a manifest with zero entries.
//!
//! Modes are normalized at construction: files carry 0o755 when any
//! execute bit was set on disk, else 0o644; dirs are 0o755; symlinks
//! 0o777. Normalization keeps the hash stable across umasks while
//! preserving executable-ness, and the build applies these modes to
//! the tree it creates — so a clone of the snapshot inherits them.
//!
//! Integrity model: verification happens once, at publish. Every file
//! blob is proven (verified-ledger trust, or full read-and-hash under
//! `WT_VERIFY=1`) before it is linked into the new snapshot; after the
//! atomic rename the snapshot is trusted like any published blob. A
//! hit performs zero blob reads by design; callers wanting continuous
//! corruption detection set `paranoid`, which bypasses hits entirely
//! and rebuilds from freshly hashed blobs.

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::mirror::{escape, unescape};
use crate::{ContentId, DiskStore, Error, Store};

/// Normalized mode for a regular file with any execute bit set.
const EXEC_FILE_MODE: u32 = 0o755;
/// Normalized mode for a plain regular file.
const PLAIN_FILE_MODE: u32 = 0o644;
/// Normalized mode for directories.
const DIR_MODE: u32 = 0o755;
/// Conventional lstat mode for symlinks.
const SYMLINK_MODE: u32 = 0o777;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EntryKind {
    File,
    Symlink,
    Dir,
}

impl EntryKind {
    fn as_str(self) -> &'static str {
        match self {
            EntryKind::File => "file",
            EntryKind::Symlink => "symlink",
            EntryKind::Dir => "dir",
        }
    }

    fn parse(text: &str) -> Option<EntryKind> {
        match text {
            "file" => Some(EntryKind::File),
            "symlink" => Some(EntryKind::Symlink),
            "dir" => Some(EntryKind::Dir),
            _ => None,
        }
    }
}

/// One canonical manifest entry. `blob` is set for files, `target`
/// for symlinks; dirs carry neither. `mode` is the normalized octal
/// mode (see module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotEntry {
    pub rel: String,
    pub kind: EntryKind,
    pub mode: u32,
    pub blob: Option<ContentId>,
    pub target: Option<String>,
}

impl SnapshotEntry {
    /// A regular-file entry; `raw_mode` decides executable-ness only.
    pub fn file(rel: impl Into<String>, blob: ContentId, raw_mode: u32) -> SnapshotEntry {
        SnapshotEntry {
            rel: rel.into(),
            kind: EntryKind::File,
            mode: normalized_file_mode(raw_mode),
            blob: Some(blob),
            target: None,
        }
    }

    /// An explicit directory entry (empty dirs must not vanish).
    pub fn dir(rel: impl Into<String>) -> SnapshotEntry {
        SnapshotEntry {
            rel: rel.into(),
            kind: EntryKind::Dir,
            mode: DIR_MODE,
            blob: None,
            target: None,
        }
    }

    /// A symlink entry; the target is recorded, never dereferenced.
    pub fn symlink(rel: impl Into<String>, target: impl Into<String>) -> SnapshotEntry {
        SnapshotEntry {
            rel: rel.into(),
            kind: EntryKind::Symlink,
            mode: SYMLINK_MODE,
            blob: None,
            target: Some(target.into()),
        }
    }
}

fn normalized_file_mode(raw_mode: u32) -> u32 {
    if raw_mode & 0o111 != 0 {
        EXEC_FILE_MODE
    } else {
        PLAIN_FILE_MODE
    }
}

/// Reject anything that cannot be a canonical relpath before it can
/// reach placement or serialization.
fn validate_rel(rel: &str) -> Result<(), String> {
    if rel.is_empty() {
        return Err("relpath is empty".into());
    }
    if rel.starts_with('/') {
        return Err(format!("relpath {rel:?} is absolute"));
    }
    for comp in rel.split('/') {
        match comp {
            "" => return Err(format!("relpath {rel:?} has an empty component")),
            "." | ".." => return Err(format!("relpath {rel:?} contains {comp:?}")),
            _ => {}
        }
    }
    Ok(())
}

/// A validated, canonically ordered manifest with its content hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub entries: Vec<SnapshotEntry>,
    /// SHA-256 of the exact serialized entry bytes (no header).
    pub hash: ContentId,
}

impl Manifest {
    /// Build a manifest from arbitrary-order inputs: validate every
    /// relpath, normalize modes, sort by raw path bytes then kind,
    /// and hash the canonical serialization.
    pub fn new(mut entries: Vec<SnapshotEntry>) -> Result<Manifest, String> {
        for e in &entries {
            validate_rel(&e.rel)?;
            match e.kind {
                EntryKind::File => {
                    if e.blob.is_none() || e.target.is_some() {
                        return Err(format!("file entry {} lacks a blob ref", e.rel));
                    }
                }
                EntryKind::Symlink => {
                    if e.target.is_none() || e.blob.is_some() {
                        return Err(format!("symlink entry {} lacks a target", e.rel));
                    }
                }
                EntryKind::Dir => {
                    if e.blob.is_some() || e.target.is_some() {
                        return Err(format!("dir entry {} carries a ref", e.rel));
                    }
                }
            }
        }
        // Sort by raw canonical path bytes, then kind, for a total
        // order independent of locale or platform.
        entries.sort_by(|a, b| {
            a.rel
                .as_bytes()
                .cmp(b.rel.as_bytes())
                .then_with(|| a.kind.cmp(&b.kind))
        });
        // Duplicates would make two entries claim one path.
        if let Some(pair) = entries.windows(2).find(|w| w[0].rel == w[1].rel) {
            return Err(format!("duplicate relpath {:?}", pair[0].rel));
        }
        let body = serialize_entries(&entries);
        let mut hasher = Sha256::new();
        hasher.update(body.as_bytes());
        Ok(Manifest {
            entries,
            hash: ContentId(hasher.finalize().into()),
        })
    }

    /// Header + entry lines. The header embeds this manifest's hash.
    pub fn serialize(&self) -> String {
        let mut out = String::new();
        out.push_str("v1\tmanifest-sha256\t");
        out.push_str(&self.hash.to_string());
        out.push('\n');
        out.push_str(&serialize_entries(&self.entries));
        out
    }

    /// Parse manifest text and verify its header hash against the
    /// exact bytes that follow it. A mismatch means the manifest was
    /// edited or corrupted underneath us: invalid, never half-trusted.
    pub fn parse(text: &str) -> Result<Manifest, String> {
        // Same torn-line tolerance as mirrors: whatever follows the
        // last newline never got its terminal newline and is not a
        // record. Everything UP TO AND INCLUDING that newline is the
        // manifest — and exactly those entry bytes are what got
        // hashed at construction.
        let end = text.rfind('\n').ok_or("manifest has no complete header line")?;
        let complete = &text[..=end];
        let (header, body) = complete
            .split_once('\n')
            .ok_or("manifest has no header line")?;
        let fields: Vec<&str> = header.split('\t').collect();
        if fields.len() != 3 || fields[0] != "v1" || fields[1] != "manifest-sha256" {
            return Err(format!("bad manifest header {header:?}"));
        }
        let claimed = ContentId::from_hex(fields[2])
            .ok_or_else(|| format!("malformed manifest hash {:?}", fields[2]))?;
        let mut hasher = Sha256::new();
        hasher.update(body.as_bytes());
        let actual = ContentId(hasher.finalize().into());
        if claimed != actual {
            return Err(format!(
                "manifest hash mismatch: header says {claimed}, body hashes to {actual}"
            ));
        }
        let mut entries = Vec::new();
        // An empty body is the valid zero-entry manifest (an empty
        // heavy directory); anything else parses line by line.
        let entry_text = body.trim_end_matches('\n');
        for line in entry_text.split('\n').filter(|l| !l.is_empty()) {
            let f: Vec<&str> = line.split('\t').collect();
            match f.as_slice() {
                ["entry", rel_esc, kind, mode, ref_esc] => {
                    let kind =
                        EntryKind::parse(kind).ok_or_else(|| format!("unknown kind {kind:?}"))?;
                    let mode_text = *mode;
                    let mode = u32::from_str_radix(mode_text, 8)
                        .map_err(|_| format!("malformed octal mode {mode_text:?}"))?;
                    let rel = unescape(rel_esc)?;
                    validate_rel(&rel)?;
                    entries.push(match kind {
                        EntryKind::File => {
                            let hex = unescape(ref_esc)?;
                            let blob = ContentId::from_hex(&hex)
                                .ok_or_else(|| format!("file entry has non-hex ref {hex:?}"))?;
                            SnapshotEntry {
                                rel,
                                kind,
                                mode,
                                blob: Some(blob),
                                target: None,
                            }
                        }
                        EntryKind::Symlink => SnapshotEntry {
                            rel,
                            kind,
                            mode,
                            blob: None,
                            target: Some(unescape(ref_esc)?),
                        },
                        EntryKind::Dir => {
                            if *ref_esc != "-" {
                                return Err(format!("dir entry {rel:?} has a non-dash ref"));
                            }
                            SnapshotEntry {
                                rel,
                                kind,
                                mode,
                                blob: None,
                                target: None,
                            }
                        }
                    });
                }
                _ => return Err(format!("malformed manifest entry line {line:?}")),
            }
        }
        // Re-sort so the in-memory form is canonical regardless of
        // what byte order the file carried, and reject duplicates.
        entries.sort_by(|a, b| {
            a.rel
                .as_bytes()
                .cmp(b.rel.as_bytes())
                .then_with(|| a.kind.cmp(&b.kind))
        });
        if let Some(pair) = entries.windows(2).find(|w| w[0].rel == w[1].rel) {
            return Err(format!("duplicate relpath {:?}", pair[0].rel));
        }
        Ok(Manifest {
            entries,
            hash: claimed,
        })
    }
}

fn serialize_entries(entries: &[SnapshotEntry]) -> String {
    let mut out = String::new();
    for e in entries {
        out.push_str("entry\t");
        out.push_str(&escape(&e.rel));
        out.push('\t');
        out.push_str(e.kind.as_str());
        out.push('\t');
        out.push_str(&format!("{:o}", e.mode));
        out.push('\t');
        match (e.blob, &e.target) {
            (Some(blob), _) => out.push_str(&blob.to_string()),
            (None, Some(target)) => out.push_str(&escape(target)),
            (None, None) => out.push('-'),
        }
        out.push('\n');
    }
    out
}

/// Final directory of one published snapshot:
/// `<root>/snapshots/<hash>/`.
pub fn snapshot_path(root: &Path, hash: &ContentId) -> PathBuf {
    root.join("snapshots").join(hash.to_string())
}

/// The clonable hydrated tree inside a published snapshot:
/// `<root>/snapshots/<hash>/tree/`. This — not the snapshot root,
/// which also carries `manifest.tsv` and `.complete` — is what gets
/// cloned into a worktree.
pub fn snapshot_tree_path(root: &Path, hash: &ContentId) -> PathBuf {
    snapshot_path(root, hash).join("tree")
}

/// Where the tree lives inside a build temp directory.
const TREE_SUBDIR: &str = "tree";

/// Load and fully validate the published snapshot for `hash`, if one
/// exists. Valid means: parseable manifest whose header hash matches
/// the directory name, plus a `.complete` marker carrying the same
/// schema version and hash. Anything else is debris (GC collects it
/// after the grace period); callers treat `None` as a miss.
///
/// This is THE shared validity check — snapshot lookup, the
/// concurrent-publish loser, and mark-and-sweep all go through it.
pub fn read_published(root: &Path, hash: &ContentId) -> Option<Manifest> {
    let dir = snapshot_path(root, hash);
    let text = fs::read_to_string(dir.join("manifest.tsv")).ok()?;
    let manifest = Manifest::parse(&text).ok()?;
    if manifest.hash != *hash {
        return None;
    }
    let complete = fs::read_to_string(dir.join(".complete")).ok()?;
    let mut parts = complete.trim_end_matches('\n').split('\t');
    if parts.next()? != "v1" {
        return None;
    }
    if parts.next()? != hash.to_string() || parts.next().is_some() {
        return None;
    }
    Some(manifest)
}

/// What [`DiskStore::publish_snapshot`] did with its temp tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishOutcome {
    /// Our temp tree won the rename; we own the published snapshot.
    Published,
    /// Another writer's valid snapshot was already there; our temp
    /// was discarded and the winner should be used.
    WinnerValid,
    /// Another writer's directory is there but NOT valid. We left it
    /// alone (never overwrite debris we cannot prove ours): treat
    /// this as a miss and fall back.
    WinnerInvalid,
}

/// Why [`DiskStore::publish_snapshot`] failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildError {
    /// The blob vanished between ingest and link — a sweep raced us.
    /// Re-put the source content, re-verify, and retry ONCE.
    MissingBlob(ContentId),
    /// Anything fatal.
    Fatal(String),
}

impl From<String> for BuildError {
    fn from(e: String) -> Self {
        BuildError::Fatal(e)
    }
}

impl DiskStore {
    /// Directory of the published snapshot for `hash`.
    pub fn snapshot_path(&self, hash: &ContentId) -> PathBuf {
        snapshot_path(self.root(), hash)
    }

    /// Shared validity check for this store's snapshots. See
    /// [`read_published`].
    pub fn find_snapshot(&self, hash: &ContentId) -> Option<Manifest> {
        read_published(self.root(), hash)
    }

    /// Build the snapshot tree for `entries` in
    /// `<root>/snapshots/tmp/<uuid>` and atomically rename it into
    /// place. Every file blob is verified BEFORE linking, per policy:
    /// full read-and-hash when `paranoid`, verified-ledger trust
    /// otherwise ([`DiskStore::ensure_verified`]).
    ///
    /// Concurrent publish: the rename is the single atomic act. If it
    /// loses (EEXIST/ENOTEMPTY), the winner is validated — valid means
    /// discard our temp and use theirs; invalid debris stays untouched
    /// and the caller treats this as a miss. See [`PublishOutcome`].
    ///
    /// Returns [`BuildError::MissingBlob`] when a blob disappeared
    /// mid-build (sweep race): the CALLER re-puts the source content
    /// and retries once — this method borrows immutably precisely so
    /// the retry can mutate the store in between.
    pub fn publish_snapshot(
        &self,
        entries: Vec<SnapshotEntry>,
        paranoid: bool,
    ) -> std::result::Result<std::result::Result<PublishOutcome, BuildError>, Error> {
        let manifest =
            Manifest::new(entries).map_err(|e| Error::Io(io::Error::other(e.to_string())))?;

        let tmp_base = self.root().join("snapshots").join("tmp");
        fs::create_dir_all(&tmp_base)?;
        let tmp = tempfile::Builder::new()
            .prefix("build-")
            .tempdir_in(&tmp_base)?;
        let tmp_path = tmp.path().to_path_buf();
        // The clonable tree lives under tree/ so metadata files never
        // leak into a cloned worktree.
        fs::create_dir_all(tmp_path.join(TREE_SUBDIR))?;

        match self.build_tree(&tmp_path.join(TREE_SUBDIR), &manifest, paranoid) {
            Ok(()) => {}
            Err(e) => {
                // Drop removes our partial temp tree; only ever cache
                // debris, never a published name.
                drop(tmp);
                return Ok(Err(e));
            }
        }

        fs::write(tmp_path.join("manifest.tsv"), manifest.serialize())?;
        fs::write(
            tmp_path.join(".complete"),
            format!("v1\t{}\n", manifest.hash),
        )?;

        let final_path = self.snapshot_path(&manifest.hash);
        match fs::rename(&tmp_path, &final_path) {
            Ok(()) => {
                // Dropping the TempDir handle now tries to remove the
                // OLD temp path, which no longer exists: a harmless
                // no-op that leaves the published tree untouched.
                drop(tmp);
                Ok(Ok(PublishOutcome::Published))
            }
            Err(e) if matches!(e.raw_os_error(), Some(libc::EEXIST) | Some(libc::ENOTEMPTY)) => {
                // Drop removes our loser temp tree.
                drop(tmp);
                if read_published(self.root(), &manifest.hash).is_some() {
                    Ok(Ok(PublishOutcome::WinnerValid))
                } else {
                    Ok(Ok(PublishOutcome::WinnerInvalid))
                }
            }
            Err(e) => {
                drop(tmp);
                Err(Error::Io(e))
            }
        }
    }

    /// Materialize the manifest inside `dir`: dirs first (sorted
    /// order guarantees parents precede descendants), then hardlinked
    /// files and symlinks, each carrying its normalized mode.
    ///
    /// Chmod on a hardlink retargets the SHARED inode, so the object
    /// blob's mode becomes the snapshot's normalized mode. That is
    /// deliberate (the plan's "blobs' stored modes preserve exec
    /// bits") and safe here: normalization only ever sets owner-write
    /// or adds/removes the x-bits consistently for one content id, and
    /// chmod does not touch mtime, so verified-ledger fingerprints
    /// stay valid.
    fn build_tree(&self, dir: &Path, manifest: &Manifest, paranoid: bool) -> Result<(), BuildError> {
        for entry in &manifest.entries {
            let dest = dir.join(&entry.rel);
            match entry.kind {
                EntryKind::Dir => fs::create_dir_all(&dest)
                    .and_then(|()| fs::set_permissions(&dest, fs::Permissions::from_mode(entry.mode)))
                    .map_err(|e| BuildError::Fatal(format!("cannot create {}: {e}", dest.display())))?,
                EntryKind::Symlink => {
                    let target = entry.target.as_deref().expect("validated symlink entry");
                    if let Some(parent) = dest.parent() {
                        if !parent.exists() {
                            fs::create_dir_all(parent).map_err(|e| {
                                BuildError::Fatal(format!(
                                    "cannot create {}: {e}",
                                    parent.display()
                                ))
                            })?;
                        }
                    }
                    #[cfg(unix)]
                    std::os::unix::fs::symlink(target, &dest).map_err(|e| {
                        BuildError::Fatal(format!("cannot link {}: {e}", dest.display()))
                    })?;
                }
                EntryKind::File => {
                    let blob = entry.blob.expect("validated file entry");
                    // Sorted order puts explicit dir entries ahead of
                    // their children, but a manifest is not obliged
                    // to name every intermediate: recreate any gap.
                    if let Some(parent) = dest.parent() {
                        if !parent.exists() {
                            fs::create_dir_all(parent).map_err(|e| {
                                BuildError::Fatal(format!(
                                    "cannot create {}: {e}",
                                    parent.display()
                                ))
                            })?;
                        }
                    }
                    // Verify first: a corrupt or missing blob never
                    // reaches placement.
                    if paranoid {
                        if let Err(e) = Store::get(self, &blob) {
                            return Err(match e {
                                Error::UnknownContent(_) => BuildError::MissingBlob(blob),
                                other => BuildError::Fatal(other.to_string()),
                            });
                        }
                    } else if let Err(e) = self.ensure_verified(&blob) {
                        return Err(match e {
                            Error::UnknownContent(_) => BuildError::MissingBlob(blob),
                            other => BuildError::Fatal(other.to_string()),
                        });
                    }
                    if let Err(e) = fs::hard_link(self.blob_path(&blob), &dest) {
                        return Err(match e.kind() {
                            io::ErrorKind::NotFound => BuildError::MissingBlob(blob),
                            _ => BuildError::Fatal(format!(
                                "cannot link blob {blob} to {}: {e}",
                                dest.display()
                            )),
                        });
                    }
                    fs::set_permissions(&dest, fs::Permissions::from_mode(entry.mode))
                        .map_err(|e| {
                            BuildError::Fatal(format!("cannot chmod {}: {e}", dest.display()))
                        })?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
