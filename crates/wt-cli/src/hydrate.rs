//! Store-backed hydration (ticket 05).
//!
//! Heavy directories no longer copy straight from the source checkout.
//! They are ingested into the content-addressed store (every unique
//! file once, addressed by hash), then materialized into the new
//! worktree from the store. A second worktree of the same project
//! therefore adds no new store content — the whole point of ADR-0001.
//!
//! References: every blob a worktree materializes gets one `add_ref`,
//! attributed to that worktree through a sidecar manifest written into
//! the worktree's git dir (`wt-hydrated.tsv`). Ticket 06's garbage
//! collection releases those references when the worktree dies.
//!
//! Integrity: materialize proves each blob before anything is
//! placed, so corrupt store content fails loudly instead of landing
//! bad bytes in a fresh tree (spec: silent corruption is detectable).
//! Fast-hydration ticket 05 makes that proof cost once per blob
//! rather than once per run: `ensure_verified` re-hashes only blobs
//! whose (size, mtime) fingerprint is not already recorded in the
//! verified ledger beside the store. `WT_VERIFY=1` restores the old
//! full-hash-every-blob-every-run paranoia.
//!
//! Materialization strategy (fast-hydration ticket 03): the default
//! places every blob as a per-file copy-on-write clone (`fclonefileat`
//! on macOS) — a fresh private inode sharing the blob's physical
//! blocks until first write, with normal writable permissions.
//! Filesystems that refuse the clone fall back silently to byte
//! copies; Linux gets byte copies until reflink is validated there.
//! Hardlinked materialization survives behind `WT_HARDLINK=1` as an
//! experimental mode (see below), and `WT_NO_HARDLINK=1` still forces
//! plain byte copies.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use wt_copy::{CloneOut, FileMaterialize, HardlinkOut, placement_refused};
use wt_store::{ContentId, DiskStore, Entry as CacheEntry, GcMode, Store, ValidationCache};

/// Where the per-machine store lives. `$WT_STORE` wins (tests use it
/// for isolation); otherwise XDG cache conventions.
fn store_dir() -> Result<PathBuf, String> {
    if let Some(dir) = std::env::var_os("WT_STORE") {
        return Ok(PathBuf::from(dir));
    }
    let base = match std::env::var_os("XDG_CACHE_HOME") {
        Some(xdg) => PathBuf::from(xdg),
        None => {
            let home =
                std::env::var_os("HOME").ok_or("cannot locate a home directory for the store")?;
            PathBuf::from(home).join(".cache")
        }
    };
    Ok(base.join("wt").join("store"))
}

pub fn open_store() -> Result<DiskStore, String> {
    let dir = store_dir()?;
    DiskStore::open(&dir).map_err(|e| format!("cannot open store at {}: {e}", dir.display()))
}

/// Everything one heavy directory contributed to the store.
pub struct Ingested {
    /// Repo-relative directory paths to recreate even when empty.
    pub dirs: Vec<String>,
    /// Repo-relative file path -> stored content address.
    pub files: BTreeMap<String, ContentId>,
}

/// Walk `src`, storing every regular file's bytes. Symlinks are never
/// followed out of `src`; they are skipped for now rather than
/// misinterpreted (matching the copy-backend trait's stance).
///
/// Ticket 02: a validation cache beside the store remembers each
/// path's size, mtime, and content id from the previous ingest. A
/// file whose size AND mtime both still match is not read or hashed —
/// its recorded blob is reused (checked against the store first, in
/// case a sweep reclaimed it). Every other file goes through the
/// normal read-and-hash path. The cache can only make runs cheaper,
/// never wronger: materialize proves every blob before placing it
/// (`ensure_verified`, ticket 05), so lying cached metadata fails
/// loudly instead of landing bad bytes in a fresh tree.
pub fn ingest_dir(store: &mut DiskStore, src_root: &Path, src: &Path) -> Result<Ingested, String> {
    let mut ingested = Ingested {
        dirs: Vec::new(),
        files: BTreeMap::new(),
    };
    let mut cache = ValidationCache::open(store.root());
    let mut stack = vec![src.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries =
            fs::read_dir(&dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
        ingested.dirs.push(rel_text(src_root, &dir));
        for entry in entries.flatten() {
            let path = entry.path();
            let file_type = entry
                .file_type()
                .map_err(|e| format!("cannot stat {}: {e}", path.display()))?;
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let rel = rel_text(src_root, &path);
            let meta =
                fs::metadata(&path).map_err(|e| format!("cannot stat {}: {e}", path.display()))?;
            let size = meta.len();
            let mtime = meta.modified().map_err(|e| format!("cannot stat {}: {e}", rel))?;
            let id = match cache.lookup(&rel, size, mtime) {
                // Cache hit: same size and same mtime as last time.
                // Trust it only while the blob is actually still here.
                Some(id) if store.contains(&id) => id,
                _ => {
                    // Miss (or a swept blob): pay for read and hash.
                    let bytes = fs::read(&path)
                        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
                    let id = store.put(&bytes).map_err(|e| e.to_string())?;
                    // An mtime before the epoch cannot round-trip
                    // through the cache format; skip caching rather
                    // than fail, so such a file just stays cold.
                    if mtime >= std::time::UNIX_EPOCH {
                        cache.record(rel.clone(), CacheEntry { size, mtime, id });
                    }
                    id
                }
            };
            ingested.files.insert(rel, id);
        }
    }
    ingested.dirs.sort();
    ingested.dirs.dedup();
    cache
        .save()
        .map_err(|e| format!("cannot update ingest cache: {e}"))?;
    Ok(ingested)
}

fn rel_text(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .expect("path under ingestion root")
        .to_string_lossy()
        .into_owned()
}

/// Outcome of materializing one heavy directory.
pub struct MaterializeReport {
    /// Total files placed.
    pub files: usize,
    /// Files written as plain byte copies because the selected
    /// strategy was disabled or refused by the filesystem.
    pub copied: usize,
    /// Strategy attempted for this directory: `"copy-on-write"`
    /// (default), `"hardlink"` (`WT_HARDLINK=1`), or `"byte-copy"`
    /// (forced by `WT_NO_HARDLINK`, and the answer on platforms with
    /// no clone support yet). Check `copied` to see how much of it
    /// actually happened.
    pub strategy: &'static str,
}

/// Which placement strategy this run uses, from the environment:
///
/// - `WT_NO_HARDLINK` present: forced byte copies (the escape hatch).
///   Kept for compatibility with the ticket 07 flag.
/// - `WT_HARDLINK=1`: experimental hardlinked materialization —
///   maximum space sharing, but linked inodes are made read-only, so
///   tools that rewrite files in place fail loudly.
/// - otherwise: per-file CoW clones on macOS; byte copies elsewhere
///   until Linux reflink is validated for store materialization.
fn select_strategy() -> Option<Box<dyn FileMaterialize>> {
    if std::env::var_os("WT_NO_HARDLINK").is_some() {
        return None;
    }
    if std::env::var_os("WT_HARDLINK").is_some() {
        return Some(Box::new(HardlinkOut));
    }
    #[cfg(target_os = "macos")]
    {
        Some(Box::new(CloneOut))
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// Recreate the ingested tree under `dest_root` from store content.
///
/// Per file: verify first, and only then place anything — a corrupt
/// blob never lands in a fresh tree. Verification is
/// `Store::get`'s read-and-hash when `WT_VERIFY` is set (env policy
/// lives in the CLI layer, like `select_strategy`), and
/// `DiskStore::ensure_verified` otherwise: a blob whose verified
/// ledger fingerprint still matches its stat is trusted without
/// reading a byte; everything else is hashed once and remembered.
///
/// Placement tries the selected strategy (CoW clone by default)
/// against the verified blob; filesystem refusals fall back silently
/// to a byte copy from the blob itself. Directories are pre-created
/// once from the ingested dir list — there is no per-file
/// `create_dir_all`; if placement still hits ENOENT (a directory the
/// manifest's walk never saw), it recreates the parent and retries
/// exactly once, EAFP-style. Permission problems on the destination
/// are real failures and stay loud.
pub fn materialize(
    store: &DiskStore,
    ingested: &Ingested,
    dest_root: &Path,
) -> Result<MaterializeReport, String> {
    let backend = select_strategy();
    let paranoid = std::env::var_os("WT_VERIFY").is_some();
    let mut copied = 0usize;
    for rel in &ingested.dirs {
        fs::create_dir_all(dest_root.join(rel))
            .map_err(|e| format!("cannot prepare {}: {e}", dest_root.join(rel).display()))?;
    }
    for (rel, id) in &ingested.files {
        // Hash verification happens here, before any placement: with
        // fclonefileat the kernel copies bytes directly from the blob
        // at clone time, so the blob must already be known-good or
        // corruption would land. The TOCTOU window between this check
        // and the kernel's read is the one the previous design had in
        // reverse; nothing else guards it today either.
        if paranoid {
            store
                .get(id)
                .map_err(|e| format!("materialize {rel}: {e}"))?;
        } else {
            store
                .ensure_verified(id)
                .map_err(|e| format!("materialize {rel}: {e}"))?;
        }
        let src = store.blob_path(id);
        let dest = dest_root.join(rel);

        match place(backend.as_deref(), &src, &dest) {
            Ok(true) => {}
            Ok(false) => copied += 1,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // The heavy directories themselves were just
                // recreated above; only an unexpected gap should ever
                // land here. Recreate the parent once and retry.
                let parent = dest
                    .parent()
                    .ok_or_else(|| format!("{rel} has no parent"))?;
                fs::create_dir_all(parent)
                    .map_err(|e| format!("cannot prepare {}: {e}", parent.display()))?;
                match place(backend.as_deref(), &src, &dest) {
                    Ok(true) => {}
                    Ok(false) => copied += 1,
                    Err(e) => return Err(format!("materialize {rel}: {e}")),
                }
            }
            Err(e) => return Err(format!("materialize {rel}: {e}")),
        }
    }
    Ok(MaterializeReport {
        files: ingested.files.len(),
        copied,
        strategy: backend.as_ref().map(|b| b.name()).unwrap_or("byte-copy"),
    })
}

/// One placement attempt for one file. Returns whether the selected
/// strategy placed it; `false` means the filesystem refused the
/// strategy and the caller got a plain byte copy instead. Errors come
/// back raw so the caller can distinguish ENOENT (retry after
/// repairing directories) from real failures.
fn place(backend: Option<&dyn FileMaterialize>, src: &Path, dest: &Path) -> std::io::Result<bool> {
    if let Some(backend) = backend {
        match backend.materialize_file(src, dest) {
            Ok(()) => return Ok(true),
            Err(e) if placement_refused(&e) => {}
            Err(e) => return Err(e),
        }
    }
    // Byte-copy fallback reads straight from the verified blob — no
    // need to have held its bytes since verification.
    fs::copy(src, dest)?;
    Ok(false)
}

// Placement refusals ("this filesystem cannot do that") are
// classified by `wt_copy::placement_refused`; everything else is a
// real failure.

/// Resolve the (absolute) git dir of a freshly created worktree.
fn worktree_git_dir(worktree: &Path) -> Result<PathBuf, String> {
    let git_dir = Command::new("git")
        .current_dir(worktree)
        .args(["rev-parse", "--absolute-git-dir"])
        .output()
        .map_err(|e| format!("cannot query git dir: {e}"))?;
    if !git_dir.status.success() {
        return Err("newly created worktree is not a git worktree".into());
    }
    Ok(PathBuf::from(String::from_utf8_lossy(&git_dir.stdout).trim()))
}

/// Give this worktree one reference on every distinct blob it uses,
/// then record the mapping where ticket 06 can find it.
///
/// Ticket 07: in `mark-sweep-no-refs` mode the refcount writes are
/// skipped entirely — mirrors are the only bookkeeping. The sidecar
/// is written in every mode; it stays the diagnostic/recovery record
/// and the liveness evidence sweep validates roots against.
pub fn claim_references(
    store: &mut DiskStore,
    worktree: &Path,
    ingested: &Ingested,
) -> Result<PathBuf, String> {
    let distinct: BTreeSet<&ContentId> = ingested.files.values().collect();
    if store.gc_mode() != GcMode::MarkSweepNoRefs {
        for id in &distinct {
            store.add_ref(id).map_err(|e| e.to_string())?;
        }
    }

    let git_dir = worktree_git_dir(worktree)?;
    let mut sidecar = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(git_dir.join("wt-hydrated.tsv"))
        .map_err(|e| format!("cannot open hydration ledger: {e}"))?;
    for (rel, id) in &ingested.files {
        writeln!(sidecar, "{rel}\t{id}").map_err(|e| format!("cannot write ledger: {e}"))?;
    }
    Ok(git_dir)
}

/// Publish the authoritative store-local mirror for one successful
/// create: one atomic write naming the worktree's canonical identity
/// and every distinct blob it hydrates from (ticket 07). This — not
/// the per-blob refcounts — is what mark-and-sweep marks through.
pub fn publish_mirror(
    store: &mut DiskStore,
    worktree: &Path,
    git_dir: &Path,
    ingested: &Ingested,
) -> Result<(), String> {
    let distinct: BTreeSet<&ContentId> = ingested.files.values().collect();
    store
        .publish_worktree_mirror(worktree, git_dir, distinct)
        .map(|_| ())
        .map_err(|e| format!("cannot publish worktree mirror: {e}"))
}
