//! Snapshot staging-tree construction: placing manifest entries into
//! a temp directory (verify-first, hardlink, chmod, symlink), plus
//! the paranoid whole-tree proof pass used before an incremental
//! publish is allowed to rename.
//!
//! Timing note: placement gathers its internal phase timings into a
//! [`TreeTimings`] accumulator passed by `&mut` instead of juggling
//! one accumulator parameter per bucket — observation only, never a
//! behavior input.

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Instant;

use super::manifest::{EntryKind, Manifest, SnapshotEntry};
use super::publish::BuildError;
use crate::DiskStore;
use crate::Error;

/// Internal phase timings of the tree-construction phase.
/// Milliseconds, best-effort.
#[derive(Debug, Default)]
pub(super) struct TreeTimings {
    /// Blob verification (hash/stat) before linking into staging.
    pub(super) verify_ms: u64,
    /// Staging-tree construction: mkdirs, hardlinks, chmods, symlinks.
    pub(super) link_train_ms: u64,
}

/// Recursive `clonefile(2)` of `src` onto (not-yet-existing) `dst`
/// — the v2 whole-tree copy primitive. APFS-only; other platforms
/// report [`io::ErrorKind::Unsupported`] so the incremental attempt
/// aborts and callers take a full build.
pub(super) fn clone_dir_recursive(src: &Path, dst: &Path) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt as _;
        let c_src = CString::new(src.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in source path"))?;
        let c_dst = CString::new(dst.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in dest path"))?;
        // SAFETY: both pointers are valid NUL-terminated C strings for
        // the duration of the call; clonefile keeps neither.
        let rc = unsafe { libc::clonefile(c_src.as_ptr(), c_dst.as_ptr(), 0) };
        if rc == 0 {
            return Ok(());
        }
        Err(io::Error::last_os_error())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (src, dst);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "recursive clonefile exists only on macOS/APFS",
        ))
    }
}

/// Every relpath under `dir` (files, symlinks, AND directories —
/// empty dirs matter), sorted. For the paranoid structural pass.
fn collect_rels(dir: &Path, prefix: &str, out: &mut Vec<String>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = format!("{prefix}{name}");
        out.push(rel.clone());
        if file_type.is_dir() {
            collect_rels(&entry.path(), &format!("{rel}/"), out)?;
        }
    }
    Ok(())
}

/// Why a validated-entry invariant failed where validation should
/// have caught it long before placement.
fn malformed_entry(entry: &SnapshotEntry, problem: &str) -> BuildError {
    BuildError::Fatal(format!(
        "malformed {:?} entry {:?}: {problem} (validation should have caught this)",
        entry.kind, entry.rel
    ))
}

/// Paranoid proof pass over a fully staged incremental tree: the path
/// set must match the manifest EXACTLY (no strays, no omissions),
/// every file must read-and-hash to its blob id, and every symlink
/// must carry its recorded target. Covers bulk-cloned and freshly-
/// linked content alike — see the trust model on
/// [`DiskStore::publish_snapshot_incremental`].
pub(super) fn paranoid_verify_tree(tree_dir: &Path, manifest: &Manifest) -> Result<(), BuildError> {
    let mut got = Vec::new();
    if let Err(e) = collect_rels(tree_dir, "", &mut got) {
        return Err(BuildError::Fatal(format!(
            "paranoid check cannot walk staged tree: {e}"
        )));
    }
    got.sort();
    let want: Vec<String> = manifest.entries.iter().map(|e| e.rel.clone()).collect();
    if got != want {
        return Err(BuildError::Fatal(
            "paranoid check failed: staged tree paths differ from the manifest".into(),
        ));
    }
    for entry in &manifest.entries {
        match entry.kind {
            EntryKind::Dir => {}
            EntryKind::Symlink => {
                let Some(target) = &entry.target else {
                    return Err(malformed_entry(entry, "symlink entry lacks a target"));
                };
                let actual = match fs::read_link(tree_dir.join(&entry.rel)) {
                    Ok(t) => t.to_string_lossy().into_owned(),
                    Err(e) => {
                        return Err(BuildError::Fatal(format!(
                            "paranoid check cannot read staged symlink {}: {e}",
                            entry.rel
                        )));
                    }
                };
                if actual != target.as_str() {
                    return Err(BuildError::Fatal(format!(
                        "paranoid check failed: staged symlink {} points to {actual:?}, \
                         manifest says {target:?}",
                        entry.rel
                    )));
                }
            }
            EntryKind::File => {
                let Some(blob) = entry.blob else {
                    return Err(malformed_entry(entry, "file entry lacks a blob ref"));
                };
                // Streaming read-and-hash of the STAGED copy (a
                // hardlink to the blob, so same inode): constant
                // memory no matter how big the file.
                if let Err(e) = DiskStore::verify_file(&tree_dir.join(&entry.rel), &blob) {
                    return Err(match e {
                        Error::UnknownContent(_) => BuildError::Fatal(format!(
                            "paranoid check cannot read staged file {}: it is missing",
                            entry.rel
                        )),
                        Error::Io(io) => BuildError::Fatal(format!(
                            "paranoid check cannot read staged file {}: {io}",
                            entry.rel
                        )),
                        _ => BuildError::Fatal(format!(
                            "paranoid check failed: staged file {} does not hash to its blob {}",
                            entry.rel, blob
                        )),
                    });
                }
            }
        }
    }
    Ok(())
}

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use super::manifest::DIR_MODE;

/// RAII guard for an open directory handle (`libc::open` / `libc::close`).
struct DirFd(libc::c_int);

impl DirFd {
    fn open(path: &Path) -> Result<Self, io::Error> {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let c_path = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in path"))?;
        let fd = unsafe {
            libc::open(
                c_path.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(fd))
    }

    fn raw(&self) -> libc::c_int {
        self.0
    }
}

impl Drop for DirFd {
    fn drop(&mut self) {
        if self.0 >= 0 {
            unsafe {
                libc::close(self.0);
            }
        }
    }
}

fn split_parent_and_filename(rel: &str) -> (&str, &str) {
    if let Some(pos) = rel.rfind('/') {
        (&rel[..pos], &rel[pos + 1..])
    } else {
        ("", rel)
    }
}

struct DirectoryGroup<'a> {
    parent_rel: &'a str,
    entries: Vec<(&'a SnapshotEntry, &'a str)>,
}

struct SubtreeBatch<'a> {
    dir_groups: Vec<DirectoryGroup<'a>>,
}

const TARGET_BATCH_SIZE: usize = 1000;

impl DiskStore {
    /// Materialize the manifest inside `dir` using two distinct phases:
    /// Phase 1 builds directory entries sequentially in sorted order to
    /// establish directory skeletons without lock contention.
    /// Phase 2 groups file and symlink entries by parent directory into
    /// subtree batches and dispatches them across a worker pool bounded by
    /// available CPU parallelism via `std::thread::scope`.
    ///
    /// Chmod on a hardlink retargets the SHARED inode, so the object
    /// blob's mode becomes the snapshot's normalized mode. That is
    /// deliberate (the plan's "blobs' stored modes preserve exec
    /// bits") and safe here: normalization only ever sets owner-write
    /// or adds/removes the x-bits consistently for one content id, and
    /// chmod does not touch mtime, so verified-ledger fingerprints
    /// stay valid.
    pub(super) fn build_tree(
        &self,
        dir: &Path,
        manifest: &Manifest,
        paranoid: bool,
        timings: &mut TreeTimings,
    ) -> Result<(), BuildError> {
        // Phase 1: Directory skeleton creation runs sequentially in sorted order
        // before file linking starts.
        let stage = Instant::now();
        let mut dir_modes: BTreeMap<String, u32> = BTreeMap::new();
        for entry in &manifest.entries {
            if entry.kind == EntryKind::Dir {
                dir_modes.insert(entry.rel.clone(), entry.mode);
            }
            // Ensure intermediate parent directories are also included
            let mut current = entry.rel.as_str();
            while let Some(pos) = current.rfind('/') {
                current = &current[..pos];
                dir_modes.entry(current.to_string()).or_insert(DIR_MODE);
            }
        }

        for (rel, mode) in &dir_modes {
            let dest = dir.join(rel);
            fs::create_dir_all(&dest)
                .and_then(|()| {
                    fs::set_permissions(&dest, fs::Permissions::from_mode(*mode))
                })
                .map_err(|e| {
                    BuildError::Fatal(format!("cannot create {}: {e}", dest.display()))
                })?;
        }
        timings.link_train_ms += stage.elapsed().as_millis() as u64;

        // Phase 2: Group file and symlink entries by parent directory into subtree batches
        // and dispatch them across scoped worker threads using relative openat operations.
        let mut parent_groups: BTreeMap<&str, Vec<(&SnapshotEntry, &str)>> = BTreeMap::new();
        for entry in &manifest.entries {
            if entry.kind != EntryKind::Dir {
                let (parent, filename) = split_parent_and_filename(&entry.rel);
                parent_groups.entry(parent).or_default().push((entry, filename));
            }
        }

        if parent_groups.is_empty() {
            return Ok(());
        }

        let mut batches: Vec<SubtreeBatch<'_>> = Vec::new();
        let mut current_batch: Vec<DirectoryGroup<'_>> = Vec::new();
        let mut current_batch_files: usize = 0;

        for (parent_rel, entries) in parent_groups {
            let count = entries.len();
            if current_batch_files >= TARGET_BATCH_SIZE && !current_batch.is_empty() {
                batches.push(SubtreeBatch {
                    dir_groups: std::mem::take(&mut current_batch),
                });
                current_batch_files = 0;
            }
            current_batch_files += count;
            current_batch.push(DirectoryGroup {
                parent_rel,
                entries,
            });
        }
        if !current_batch.is_empty() {
            batches.push(SubtreeBatch {
                dir_groups: current_batch,
            });
        }

        let num_cpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        let num_workers = num_cpus.min(batches.len()).max(1);

        let batch_index = AtomicUsize::new(0);
        let total_verify_ms = AtomicU64::new(0);
        let total_link_train_ms = AtomicU64::new(0);
        let err_slot = Mutex::new(None);

        std::thread::scope(|s| {
            for _ in 0..num_workers {
                s.spawn(|| {
                    let mut worker_verify_ms = 0u64;
                    let mut worker_link_train_ms = 0u64;

                    loop {
                        if err_slot.lock().unwrap_or_else(|p| p.into_inner()).is_some() {
                            break;
                        }

                        let idx = batch_index.fetch_add(1, Ordering::Relaxed);
                        if idx >= batches.len() {
                            break;
                        }

                        let batch = &batches[idx];
                        if let Err(e) = self.process_batch(
                            dir,
                            batch,
                            paranoid,
                            &mut worker_verify_ms,
                            &mut worker_link_train_ms,
                        ) {
                            let mut slot = err_slot.lock().unwrap_or_else(|p| p.into_inner());
                            if slot.is_none() {
                                *slot = Some(e);
                            }
                            break;
                        }
                    }

                    total_verify_ms.fetch_add(worker_verify_ms, Ordering::Relaxed);
                    total_link_train_ms.fetch_add(worker_link_train_ms, Ordering::Relaxed);
                });
            }
        });

        if let Some(err) = err_slot.into_inner().unwrap_or_default() {
            return Err(err);
        }

        timings.verify_ms += total_verify_ms.load(Ordering::Relaxed);
        timings.link_train_ms += total_link_train_ms.load(Ordering::Relaxed);

        Ok(())
    }

    fn process_batch(
        &self,
        base_dir: &Path,
        batch: &SubtreeBatch<'_>,
        paranoid: bool,
        verify_ms: &mut u64,
        link_train_ms: &mut u64,
    ) -> Result<(), BuildError> {
        for group in &batch.dir_groups {
            let dir_path = if group.parent_rel.is_empty() {
                base_dir.to_path_buf()
            } else {
                base_dir.join(group.parent_rel)
            };

            let stage = Instant::now();
            let dir_fd = DirFd::open(&dir_path).map_err(|e| {
                BuildError::Fatal(format!("cannot open dir {}: {e}", dir_path.display()))
            })?;
            *link_train_ms += stage.elapsed().as_millis() as u64;

            for (entry, filename) in &group.entries {
                self.place_entry_relative(
                    dir_fd.raw(),
                    &dir_path,
                    filename,
                    entry,
                    paranoid,
                    verify_ms,
                    link_train_ms,
                )?;
            }
            // dir_fd drops here eagerly when moving to the next parent dir.
        }
        Ok(())
    }

    fn place_entry_relative(
        &self,
        dir_fd: libc::c_int,
        dir_path: &Path,
        filename: &str,
        entry: &SnapshotEntry,
        paranoid: bool,
        verify_ms: &mut u64,
        link_train_ms: &mut u64,
    ) -> Result<(), BuildError> {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let c_filename = CString::new(filename.as_bytes())
            .map_err(|_| malformed_entry(entry, "NUL in filename"))?;

        match entry.kind {
            EntryKind::Dir => Ok(()),
            EntryKind::Symlink => {
                let Some(target) = &entry.target else {
                    return Err(malformed_entry(entry, "symlink entry lacks a target"));
                };
                let c_target = CString::new(target.as_bytes())
                    .map_err(|_| malformed_entry(entry, "NUL in symlink target"))?;
                let stage = Instant::now();
                let rc =
                    unsafe { libc::symlinkat(c_target.as_ptr(), dir_fd, c_filename.as_ptr()) };
                if rc != 0 {
                    let err = io::Error::last_os_error();
                    return Err(BuildError::Fatal(format!(
                        "cannot link symlink {}/{filename}: {err}",
                        dir_path.display()
                    )));
                }
                *link_train_ms += stage.elapsed().as_millis() as u64;
                Ok(())
            }
            EntryKind::File => {
                let Some(blob) = entry.blob else {
                    return Err(malformed_entry(entry, "file entry lacks a blob ref"));
                };

                let stage = Instant::now();
                let verdict = if paranoid {
                    self.verify_digest(&blob)
                } else {
                    self.ensure_verified(&blob)
                };
                if let Err(e) = verdict {
                    return Err(match e {
                        Error::UnknownContent(_) => BuildError::MissingBlob(blob),
                        other => BuildError::Fatal(other.to_string()),
                    });
                }
                *verify_ms += stage.elapsed().as_millis() as u64;

                let blob_path = self.blob_path(&blob);
                let c_blob = CString::new(blob_path.as_os_str().as_bytes())
                    .map_err(|_| BuildError::Fatal("NUL in blob path".into()))?;

                let stage = Instant::now();
                let rc = unsafe {
                    libc::linkat(
                        libc::AT_FDCWD,
                        c_blob.as_ptr(),
                        dir_fd,
                        c_filename.as_ptr(),
                        0,
                    )
                };
                if rc != 0 {
                    let err = io::Error::last_os_error();
                    return Err(match err.kind() {
                        io::ErrorKind::NotFound => BuildError::MissingBlob(blob),
                        _ => BuildError::Fatal(format!(
                            "cannot link blob {blob} to {}/{filename}: {err}",
                            dir_path.display()
                        )),
                    });
                }

                let mut stat_buf = std::mem::MaybeUninit::<libc::stat>::uninit();
                let rc = unsafe {
                    libc::fstatat(
                        dir_fd,
                        c_filename.as_ptr(),
                        stat_buf.as_mut_ptr(),
                        libc::AT_SYMLINK_NOFOLLOW,
                    )
                };
                if rc != 0 {
                    let err = io::Error::last_os_error();
                    return Err(BuildError::Fatal(format!(
                        "cannot stat {}/{filename}: {err}",
                        dir_path.display()
                    )));
                }
                let stat = unsafe { stat_buf.assume_init() };
                if (stat.st_mode as u32) & 0o7777 != entry.mode {
                    let rc = unsafe {
                        libc::fchmodat(
                            dir_fd,
                            c_filename.as_ptr(),
                            entry.mode as libc::mode_t,
                            0,
                        )
                    };
                    if rc != 0 {
                        let err = io::Error::last_os_error();
                        return Err(BuildError::Fatal(format!(
                            "cannot chmod {}/{filename}: {err}",
                            dir_path.display()
                        )));
                    }
                }
                *link_train_ms += stage.elapsed().as_millis() as u64;
                Ok(())
            }
        }
    }

    /// Place ONE entry under `dir`. Shared by the full builder
    /// ([`Self::build_tree`]) and the v2 incremental rebuild (which
    /// calls it for added and content-modified entries, and for any
    /// manifest-required dir the delta left missing). Semantics are
    /// identical:
    /// verify-first policy (`paranoid` = full streaming read-and-hash,
    /// otherwise verified-ledger trust through
    /// [`DiskStore::ensure_verified`]), hardlink +
    /// skip-no-op-chmod for files, symlink recreation, mkdir + chmod
    /// for dirs. Missing blobs surface as [`BuildError::MissingBlob`]
    /// so callers can heal and retry once.
    pub(super) fn place_entry(
        &self,
        entry: &SnapshotEntry,
        dir: &Path,
        paranoid: bool,
        timings: &mut TreeTimings,
    ) -> Result<(), BuildError> {
        let dest = dir.join(&entry.rel);
        match entry.kind {
            EntryKind::Dir => {
                let stage = Instant::now();
                fs::create_dir_all(&dest)
                    .and_then(|()| {
                        fs::set_permissions(&dest, fs::Permissions::from_mode(entry.mode))
                    })
                    .map_err(|e| {
                        BuildError::Fatal(format!("cannot create {}: {e}", dest.display()))
                    })?;
                timings.link_train_ms += stage.elapsed().as_millis() as u64;
                Ok(())
            }
            EntryKind::Symlink => {
                let Some(target) = &entry.target else {
                    return Err(malformed_entry(entry, "symlink entry lacks a target"));
                };
                let stage = Instant::now();
                if let Some(parent) = dest.parent() {
                    if !parent.exists() {
                        fs::create_dir_all(parent).map_err(|e| {
                            BuildError::Fatal(format!("cannot create {}: {e}", parent.display()))
                        })?;
                    }
                }
                #[cfg(unix)]
                std::os::unix::fs::symlink(target, &dest).map_err(|e| {
                    BuildError::Fatal(format!("cannot link {}: {e}", dest.display()))
                })?;
                timings.link_train_ms += stage.elapsed().as_millis() as u64;
                Ok(())
            }
            EntryKind::File => {
                let Some(blob) = entry.blob else {
                    return Err(malformed_entry(entry, "file entry lacks a blob ref"));
                };
                // Sorted order puts explicit dir entries ahead of
                // their children, but a manifest is not obliged
                // to name every intermediate: recreate any gap.
                let stage = Instant::now();
                if let Some(parent) = dest.parent() {
                    if !parent.exists() {
                        fs::create_dir_all(parent).map_err(|e| {
                            BuildError::Fatal(format!("cannot create {}: {e}", parent.display()))
                        })?;
                    }
                }
                timings.link_train_ms += stage.elapsed().as_millis() as u64;
                // Verify first: a corrupt or missing blob never
                // reaches placement. Both checks STREAM the bytes
                // through a fixed-size window, so verification cost
                // is bounded regardless of blob size; paranoid runs
                // always re-hash, everyone else trusts the verified
                // ledger when it can answer.
                let stage = Instant::now();
                let verdict = if paranoid {
                    self.verify_digest(&blob)
                } else {
                    self.ensure_verified(&blob)
                };
                if let Err(e) = verdict {
                    return Err(match e {
                        Error::UnknownContent(_) => BuildError::MissingBlob(blob),
                        other => BuildError::Fatal(other.to_string()),
                    });
                }
                timings.verify_ms += stage.elapsed().as_millis() as u64;
                let stage = Instant::now();
                if let Err(e) = fs::hard_link(self.blob_path(&blob), &dest) {
                    return Err(match e.kind() {
                        io::ErrorKind::NotFound => BuildError::MissingBlob(blob),
                        _ => BuildError::Fatal(format!(
                            "cannot link blob {blob} to {}: {e}",
                            dest.display()
                        )),
                    });
                }
                // Chmod on a hardlink retargets the SHARED inode, so
                // this may rewrite the object blob's mode — but only
                // when it actually differs. Most blobs are born 0644
                // and most entries want 0644; skipping the no-op
                // chmod saves a measurable slice of build time at
                // 40k-file scale without changing any outcome.
                let meta = dest.symlink_metadata().map_err(|e| {
                    BuildError::Fatal(format!("cannot stat {}: {e}", dest.display()))
                })?;
                if meta.permissions().mode() & 0o7777 != entry.mode {
                    fs::set_permissions(&dest, fs::Permissions::from_mode(entry.mode)).map_err(
                        |e| BuildError::Fatal(format!("cannot chmod {}: {e}", dest.display())),
                    )?;
                }
                timings.link_train_ms += stage.elapsed().as_millis() as u64;
                Ok(())
            }
        }
    }
}
