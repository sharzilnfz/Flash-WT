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

#[derive(Debug)]
pub struct Ingested {
    pub dirs: Vec<String>,

    pub dir_modes: BTreeMap<String, u32>,

    pub files: BTreeMap<String, ContentId>,

    pub file_sizes: BTreeMap<String, u64>,

    pub symlinks: BTreeMap<String, String>,

    pub modes: BTreeMap<String, u32>,
}

impl Ingested {
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

    pub fn into_snapshot_manifest(
        self,
        heavy_rel: &str,
        lockfile_hash: Option<ContentId>,
    ) -> std::result::Result<Manifest, String> {
        self.to_snapshot_manifest(heavy_rel, lockfile_hash)
    }
}

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

pub struct IngestOptions<'a> {
    pub snapshots: bool,

    pub exclude: &'a dyn Fn(&str) -> bool,
}

impl DiskStore {
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

        #[cfg(target_os = "macos")]
        let walked = if options.snapshots {
            bulkwalk::walk(src).ok()
        } else {
            None
        };

        #[cfg(not(target_os = "macos"))]
        let walked: Option<std::convert::Infallible> = None;

        match walked {
            #[cfg(target_os = "macos")]
            Some(entries) => {
                let mut pending = Vec::new();

                let base = rel_text(src_root, src)?;

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

    let mut batch = Vec::with_capacity(pending.len());
    for (file, hash_res) in pending.drain(..).zip(results) {
        let id = hash_res.map_err(|e| io_ctx("read", &file.path, e))?;
        batch.push((file, id));
    }

    let files_to_put: Vec<(&Path, ContentId)> = batch
        .iter()
        .map(|(file, id)| (file.path.as_path(), *id))
        .collect();

    store.put_files_batch_buf(&files_to_put, &mut copy_buf)?;

    for (file, id) in batch {
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
                ingest_symlink(ingested, rel, &path)?;
                continue;
            }
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            if !file_type.is_file() {
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

fn io_ctx(op: &str, path: &Path, e: io::Error) -> Error {
    Error::Io(io::Error::other(format!("{op} {}: {e}", path.display())))
}
