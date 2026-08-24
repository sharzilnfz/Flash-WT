//! The canonical snapshot manifest: entries, validation, TSV codec,
//! and the content hash that names a published snapshot.
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

use sha2::{Digest, Sha256};

use crate::mirror::unescape;
use crate::ContentId;

/// Normalized mode for a regular file with any execute bit set.
pub(super) const EXEC_FILE_MODE: u32 = 0o755;
/// Normalized mode for a plain regular file.
pub(super) const PLAIN_FILE_MODE: u32 = 0o644;
/// Normalized mode for directories.
pub(super) const DIR_MODE: u32 = 0o755;
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
    #[must_use]
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
        let end = text
            .rfind('\n')
            .ok_or("manifest has no complete header line")?;
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

pub(super) fn serialize_entries(entries: &[SnapshotEntry]) -> String {
    let mut out = String::new();
    for e in entries {
        out.push_str("entry\t");
        out.push_str(&crate::mirror::escape(&e.rel));
        out.push('\t');
        out.push_str(e.kind.as_str());
        out.push('\t');
        out.push_str(&format!("{:o}", e.mode));
        out.push('\t');
        match (e.blob, &e.target) {
            (Some(blob), _) => out.push_str(&blob.to_string()),
            (None, Some(target)) => out.push_str(&crate::mirror::escape(target)),
            (None, None) => out.push('-'),
        }
        out.push('\n');
    }
    out
}
