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
use std::os::unix::fs::{MetadataExt, PermissionsExt};
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
    /// Repo-relative directory path -> on-disk mode (`& 0o7777`).
    /// Recorded on every ingest; materialize restores these bits
    /// after placement, because `create_dir_all` normalizes new
    /// directories through the process umask just like it once did
    /// for files.
    pub dir_modes: BTreeMap<String, u32>,
    /// Repo-relative file path -> stored content address.
    pub files: BTreeMap<String, ContentId>,
    /// Repo-relative symlink path -> raw target (possibly dangling;
    /// targets are never followed or stored as blobs). Recorded on
    /// every ingest so both the fallback ladder and the snapshot
    /// manifest can recreate the link verbatim.
    pub symlinks: BTreeMap<String, String>,
    /// Repo-relative file path -> on-disk mode (`& 0o7777`). Recorded
    /// on every ingest; the fallback ladder restores it after
    /// placement, the snapshot manifest consumes it too.
    pub modes: BTreeMap<String, u32>,
}

/// Walk `src`, storing every regular file's bytes. Symlinks are never
/// followed out of `src`; they are recorded with their raw targets
/// (dangling ones included) so materialize can recreate them
/// verbatim, and non-regular files are skipped — except under
/// `WT_SNAPSHOTS=1`, where anything a manifest cannot represent fails
/// loudly rather than silently vanishing from a snapshot.
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
        dir_modes: BTreeMap::new(),
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
            // The heavy root itself is a directory the manifest must
            // carry (its clone replaces it wholesale on the snapshot
            // path, but the per-file ladder recreates it).
            let root_meta = fs::symlink_metadata(src).map_err(|e| Error::io("stat", src, e))?;
            ingested
                .dir_modes
                .insert(base.clone(), root_meta.mode() & 0o7777);
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
                        .insert(rel, target.to_string_lossy().into_owned());
                    continue;
                }
                if entry.is_dir {
                    ingested.dir_modes.insert(rel.clone(), entry.mode & 0o7777);
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
        let rel = rel_text(src_root, &dir)?;
        // One extra stat per DIRECTORY (not per file): its permission
        // bits must survive hydration, since create_dir_all cannot
        // carry them through the umask.
        let dir_meta = fs::symlink_metadata(&dir).map_err(|e| Error::io("stat", &dir, e))?;
        ingested
            .dir_modes
            .insert(rel.clone(), dir_meta.mode() & 0o7777);
        ingested.dirs.push(rel);
        for entry in entries.flatten() {
            let path = entry.path();
            let file_type = entry.file_type().map_err(|e| Error::io("stat", &path, e))?;
            if file_type.is_symlink() {
                // The raw target is what gets recorded, dangling or
                // not: materialize recreates it verbatim via
                // symlink(2), which never resolves the target. (With
                // snapshots on the manifest consumes the same record.)
                let target =
                    fs::read_link(&path).map_err(|e| Error::io("read symlink", &path, e))?;
                ingested.symlinks.insert(
                    rel_text(src_root, &path)?,
                    target.to_string_lossy().into_owned(),
                );
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
            ingested.modes.insert(rel.clone(), meta.mode() & 0o7777);
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
    /// strategy was disabled, refused by the filesystem, or could not
    /// carry the recorded mode (a hardlink whose exec bits mismatched
    /// is replaced by a private copy — that replacement counts here).
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
/// to a byte copy from the blob itself. After placement each file
/// gets the mode recorded at ingest time (the exec bit survives even
/// though blobs themselves are normalized), and every ingested
/// symlink is recreated verbatim from its raw target, dangling or
/// not. Directories are pre-created once from the ingested dir
/// list — there is no per-file `create_dir_all`; if placement still
/// hits ENOENT (a directory the manifest's walk never saw), it
/// recreates the parent and retries exactly once, EAFP-style.
/// Permission problems on the destination are real failures and stay
/// loud.
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
        let placed = match place(backend.as_deref(), &src, &dest) {
            Ok(placed) => placed,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // The heavy directories themselves were just
                // recreated above; only an unexpected gap should ever
                // land here. Recreate the parent once and retry.
                let parent = dest
                    .parent()
                    .ok_or_else(|| Error::Store(format!("{rel} has no parent")))?;
                fs::create_dir_all(parent).map_err(|e| Error::io("prepare", parent, e))?;
                match place(backend.as_deref(), &src, &dest) {
                    Ok(placed) => placed,
                    Err(e) => return Err(Error::Store(format!("materialize {rel}: {e}"))),
                }
            }
            Err(e) => return Err(Error::Store(format!("materialize {rel}: {e}"))),
        };
        if !placed {
            copied += 1;
        }
        if let Some(&mode) = ingested.modes.get(rel) {
            // A mode can only be applied in place when the placement
            // owns its inode; see `finalize_mode` for the shared case.
            let shared_inode = placed
                && backend
                    .as_deref()
                    .is_some_and(|b| b.shares_inode_with_source());
            match finalize_mode(shared_inode, mode, &src, &dest) {
                Ok(repaired) => {
                    // The shared-inode repair IS a byte copy: count it
                    // so `copied` reports honestly how much of the
                    // hardlink strategy actually stuck.
                    if repaired {
                        copied += 1;
                    }
                }
                Err(e) => return Err(Error::Store(format!("materialize {rel}: {e}"))),
            }
        }
        place_ms += stage.elapsed().as_millis();
    }
    for (rel, target) in &ingested.symlinks {
        let dest = dest_root.join(rel);
        // The dirs pass created every walked directory; this only
        // covers a parent that somehow escaped the walk (same EAFP
        // stance as the files above).
        if let Some(parent) = dest.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent).map_err(|e| Error::io("prepare", parent, e))?;
            }
        }
        std::os::unix::fs::symlink(target, &dest)
            .map_err(|e| Error::io("place symlink", &dest, e))?;
    }
    // Restore directory permission bits AFTER placement (a restrictive
    // parent must not block file creation) and deepest-first: sorted
    // ascending puts every ancestor before its descendants, so the
    // reverse walk fixes children while their ancestors can still be
    // traversed. Without this pass, `create_dir_all` leaves every
    // directory umask-normalized and modes like 0700 or 0500 are lost.
    for rel in ingested.dirs.iter().rev() {
        let Some(&mode) = ingested.dir_modes.get(rel) else {
            continue;
        };
        let path = dest_root.join(rel);
        fs::set_permissions(&path, fs::Permissions::from_mode(mode))
            .map_err(|e| Error::io("restore dir mode", &path, e))?;
    }
    Ok(MaterializeReport {
        files: ingested.files.len(),
        copied,
        strategy: backend.as_ref().map(|b| b.name()).unwrap_or("byte-copy"),
        verify_ms,
        place_ms,
    })
}

/// Bring one freshly placed file to the mode recorded at ingest time.
///
/// Blobs are normalized (0644) and deduped by content only, so the
/// recorded mode is per PATH and must be applied after placement. On
/// a private inode that is a plain chmod. A hardlinked placement
/// shares its inode with the store blob and every sibling tree —
/// chmod there would leak both directions, and re-adding write bits
/// would defeat the read-only guard — so a link whose exec bits do
/// not match the record is replaced by a private byte copy first,
/// which then takes any mode safely. Exec-bit parity is all a shared
/// inode can ever carry faithfully; the stripped write bits stay.
///
/// Returns `true` when that replacement happened: the caller counts
/// it as a byte copy in [`MaterializeReport::copied`].
fn finalize_mode(shared_inode: bool, mode: u32, src: &Path, dest: &Path) -> io::Result<bool> {
    let current = fs::metadata(dest)?.permissions().mode();
    if shared_inode {
        if current & 0o111 == mode & 0o111 {
            return Ok(false);
        }
        fs::remove_file(dest)?;
        fs::copy(src, dest)?;
        fs::set_permissions(dest, fs::Permissions::from_mode(mode))?;
        return Ok(true);
    }
    if current & 0o7777 != mode {
        fs::set_permissions(dest, fs::Permissions::from_mode(mode))?;
    }
    Ok(false)
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
