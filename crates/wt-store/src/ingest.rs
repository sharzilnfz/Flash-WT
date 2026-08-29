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
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
#[cfg(target_os = "macos")]
use std::time::{Duration, UNIX_EPOCH};

#[cfg(target_os = "macos")]
use crate::bulkwalk;
use crate::validation::{Entry, ValidationCache};
use crate::{ContentId, DiskStore, Error, Result, Store};

/// Everything one ingested tree contributed to the store.
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
                        let target =
                            fs::read_link(&path).map_err(|e| io_ctx("read symlink", &path, e))?;
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
                    if options.snapshots {
                        ingested.modes.insert(rel.clone(), entry.mode & 0o7777);
                    }
                    let mtime = UNIX_EPOCH + Duration::new(entry.mtime_secs, entry.mtime_nanos);
                    let id = match cache.lookup(&rel, entry.size, mtime) {
                        Some(id) if self.contains(&id) => id,
                        _ => {
                            let bytes = fs::read(&path).map_err(|e| io_ctx("read", &path, e))?;
                            let id = self.put(&bytes)?;
                            cache.record(
                                rel.clone(),
                                Entry {
                                    size: entry.size,
                                    mtime,
                                    id,
                                },
                            );
                            id
                        }
                    };
                    ingested.file_sizes.insert(rel.clone(), entry.size);
                    ingested.files.insert(rel, id);
                }
            }
            None => ingest_tree_walk(self, &mut cache, &mut ingested, src_root, src, options)?,
        }

        ingested.dirs.sort();
        ingested.dirs.dedup();
        cache.save().map_err(Error::Io)?;
        Ok(ingested)
    }
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
                let target = fs::read_link(&path).map_err(|e| io_ctx("read symlink", &path, e))?;
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
            let rel = rel_text(src_root, &path)?;
            let meta = fs::metadata(&path).map_err(|e| io_ctx("stat", &path, e))?;
            ingested.modes.insert(rel.clone(), meta.mode() & 0o7777);
            let size = meta.len();
            let mtime = meta.modified().map_err(|e| io_ctx("stat", &path, e))?;
            let id = match cache.lookup(&rel, size, mtime) {
                // Cache hit: same size and same mtime as last time.
                // Trust it only while the blob is actually still here.
                Some(id) if store.contains(&id) => id,
                _ => {
                    // Miss (or a swept blob): pay for read and hash.
                    let bytes = fs::read(&path).map_err(|e| io_ctx("read", &path, e))?;
                    let id = store.put(&bytes)?;
                    // An mtime before the epoch cannot round-trip
                    // through the cache format; skip caching rather
                    // than fail, so such a file just stays cold.
                    if mtime >= std::time::UNIX_EPOCH {
                        cache.record(rel.clone(), Entry { size, mtime, id });
                    }
                    id
                }
            };
            ingested.file_sizes.insert(rel.clone(), size);
            ingested.files.insert(rel, id);
        }
    }
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
