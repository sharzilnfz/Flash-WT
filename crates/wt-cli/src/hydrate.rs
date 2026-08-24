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
use std::io::{self, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[cfg(target_os = "macos")]
use wt_copy::CloneOut;
use wt_copy::{FileMaterialize, HardlinkOut, placement_refused};
#[cfg(target_os = "macos")]
use wt_store::bulkwalk;
use wt_store::{ContentId, DiskStore, Entry as CacheEntry, GcMode, Store, ValidationCache};

use crate::config::{RunConfig, StrategyPolicy};
use crate::error::{Error, Result};
use crate::gitops;

/// Where the per-machine store lives. `$WT_STORE` wins (tests use it
/// for isolation); otherwise XDG cache conventions.
fn store_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("WT_STORE") {
        return Ok(PathBuf::from(dir));
    }
    let base = match std::env::var_os("XDG_CACHE_HOME") {
        Some(xdg) => PathBuf::from(xdg),
        None => {
            let home = std::env::var_os("HOME").ok_or_else(|| {
                Error::Usage("cannot locate a home directory for the store".into())
            })?;
            PathBuf::from(home).join(".cache")
        }
    };
    Ok(base.join("wt").join("store"))
}

pub fn open_store() -> Result<DiskStore> {
    let dir = store_dir()?;
    DiskStore::open(&dir)
        .map_err(|e| Error::Store(format!("cannot open store at {}: {e}", dir.display())))
}

/// Everything one heavy directory contributed to the store.
pub struct Ingested {
    /// Repo-relative directory paths to recreate even when empty.
    pub dirs: Vec<String>,
    /// Repo-relative file path -> stored content address.
    pub files: BTreeMap<String, ContentId>,
    /// Ticket 08 (only populated when `WT_SNAPSHOTS=1`): repo-relative
    /// symlink path -> raw target. Symlinks are recorded, never
    /// followed or stored as blobs; only the snapshot manifest
    /// consumes them.
    pub symlinks: BTreeMap<String, String>,
    /// Ticket 08 (same gate): repo-relative file path -> on-disk mode.
    /// Only the snapshot manifest consumes these; the per-file ladder
    /// keeps its existing normalized-mode behavior untouched.
    pub modes: BTreeMap<String, u32>,
}

/// Walk `src`, storing every regular file's bytes. Symlinks are never
/// followed out of `src`; with the snapshot gate off they are skipped
/// for now rather than misinterpreted (matching the copy-backend
/// trait's stance). With `WT_SNAPSHOTS=1` they are recorded for the
/// manifest instead, and non-regular files fail loudly rather than
/// silently vanishing from a snapshot.
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
pub fn ingest_dir(
    store: &mut DiskStore,
    src_root: &Path,
    src: &Path,
    cfg: &RunConfig,
) -> Result<Ingested> {
    let snapshots = cfg.snapshots;
    let mut ingested = Ingested {
        dirs: Vec::new(),
        files: BTreeMap::new(),
        symlinks: BTreeMap::new(),
        modes: BTreeMap::new(),
    };
    let mut cache = ValidationCache::open(store.root());

    // macOS fast path (Step 0 follow-up): one getattrlistbulk per
    // directory replaces readdir-plus-per-file-stat — at 40k files
    // that is ~40k fewer syscalls. Only engaged with snapshots on,
    // where every stat'd attribute is actually consumed; any failure
    // silently falls back to the portable walk below.
    #[cfg(target_os = "macos")]
    let walked = if snapshots {
        bulkwalk::walk(src).ok()
    } else {
        None
    };
    // Never constructed off macOS; the type exists only so the match
    // below compiles unchanged.
    #[cfg(not(target_os = "macos"))]
    let walked: Option<std::convert::Infallible> = None;

    match walked {
        #[cfg(target_os = "macos")]
        Some(entries) => {
            // Bulk entries are relative to `src`; every consumer (and
            // the legacy walk) speaks repo-relative paths, so prefix
            // with the heavy directory's own repo-relative name.
            let base = rel_text(src_root, src)?;
            ingested.dirs.push(base.clone());
            for entry in entries {
                let rel = if base.is_empty() {
                    entry.rel_path.clone()
                } else {
                    format!("{base}/{}", entry.rel_path)
                };
                let path = src.join(&entry.rel_path);
                if entry.is_symlink {
                    let target =
                        fs::read_link(&path).map_err(|e| Error::io("read symlink", &path, e))?;
                    ingested
                        .symlinks
                        .insert(rel.clone(), target.to_string_lossy().into_owned());
                    continue;
                }
                if entry.is_dir {
                    ingested.dirs.push(rel);
                    continue;
                }
                if !entry.is_file {
                    if snapshots {
                        return Err(Error::Store(format!(
                            "{} is not a regular file (fifos/sockets/devices are unsupported)",
                            path.display()
                        )));
                    }
                    continue;
                }
                if snapshots {
                    ingested.modes.insert(rel.clone(), entry.mode & 0o7777);
                }
                let mtime = std::time::UNIX_EPOCH
                    + std::time::Duration::new(entry.mtime_secs, entry.mtime_nanos);
                let id = match cache.lookup(&rel, entry.size, mtime) {
                    Some(id) if store.contains(&id) => id,
                    _ => {
                        let bytes = fs::read(&path).map_err(|e| Error::io("read", &path, e))?;
                        let id = store.put(&bytes)?;
                        cache.record(
                            rel.clone(),
                            CacheEntry {
                                size: entry.size,
                                mtime,
                                id,
                            },
                        );
                        id
                    }
                };
                ingested.files.insert(rel, id);
            }
        }
        None => ingest_dir_walk(store, &mut cache, &mut ingested, src_root, src, snapshots)?,
    }

    ingested.dirs.sort();
    ingested.dirs.dedup();
    cache
        .save()
        .map_err(|e| Error::io_unanchored("update ingest cache", store.root(), e))?;
    Ok(ingested)
}

/// The portable read_dir+metadata walk: one `fs::metadata` per regular
/// file on top of each directory's readdir. Also the fallback for the
/// macOS bulk walker.
fn ingest_dir_walk(
    store: &mut DiskStore,
    cache: &mut ValidationCache,
    ingested: &mut Ingested,
    src_root: &Path,
    src: &Path,
    snapshots: bool,
) -> Result<()> {
    let mut stack = vec![src.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = fs::read_dir(&dir).map_err(|e| Error::io("read", &dir, e))?;
        ingested.dirs.push(rel_text(src_root, &dir)?);
        for entry in entries.flatten() {
            let path = entry.path();
            let file_type = entry.file_type().map_err(|e| Error::io("stat", &path, e))?;
            if file_type.is_symlink() {
                // Ticket 08: with snapshots on, symlinks are recorded
                // faithfully in the manifest (target only — targets are
                // never stored as blobs). With the gate off, the long-
                // standing skip applies unchanged.
                if snapshots {
                    let target =
                        fs::read_link(&path).map_err(|e| Error::io("read symlink", &path, e))?;
                    ingested.symlinks.insert(
                        rel_text(src_root, &path)?,
                        target.to_string_lossy().into_owned(),
                    );
                }
                continue;
            }
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            if !file_type.is_file() {
                // FIFOs, sockets, devices have no place in a snapshot:
                // fail loudly before placement rather than silently
                // dropping content the manifest cannot represent.
                if snapshots {
                    return Err(Error::Store(format!(
                        "{} is not a regular file (fifos/sockets/devices are unsupported)",
                        path.display()
                    )));
                }
                continue;
            }
            let rel = rel_text(src_root, &path)?;
            let meta = fs::metadata(&path).map_err(|e| Error::io("stat", &path, e))?;
            if snapshots {
                ingested.modes.insert(rel.clone(), meta.mode() & 0o7777);
            }
            let size = meta.len();
            let mtime = meta.modified().map_err(|e| Error::io("stat", &rel, e))?;
            let id = match cache.lookup(&rel, size, mtime) {
                // Cache hit: same size and same mtime as last time.
                // Trust it only while the blob is actually still here.
                Some(id) if store.contains(&id) => id,
                _ => {
                    // Miss (or a swept blob): pay for read and hash.
                    let bytes = fs::read(&path).map_err(|e| Error::io("read", &path, e))?;
                    let id = store.put(&bytes)?;
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
    Ok(())
}

/// Repo-relative text of `path` under `root`, or a loud error when a
/// pattern somehow matched outside the ingestion root.
fn rel_text(root: &Path, path: &Path) -> Result<String> {
    path.strip_prefix(root)
        .map_err(|_| {
            Error::Store(format!(
                "pattern matched path outside repository root: {}",
                path.display()
            ))
        })
        .map(|p| p.to_string_lossy().into_owned())
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
    /// Step 0 instrumentation: milliseconds spent proving blobs
    /// before placement (`Store::get` under `WT_VERIFY`, else
    /// `DiskStore::ensure_verified`).
    pub verify_ms: u128,
    /// Step 0 instrumentation: milliseconds spent placing files —
    /// the selected strategy, byte-copy fallbacks, and the ENOENT
    /// parent-repair retry included.
    pub place_ms: u128,
}

/// Which placement strategy this run uses, from the startup policy:
///
/// - `ForceByteCopy` (`WT_NO_HARDLINK` on): forced byte copies (the
///   escape hatch). Kept for compatibility with the ticket 07 flag.
/// - `Hardlink` (`WT_HARDLINK` on): experimental hardlinked
///   materialization — maximum space sharing, but linked inodes are
///   made read-only, so tools that rewrite files in place fail loudly.
/// - `Default`: per-file CoW clones on macOS; byte copies elsewhere
///   until Linux reflink is validated for store materialization.
fn select_strategy(policy: StrategyPolicy) -> Option<Box<dyn FileMaterialize>> {
    match policy {
        StrategyPolicy::ForceByteCopy => None,
        StrategyPolicy::Hardlink => Some(Box::new(HardlinkOut)),
        StrategyPolicy::Default => {
            #[cfg(target_os = "macos")]
            {
                Some(Box::new(CloneOut))
            }
            #[cfg(not(target_os = "macos"))]
            {
                None
            }
        }
    }
}

/// Recreate the ingested tree under `dest_root` from store content.
///
/// Per file: verify first, and only then place anything — a corrupt
/// blob never lands in a fresh tree. Verification is
/// `Store::get`'s read-and-hash when `RunConfig::verify` is set, and
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
    cfg: &RunConfig,
) -> Result<MaterializeReport> {
    let backend = select_strategy(cfg.strategy_policy);
    let paranoid = cfg.verify;
    let mut copied = 0usize;
    let mut verify_ms = 0u128;
    let mut place_ms = 0u128;
    for rel in &ingested.dirs {
        fs::create_dir_all(dest_root.join(rel))
            .map_err(|e| Error::io("prepare", dest_root.join(rel), e))?;
    }
    for (rel, id) in &ingested.files {
        // Hash verification happens here, before any placement: with
        // fclonefileat the kernel copies bytes directly from the blob
        // at clone time, so the blob must already be known-good or
        // corruption would land. The TOCTOU window between this check
        // and the kernel's read is the one the previous design had in
        // reverse; nothing else guards it today either.
        if paranoid {
            let stage = Instant::now();
            store
                .get(id)
                .map_err(|e| Error::Store(format!("materialize {rel}: {e}")))?;
            verify_ms += stage.elapsed().as_millis();
        } else {
            let stage = Instant::now();
            store
                .ensure_verified(id)
                .map_err(|e| Error::Store(format!("materialize {rel}: {e}")))?;
            verify_ms += stage.elapsed().as_millis();
        }
        let src = store.blob_path(id);
        let dest = dest_root.join(rel);

        // The ENOENT parent-repair retry is part of placement cost.
        let stage = Instant::now();
        match place(backend.as_deref(), &src, &dest) {
            Ok(true) => {}
            Ok(false) => copied += 1,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // The heavy directories themselves were just
                // recreated above; only an unexpected gap should ever
                // land here. Recreate the parent once and retry.
                let parent = dest
                    .parent()
                    .ok_or_else(|| Error::Store(format!("{rel} has no parent")))?;
                fs::create_dir_all(parent).map_err(|e| Error::io("prepare", parent, e))?;
                match place(backend.as_deref(), &src, &dest) {
                    Ok(true) => {}
                    Ok(false) => copied += 1,
                    Err(e) => return Err(Error::Store(format!("materialize {rel}: {e}"))),
                }
            }
            Err(e) => return Err(Error::Store(format!("materialize {rel}: {e}"))),
        }
        place_ms += stage.elapsed().as_millis();
    }
    Ok(MaterializeReport {
        files: ingested.files.len(),
        copied,
        strategy: backend.as_ref().map(|b| b.name()).unwrap_or("byte-copy"),
        verify_ms,
        place_ms,
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
fn worktree_git_dir(worktree: &Path) -> Result<PathBuf> {
    gitops::git_dir(worktree)
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
) -> Result<PathBuf> {
    let distinct: BTreeSet<&ContentId> = ingested.files.values().collect();
    if store.gc_mode() != GcMode::MarkSweepNoRefs {
        for id in &distinct {
            store.add_ref(id)?;
        }
    }

    let git_dir = worktree_git_dir(worktree)?;
    let sidecar_file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(git_dir.join("wt-hydrated.tsv"))
        .map_err(|e| {
            Error::io_unanchored("open hydration ledger", git_dir.join("wt-hydrated.tsv"), e)
        })?;
    // 40k one-line writes through an unbuffered handle cost 40k
    // write(2) syscalls; a BufWriter turns that into a handful of
    // large ones.
    let mut sidecar = io::BufWriter::with_capacity(128 * 1024, sidecar_file);
    for (rel, id) in &ingested.files {
        writeln!(sidecar, "{rel}\t{id}").map_err(|e| {
            Error::io_unanchored("write ledger", git_dir.join("wt-hydrated.tsv"), e)
        })?;
    }
    sidecar
        .flush()
        .map_err(|e| Error::io_unanchored("write ledger", git_dir.join("wt-hydrated.tsv"), e))?;
    Ok(git_dir)
}

/// Publish the authoritative store-local mirror for one successful
/// create: one atomic write naming the worktree's canonical identity
/// and every blob or snapshot it hydrates from (ticket 07). This —
/// not the per-blob refcounts — is what mark-and-sweep marks through.
/// A snapshot hydration contributes ONLY its `snapshot` record; the
/// manifest marks every child blob.
pub fn publish_mirror(
    store: &mut DiskStore,
    worktree: &Path,
    git_dir: &Path,
    ingested: &Ingested,
    snapshots: &[ContentId],
) -> Result<()> {
    let distinct: BTreeSet<&ContentId> = ingested.files.values().collect();
    store
        .publish_worktree_mirror(worktree, git_dir, distinct, snapshots.iter())
        .map(|_| ())
        .map_err(|e| Error::Store(format!("cannot publish worktree mirror: {e}")))
}

/// Ticket 08: give a snapshot-hydrated worktree its bookkeeping.
///
/// Legacy refs are still claimed on every distinct child blob (dual-
/// write safety: an old binary reading the store must not collect
/// blobs it cannot see through snapshot manifests), but the sidecar
/// carries TYPED rows so removal knows which ids are blobs and which
/// name snapshots:
///
/// ```text
/// <rel><TAB>blob<TAB><64-hex-blob-id>
/// -<TAB>snapshot<TAB><64-hex-manifest-hash>
/// ```
///
/// (Two-field rows remain the legacy per-file format from ticket 05.)
/// The store-local mirror itself gets only the `snapshot` record.
pub fn claim_snapshot_references(
    store: &mut DiskStore,
    worktree: &Path,
    ingested: &Ingested,
    snapshot: ContentId,
) -> Result<PathBuf> {
    use wt_store::Store as _;

    let distinct: BTreeSet<&ContentId> = ingested.files.values().collect();
    if store.gc_mode() != GcMode::MarkSweepNoRefs {
        for id in &distinct {
            store.add_ref(id)?;
        }
    }

    let git_dir = worktree_git_dir(worktree)?;
    let sidecar_file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(git_dir.join("wt-hydrated.tsv"))
        .map_err(|e| {
            Error::io_unanchored("open hydration ledger", git_dir.join("wt-hydrated.tsv"), e)
        })?;
    // Same buffering rationale as claim_references.
    let mut sidecar = io::BufWriter::with_capacity(128 * 1024, sidecar_file);
    for (rel, id) in &ingested.files {
        writeln!(sidecar, "{rel}\tblob\t{id}").map_err(|e| {
            Error::io_unanchored("write ledger", git_dir.join("wt-hydrated.tsv"), e)
        })?;
    }
    writeln!(sidecar, "-\tsnapshot\t{snapshot}")
        .map_err(|e| Error::io_unanchored("write ledger", git_dir.join("wt-hydrated.tsv"), e))?;
    sidecar
        .flush()
        .map_err(|e| Error::io_unanchored("write ledger", git_dir.join("wt-hydrated.tsv"), e))?;
    Ok(git_dir)
}
