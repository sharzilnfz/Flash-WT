//! Tree ingestion: walk source directories into the content-addressed
//! store.
//!
//! Ingesting a tree is a storage engine responsibility: walk the
//! source, store every unique file's bytes once, remember what was
//! already seen in the validation cache beside the store, and hand
//! back an [`Ingested`] summary describing everything the walk found.
//!
//! Symlinks are never followed out of the source; they are recorded
//! with their raw targets (dangling ones included) so a consumer can
//! recreate them verbatim, and non-regular files are skipped — except
//! with [`IngestOptions::snapshots`] on, where anything a snapshot
//! manifest cannot represent fails loudly rather than silently
//! vanishing.
//!
//! The validation cache remembers each path's size, mtime, and
//! content id from the previous ingest. A file whose size AND mtime
//! both still match is not read or hashed — its recorded blob is
//! reused (checked against the store first, in case a sweep reclaimed
//! it). Every other file goes through the normal read-and-hash path.
//! The cache can only make runs cheaper, never wronger: consumers
//! prove every blob before placing it, so lying cached metadata fails
//! loudly instead of landing bad bytes in a fresh tree.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, UNIX_EPOCH};

#[cfg(target_os = "macos")]
use crate::bulkwalk;
use crate::snapshot::{EntryKind, Manifest, SnapshotEntry};
use crate::validation::{Entry, ValidationCache};
use crate::{ContentId, DiskStore, Error, Result};

/// Everything one ingested tree contributed to the store.
#[derive(Debug)]
pub struct Ingested {
    /// Source-relative directory paths to recreate even when empty.
    pub dirs: Vec<String>,
    /// Source-relative directory path -> on-disk mode (`& 0o7777`).
    /// Recorded on every ingest; consumers restore these bits after
    /// placement, because `create_dir_all` normalizes new directories
    /// through the process umask just like it once did for files.
    pub dir_modes: BTreeMap<String, u32>,
    /// Source-relative file path -> stored content address.
    pub files: BTreeMap<String, ContentId>,
    /// Source-relative file path -> file size in bytes.
    pub file_sizes: BTreeMap<String, u64>,
    /// Source-relative symlink path -> raw target (possibly dangling;
    /// targets are never followed or stored as blobs). Recorded on
    /// every ingest so both per-file placement and snapshot manifests
    /// can recreate the link verbatim.
    pub symlinks: BTreeMap<String, String>,
    /// Source-relative file path -> on-disk mode (`& 0o7777`).
    /// Recorded on every ingest; consumers restore it after placement,
    /// snapshot manifests consume it too.
    pub modes: BTreeMap<String, u32>,
}

impl Ingested {
    /// Canonical snapshot view of this ingest, relative to `heavy_rel`.
    ///
    /// Single source of truth for `Ingested` -> `Manifest`: directory
    /// modes come from `dir_modes` (not a fixed `0o755`), file modes
    /// must be present (no silent `0o644` fallback), sizes must be
    /// present, and `heavy_rel` itself is skipped as the implicit
    /// snapshot root. Total size sums unique blob sizes from
    /// `file_sizes`, matching the historical projection computation;
    /// the publish path still recomputes via blob stat.
    pub fn to_snapshot_manifest(
        &self,
        heavy_rel: &str,
        lockfile_hash: Option<ContentId>,
    ) -> std::result::Result<Manifest, String> {
        manifest_from_parts(
            &self.dirs,
            &self.dir_modes,
            &self.files,
            &self.file_sizes,
            &self.symlinks,
            &self.modes,
            heavy_rel,
            lockfile_hash,
        )
    }

    /// Owned form of [`Self::to_snapshot_manifest`].
    pub fn into_snapshot_manifest(
        self,
        heavy_rel: &str,
        lockfile_hash: Option<ContentId>,
    ) -> std::result::Result<Manifest, String> {
        self.to_snapshot_manifest(heavy_rel, lockfile_hash)
    }
}

/// Build a snapshot manifest from borrowed ingest parts without
/// cloning the maps. Used by `Ingested::to_snapshot_manifest` and by
/// the snapshot projection bridge so the conversion lives in exactly
/// one place.
#[allow(clippy::too_many_arguments)]
pub fn manifest_from_parts(
    dirs: &[String],
    dir_modes: &BTreeMap<String, u32>,
    files: &BTreeMap<String, ContentId>,
    file_sizes: &BTreeMap<String, u64>,
    symlinks: &BTreeMap<String, String>,
    modes: &BTreeMap<String, u32>,
    heavy_rel: &str,
    lockfile_hash: Option<ContentId>,
) -> std::result::Result<Manifest, String> {
    let prefix = format!("{heavy_rel}/");
    let strip = |path: &str| -> std::result::Result<String, String> {
        path.strip_prefix(&prefix)
            .map(str::to_owned)
            .ok_or_else(|| format!("ingested path {path:?} lies outside {heavy_rel:?}"))
    };
    let mut entries = Vec::new();
    for dir in dirs {
        if dir == heavy_rel {
            continue;
        }
        let rel = strip(dir)?;
        let raw = dir_modes.get(dir).copied().unwrap_or(0o755) & 0o7777;
        entries.push(SnapshotEntry {
            rel,
            kind: EntryKind::Dir,
            mode: raw,
            blob: None,
            target: None,
        });
    }
    for (path, id) in files {
        let rel = strip(path)?;
        let mode = modes.get(path).copied().ok_or_else(|| {
            format!("ingested file {path:?} lacks a recorded mode (files/modes invariant)")
        })?;
        file_sizes.get(path).ok_or_else(|| {
            format!("ingested file {path:?} lacks a recorded size (files/file_sizes invariant)")
        })?;
        entries.push(SnapshotEntry::file(rel, *id, mode));
    }
    for (path, target) in symlinks {
        entries.push(SnapshotEntry::symlink(strip(path)?, target));
    }
    let mut unique_blobs = BTreeMap::new();
    for (rel, id) in files {
        if let Some(&size) = file_sizes.get(rel) {
            unique_blobs.entry(*id).or_insert(size);
        }
    }
    let total_size: u64 = unique_blobs.values().sum();
    Manifest::new_with_lockfile_and_size(entries, lockfile_hash, total_size)
}

/// Options for [`DiskStore::ingest_tree`].
pub struct IngestOptions<'a> {
    /// Record mode bits and fail loudly on non-regular files (fifos,
    /// sockets, devices), as the snapshot manifest path requires.
    pub snapshots: bool,
    /// Predicate over source-relative paths; a `true` return skips the
    /// path entirely — and for a directory, everything beneath it.
    pub exclude: &'a dyn Fn(&str) -> bool,
}

impl DiskStore {
    /// Walk `src` (a directory under `src_root`), storing every regular
    /// file's bytes. Returns everything the walk found, with paths
    /// relative to `src_root`.
    pub fn ingest_tree(
        &mut self,
        src_root: &Path,
        src: &Path,
        options: &IngestOptions<'_>,
    ) -> Result<Ingested> {
        let mut ingested = Ingested {
            dirs: Vec::new(),
            dir_modes: BTreeMap::new(),
            files: BTreeMap::new(),
            file_sizes: BTreeMap::new(),
            symlinks: BTreeMap::new(),
            modes: BTreeMap::new(),
        };
        let mut cache = ValidationCache::open(self.root());

        // macOS fast path: one getattrlistbulk per directory replaces
        // readdir-plus-per-file-stat — at 40k files that is ~40k fewer
        // syscalls. Only engaged with snapshots on, where every stat'd
        // attribute is actually consumed; any failure silently falls
        // back to the portable walk below.
        #[cfg(target_os = "macos")]
        let walked = if options.snapshots {
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
                let mut pending = Vec::new();
                // Bulk entries are relative to `src`; every consumer (and
                // the legacy walk) speaks source-root-relative paths, so
                // prefix with the ingested tree's own source-relative name.
                let base = rel_text(src_root, src)?;
                // The tree root itself is a directory consumers must
                // carry (a snapshot clone replaces it wholesale, but
                // per-file placement recreates it).
                let root_meta = fs::symlink_metadata(src).map_err(|e| io_ctx("stat", src, e))?;
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
                    if (options.exclude)(&rel) {
                        continue;
                    }
                    let path = src.join(&entry.rel_path);
                    if entry.is_symlink {
                        ingest_symlink(&mut ingested, rel, &path)?;
                        continue;
                    }
                    if entry.is_dir {
                        ingested.dir_modes.insert(rel.clone(), entry.mode & 0o7777);
                        ingested.dirs.push(rel);
                        continue;
                    }
                    if !entry.is_file {
                        if options.snapshots {
                            return Err(io_ctx(
                                "ingest",
                                &path,
                                io::Error::other(
                                    "not a regular file (fifos/sockets/devices are unsupported)",
                                ),
                            ));
                        }
                        continue;
                    }
                    let mtime = UNIX_EPOCH + Duration::new(entry.mtime_secs, entry.mtime_nanos);
                    let ctime = UNIX_EPOCH + Duration::new(entry.ctime_secs, entry.ctime_nanos);
                    let inode = entry.inode;
                    process_file_entry(
                        self,
                        &mut cache,
                        &mut ingested,
                        &mut pending,
                        rel,
                        path,
                        entry.size,
                        mtime,
                        inode,
                        ctime,
                        entry.mode,
                    )?;
                }
                flush_pending(self, &mut cache, &mut ingested, &mut pending)?;
            }
            None => ingest_tree_walk(self, &mut cache, &mut ingested, src_root, src, options)?,
        }

        ingested.dirs.sort();
        ingested.dirs.dedup();
        cache.save().map_err(Error::Io)?;
        Ok(ingested)
    }
}

const INGEST_BATCH_SIZE: usize = 1024;

struct PendingFile {
    rel: String,
    path: PathBuf,
    size: u64,
    mtime: std::time::SystemTime,
    inode: u64,
    ctime: std::time::SystemTime,
}

/// Stream-hash a file using SHA-256 in 64KB chunks into a [`ContentId`].
///
/// Large files never hold full contents in memory, keeping memory consumption
/// bounded to 64KB regardless of file size.
pub fn stream_hash_file(path: &Path) -> io::Result<ContentId> {
    let mut buf = vec![0u8; 64 * 1024];
    stream_hash_file_with_buf(path, &mut buf)
}

pub(crate) fn stream_hash_file_with_buf(path: &Path, buf: &mut [u8]) -> io::Result<ContentId> {
    use sha2::{Digest, Sha256};

    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    loop {
        let n = file.read(buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(ContentId(hasher.finalize().into()))
}

fn hash_pending_files(pending: &[PendingFile]) -> Vec<io::Result<ContentId>> {
    if pending.is_empty() {
        return Vec::new();
    }
    if pending.len() == 1 {
        let mut buf = vec![0u8; 64 * 1024];
        return vec![stream_hash_file_with_buf(&pending[0].path, &mut buf)];
    }

    let num_threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 8)
        .min(pending.len());

    if num_threads <= 1 {
        let mut buf = vec![0u8; 64 * 1024];
        return pending
            .iter()
            .map(|f| stream_hash_file_with_buf(&f.path, &mut buf))
            .collect();
    }

    let mut results: Vec<Option<io::Result<ContentId>>> =
        (0..pending.len()).map(|_| None).collect();
    let next_index = AtomicUsize::new(0);

    std::thread::scope(|s| {
        let mut handles = Vec::with_capacity(num_threads);
        for _ in 0..num_threads {
            handles.push(s.spawn(|| {
                let mut buf = vec![0u8; 64 * 1024];
                let mut local = Vec::new();
                loop {
                    let idx = next_index.fetch_add(1, Ordering::Relaxed);
                    if idx >= pending.len() {
                        break;
                    }
                    let res = stream_hash_file_with_buf(&pending[idx].path, &mut buf);
                    local.push((idx, res));
                }
                local
            }));
        }

        for handle in handles {
            if let Ok(local) = handle.join() {
                for (idx, res) in local {
                    results[idx] = Some(res);
                }
            }
        }
    });

    results
        .into_iter()
        .map(|opt| opt.unwrap_or_else(|| Err(io::Error::other("worker failed to produce hash"))))
        .collect()
}

fn flush_pending(
    store: &mut DiskStore,
    cache: &mut ValidationCache,
    ingested: &mut Ingested,
    pending: &mut Vec<PendingFile>,
) -> Result<()> {
    if pending.is_empty() {
        return Ok(());
    }

    let results = hash_pending_files(pending);
    let mut copy_buf = vec![0u8; 64 * 1024];

    for (file, hash_res) in pending.drain(..).zip(results) {
        let id = hash_res.map_err(|e| io_ctx("read", &file.path, e))?;
        store.put_file_with_id_buf(&file.path, &id, &mut copy_buf)?;
        if file.mtime >= UNIX_EPOCH && file.ctime >= UNIX_EPOCH {
            cache.record(
                file.rel.clone(),
                Entry {
                    size: file.size,
                    mtime: file.mtime,
                    inode: file.inode,
                    ctime: file.ctime,
                    id,
                },
            );
        }
        ingested.file_sizes.insert(file.rel.clone(), file.size);
        ingested.files.insert(file.rel, id);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn process_file_entry(
    store: &mut DiskStore,
    cache: &mut ValidationCache,
    ingested: &mut Ingested,
    pending: &mut Vec<PendingFile>,
    rel: String,
    path: PathBuf,
    size: u64,
    mtime: std::time::SystemTime,
    inode: u64,
    ctime: std::time::SystemTime,
    mode: u32,
) -> Result<()> {
    ingested.modes.insert(rel.clone(), mode & 0o7777);
    match cache.lookup(&rel, size, mtime, inode, ctime) {
        Some(id) if store.contains(&id) => {
            ingested.file_sizes.insert(rel.clone(), size);
            ingested.files.insert(rel, id);
        }
        _ => {
            pending.push(PendingFile {
                rel,
                path,
                size,
                mtime,
                inode,
                ctime,
            });
            if pending.len() >= INGEST_BATCH_SIZE {
                flush_pending(store, cache, ingested, pending)?;
            }
        }
    }
    Ok(())
}

/// The portable read_dir+metadata walk: one `fs::metadata` per regular
/// file on top of each directory's readdir. Also the fallback for the
/// macOS bulk walker.
fn ingest_tree_walk(
    store: &mut DiskStore,
    cache: &mut ValidationCache,
    ingested: &mut Ingested,
    src_root: &Path,
    src: &Path,
    options: &IngestOptions<'_>,
) -> Result<()> {
    let snapshots = options.snapshots;
    let mut pending = Vec::new();
    let mut stack = vec![src.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let rel = rel_text(src_root, &dir)?;
        if (options.exclude)(&rel) {
            continue;
        }
        let entries = fs::read_dir(&dir).map_err(|e| io_ctx("read", &dir, e))?;
        // One extra stat per DIRECTORY (not per file): its permission
        // bits must survive hydration, since create_dir_all cannot
        // carry them through the umask.
        let dir_meta = fs::symlink_metadata(&dir).map_err(|e| io_ctx("stat", &dir, e))?;
        ingested
            .dir_modes
            .insert(rel.clone(), dir_meta.mode() & 0o7777);
        ingested.dirs.push(rel);
        for entry in entries.flatten() {
            let path = entry.path();
            let rel = rel_text(src_root, &path)?;
            if (options.exclude)(&rel) {
                continue;
            }
            let file_type = entry.file_type().map_err(|e| io_ctx("stat", &path, e))?;
            if file_type.is_symlink() {
                // The raw target is what gets recorded, dangling or
                // not: placement recreates it verbatim via symlink(2),
                // which never resolves the target. (With snapshots on
                // the manifest consumes the same record.)
                ingest_symlink(ingested, rel, &path)?;
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
                    return Err(io_ctx(
                        "ingest",
                        &path,
                        io::Error::other(
                            "not a regular file (fifos/sockets/devices are unsupported)",
                        ),
                    ));
                }
                continue;
            }
            let meta = fs::metadata(&path).map_err(|e| io_ctx("stat", &path, e))?;
            let size = meta.len();
            let mtime = meta.modified().map_err(|e| io_ctx("stat", &path, e))?;
            let inode = meta.ino();
            let ctime_secs = meta.ctime().max(0) as u64;
            let ctime_nanos = meta.ctime_nsec().clamp(0, 999_999_999) as u32;
            let ctime = UNIX_EPOCH + Duration::new(ctime_secs, ctime_nanos);
            let mode = meta.mode();
            process_file_entry(
                store,
                cache,
                ingested,
                &mut pending,
                rel,
                path,
                size,
                mtime,
                inode,
                ctime,
                mode,
            )?;
        }
    }
    flush_pending(store, cache, ingested, &mut pending)?;
    Ok(())
}

#[allow(clippy::too_many_arguments, dead_code)]
fn ingest_file(
    store: &mut DiskStore,
    cache: &mut ValidationCache,
    ingested: &mut Ingested,
    rel: String,
    path: &Path,
    size: u64,
    mtime: std::time::SystemTime,
    inode: u64,
    ctime: std::time::SystemTime,
    mode: u32,
) -> Result<()> {
    let mut pending = Vec::new();
    process_file_entry(
        store,
        cache,
        ingested,
        &mut pending,
        rel,
        path.to_path_buf(),
        size,
        mtime,
        inode,
        ctime,
        mode,
    )?;
    flush_pending(store, cache, ingested, &mut pending)
}

fn ingest_symlink(ingested: &mut Ingested, rel: String, path: &Path) -> Result<()> {
    let target = fs::read_link(path).map_err(|e| io_ctx("read symlink", path, e))?;
    ingested
        .symlinks
        .insert(rel, target.to_string_lossy().into_owned());
    Ok(())
}

/// Source-relative text of `path` under `root`, or a loud error when a
/// pattern somehow matched outside the ingestion root.
fn rel_text(root: &Path, path: &Path) -> Result<String> {
    path.strip_prefix(root)
        .map_err(|_| {
            io_ctx(
                "ingest",
                path,
                io::Error::other("pattern matched path outside the ingestion root"),
            )
        })
        .map(|p| p.to_string_lossy().into_owned())
}

/// Wrap an io failure with the operation and the path it hit.
fn io_ctx(op: &str, path: &Path, e: io::Error) -> Error {
    Error::Io(io::Error::other(format!("{op} {}: {e}", path.display())))
}
