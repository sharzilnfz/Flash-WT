//! Store scrubbing: the repair path for the trust model's documented
//! residual risk (fast-hydration ticket 05, product-handoff known
//! limitations). The verified-blob ledger trusts a blob while its
//! (size, mtime) fingerprint stays unchanged, so a bit flip that
//! preserves both slips past every warm run. A scrub pass closes that
//! gap by re-hashing EVERY blob against its own content address —
//! the one check the ledger exists to avoid paying, spent deliberately.
//!
//! A corrupt blob cannot be repaired: its true content is gone. It is
//! deleted outright (with its refcount file and verified-ledger
//! entry) because blobs are rebuildable cache data — the next ingest
//! from any surviving checkout re-stores them — while anything still
//! referencing the address must fail LOUDLY (`Error::UnknownContent`)
//! rather than serve bad bytes. `--dry-run` reports without touching.
//!
//! Concurrency: the hash pass holds no lock — blobs are immutable by
//! convention and read via atomic opens, so a concurrent process
//  either sees a complete blob or none. Deletion takes the same
//! exclusive `flock(2)` on the `refs/` directory that every refcount
//! read-modify-write takes, so a concurrent `add_ref`/`release_ref`
//! cannot interleave with the corrupt entry's ref-file removal. Object
//! removal itself is a single `unlink(2)`: a concurrent reader racing
//! the delete gets the file or `NotFound`, never torn bytes.

use std::fs;
use std::io;
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::snapshot::{EntryKind, Manifest};
use crate::{ContentId, DiskStore, Error, Result};

/// Held `flock(2)` on the store's `refs/` directory for the lifetime
/// of the guard; released on drop. Same exclusion semantics as the
/// refcount lock in [`crate::disk`] — locks are per open file
/// description, so separate processes genuinely contend — reimplemented
/// here rather than exported, keeping the disk module's surface
/// untouched.
struct RefsDirLock {
    _file: fs::File,
}

impl Drop for RefsDirLock {
    fn drop(&mut self) {
        // SAFETY: the fd is valid for as long as `_file` is alive,
        // i.e. through the end of this drop.
        unsafe { libc::flock(self._file.as_raw_fd(), libc::LOCK_UN) };
    }
}

/// Exclusive-lock the `refs/` directory, serializing this pass's
/// refcount-affecting deletions against every other wt process.
fn lock_refs(root: &Path) -> io::Result<RefsDirLock> {
    let dir = fs::File::open(root.join("refs"))?;
    // SAFETY: flock(2) takes only an fd and constants; the fd is
    // valid for as long as `dir` is alive.
    if unsafe { libc::flock(dir.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(RefsDirLock { _file: dir })
}

/// Recursively collect relative paths under `dir` (files, symlinks, and directories).
fn collect_tree_rels(dir: &Path, prefix: &str, out: &mut Vec<String>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = format!("{prefix}{name}");
        out.push(rel.clone());
        if file_type.is_dir() {
            collect_tree_rels(&entry.path(), &format!("{rel}/"), out)?;
        }
    }
    Ok(())
}

/// Verify published snapshot directory: manifest validity, .complete marker, and file tree.
fn verify_snapshot_dir(dir: &Path, name: &str) -> std::result::Result<(), String> {
    let Some(hash) = ContentId::from_hex(name) else {
        return Err("directory name is not a valid 64-hex content hash".to_string());
    };

    let manifest_path = dir.join("manifest.tsv");
    let manifest_text = fs::read_to_string(&manifest_path)
        .map_err(|e| format!("cannot read manifest.tsv: {e}"))?;
    let manifest = Manifest::parse(&manifest_text)
        .map_err(|reason| format!("unparseable manifest: {reason}"))?;
    if manifest.hash != hash {
        return Err("manifest hash does not match directory name".to_string());
    }

    let complete_path = dir.join(".complete");
    let complete_text = fs::read_to_string(&complete_path)
        .map_err(|e| format!("missing or unreadable .complete marker: {e}"))?;
    let mut parts = complete_text.trim_end_matches('\n').split('\t');
    if parts.next() != Some("v1") {
        return Err("wrong schema version in .complete".to_string());
    }
    let expected_hash = hash.to_string();
    if parts.next() != Some(expected_hash.as_str()) || parts.next().is_some() {
        return Err(".complete does not match manifest hash".to_string());
    }

    let tree_dir = dir.join("tree");
    if !tree_dir.is_dir() {
        return Err("missing tree directory".to_string());
    }

    let mut got_rels = Vec::new();
    collect_tree_rels(&tree_dir, "", &mut got_rels)
        .map_err(|e| format!("cannot walk tree directory: {e}"))?;
    got_rels.sort();

    let mut want_rels: Vec<String> = manifest.entries.iter().map(|e| e.rel.clone()).collect();
    want_rels.sort();
    if got_rels != want_rels {
        return Err("tree paths differ from manifest".to_string());
    }

    for entry in &manifest.entries {
        let entry_path = tree_dir.join(&entry.rel);
        match entry.kind {
            EntryKind::Dir => {
                if !entry_path.is_dir() {
                    return Err(format!("missing tree directory {}", entry.rel));
                }
            }
            EntryKind::Symlink => {
                let Some(target) = &entry.target else {
                    return Err(format!("symlink entry {} lacks target", entry.rel));
                };
                let actual = fs::read_link(&entry_path)
                    .map_err(|e| format!("cannot read symlink {}: {e}", entry.rel))?;
                if actual.to_string_lossy() != target.as_str() {
                    return Err(format!(
                        "symlink {} points to {:?}, manifest says {:?}",
                        entry.rel, actual, target
                    ));
                }
            }
            EntryKind::File => {
                let Some(blob) = entry.blob else {
                    return Err(format!("file entry {} lacks blob reference", entry.rel));
                };
                if !entry_path.is_file() {
                    return Err(format!("missing tree file {}", entry.rel));
                }
                if let Err(e) = DiskStore::verify_file(&entry_path, &blob) {
                    return Err(format!("tree file {} failed verification: {e}", entry.rel));
                }
            }
        }
    }

    Ok(())
}

/// What one scrub pass observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScrubReport {
    /// Blobs the pass streamed through SHA-256.
    pub scanned: u64,
    /// Blobs whose bytes no longer match their content address,
    /// sorted (the enumeration order).
    pub corrupt: Vec<ContentId>,
    /// Corrupt blobs actually deleted; always zero for a dry run.
    pub deleted: u64,
    /// Published snapshot directories scanned.
    pub snapshot_dirs_scanned: u64,
    /// Broken or corrupted snapshot directory names / hashes, sorted.
    pub corrupt_snapshots: Vec<String>,
    /// Broken snapshot directories actually deleted; always zero for a dry run.
    pub snapshot_dirs_deleted: u64,
}

impl DiskStore {
    /// Re-hash every blob across 256 hash prefix shards in parallel worker threads,
    /// and verify published snapshot manifests, markers, and trees.
    ///
    /// With `dry_run` set, corrupt blobs and broken snapshots are only reported.
    /// Otherwise each corrupt blob is deleted outright (with refcount and ledger entries),
    /// and broken snapshot directories are removed.
    pub fn scrub(&mut self, dry_run: bool) -> Result<ScrubReport> {
        let objects_dir = self.root().join("objects");
        let num_cpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        let num_workers = num_cpus.min(256).max(1);

        let shard_index = AtomicUsize::new(0);
        let total_scanned = AtomicU64::new(0);
        let corrupt_blobs = Mutex::new(Vec::new());
        let err_slot = Mutex::new(None);

        std::thread::scope(|s| {
            for _ in 0..num_workers {
                s.spawn(|| {
                    let mut worker_scanned = 0u64;
                    let mut worker_corrupt = Vec::new();

                    loop {
                        if err_slot.lock().unwrap_or_else(|p| p.into_inner()).is_some() {
                            break;
                        }

                        let shard = shard_index.fetch_add(1, Ordering::Relaxed);
                        if shard >= 256 {
                            break;
                        }

                        let shard_hex = format!("{shard:02x}");
                        let shard_dir = objects_dir.join(&shard_hex);
                        if !shard_dir.is_dir() {
                            continue;
                        }

                        let entries = match fs::read_dir(&shard_dir) {
                            Ok(entries) => entries,
                            Err(e) => {
                                let mut slot = err_slot.lock().unwrap_or_else(|p| p.into_inner());
                                if slot.is_none() {
                                    *slot = Some(Error::Io(e));
                                }
                                break;
                            }
                        };

                        for entry in entries.flatten() {
                            let file_name = entry.file_name();
                            let file_name_str = file_name.to_string_lossy();
                            let hex = format!("{shard_hex}{file_name_str}");
                            let Some(id) = ContentId::from_hex(&hex) else {
                                continue;
                            };
                            let blob_path = entry.path();
                            worker_scanned += 1;
                            match DiskStore::verify_file(&blob_path, &id) {
                                Ok(()) => {}
                                Err(Error::Corrupted(_)) => {
                                    worker_corrupt.push(id);
                                }
                                Err(Error::UnknownContent(_)) => {
                                    // Blob vanished concurrently; skip.
                                }
                                Err(e) => {
                                    let mut slot = err_slot.lock().unwrap_or_else(|p| p.into_inner());
                                    if slot.is_none() {
                                        *slot = Some(e);
                                    }
                                    break;
                                }
                            }
                        }
                    }

                    total_scanned.fetch_add(worker_scanned, Ordering::Relaxed);
                    if !worker_corrupt.is_empty() {
                        let mut guard = corrupt_blobs.lock().unwrap_or_else(|p| p.into_inner());
                        guard.extend(worker_corrupt);
                    }
                });
            }
        });

        if let Some(err) = err_slot.into_inner().unwrap_or_default() {
            return Err(err);
        }

        let mut corrupt = corrupt_blobs.into_inner().unwrap_or_default();
        corrupt.sort();
        corrupt.dedup();

        // Published snapshot scrubbing
        let snapshots_dir = self.root().join("snapshots");
        let mut snapshot_candidates = Vec::new();
        if let Ok(entries) = fs::read_dir(&snapshots_dir) {
            for entry in entries.flatten() {
                let name_str = entry.file_name().to_string_lossy().into_owned();
                if ContentId::from_hex(&name_str).is_none() {
                    continue;
                }
                let path = entry.path();
                if path.is_dir() {
                    snapshot_candidates.push((name_str, path));
                }
            }
        }
        snapshot_candidates.sort_by(|a, b| a.0.cmp(&b.0));

        let snapshot_scanned_count = snapshot_candidates.len() as u64;
        let corrupt_snapshots = Mutex::new(Vec::new());

        if !snapshot_candidates.is_empty() {
            let snap_index = AtomicUsize::new(0);
            let snap_workers = num_cpus.min(snapshot_candidates.len()).max(1);

            std::thread::scope(|s| {
                for _ in 0..snap_workers {
                    s.spawn(|| {
                        let mut worker_corrupt_snaps = Vec::new();
                        loop {
                            let idx = snap_index.fetch_add(1, Ordering::Relaxed);
                            if idx >= snapshot_candidates.len() {
                                break;
                            }
                            let (name, path) = &snapshot_candidates[idx];
                            if let Err(_) = verify_snapshot_dir(path, name) {
                                worker_corrupt_snaps.push(name.clone());
                            }
                        }
                        if !worker_corrupt_snaps.is_empty() {
                            let mut guard = corrupt_snapshots.lock().unwrap_or_else(|p| p.into_inner());
                            guard.extend(worker_corrupt_snaps);
                        }
                    });
                }
            });
        }

        let mut corrupt_snaps = corrupt_snapshots.into_inner().unwrap_or_default();
        corrupt_snaps.sort();
        corrupt_snaps.dedup();

        let mut deleted = 0u64;
        if !dry_run && !corrupt.is_empty() {
            let _lock = lock_refs(self.root())?;
            for id in &corrupt {
                self.delete(id)?;
                deleted += 1;
            }
        }
        // Persist the ledger forgets now rather than at drop.
        self.flush()?;

        let mut snapshot_dirs_deleted = 0u64;
        if !dry_run && !corrupt_snaps.is_empty() {
            for snap_name in &corrupt_snaps {
                let snap_path = snapshots_dir.join(snap_name);
                if snap_path.exists() {
                    if fs::remove_dir_all(&snap_path).is_ok() {
                        snapshot_dirs_deleted += 1;
                    }
                }
            }
        }

        Ok(ScrubReport {
            scanned: total_scanned.load(Ordering::Relaxed),
            corrupt,
            deleted,
            snapshot_dirs_scanned: snapshot_scanned_count,
            corrupt_snapshots: corrupt_snaps,
            snapshot_dirs_deleted,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SnapshotEntry;
    use crate::Store;

    #[test]
    fn sharded_parallel_scrub_verifies_blobs_across_multiple_shards() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = DiskStore::open(temp.path()).unwrap();

        // Create blobs with distinct content to populate multiple shards
        let mut ids = Vec::new();
        for i in 0..60 {
            let content = format!("blob-content-{i}-{}", i * 1337);
            let id = store.put(content.as_bytes()).unwrap();
            ids.push(id);
        }

        let report = store.scrub(false).unwrap();
        assert_eq!(report.scanned, 60);
        assert_eq!(report.corrupt.len(), 0);
        assert_eq!(report.deleted, 0);

        // Tamper 2 blobs
        let id_to_tamper = ids[5];
        let blob_path = store.object_path(&id_to_tamper);
        let mut bytes = fs::read(&blob_path).unwrap();
        bytes[0] ^= 0xff;
        fs::write(&blob_path, bytes).unwrap();

        let id_to_tamper2 = ids[18];
        let blob_path2 = store.object_path(&id_to_tamper2);
        let mut bytes2 = fs::read(&blob_path2).unwrap();
        bytes2[0] ^= 0xff;
        fs::write(&blob_path2, bytes2).unwrap();

        // Dry run detects corruptions without deleting
        let dry_report = store.scrub(true).unwrap();
        assert_eq!(dry_report.scanned, 60);
        assert_eq!(dry_report.corrupt.len(), 2);
        assert_eq!(dry_report.deleted, 0);
        assert!(dry_report.corrupt.contains(&id_to_tamper));
        assert!(dry_report.corrupt.contains(&id_to_tamper2));
        assert!(blob_path.exists());
        assert!(blob_path2.exists());

        // Real run deletes corrupt blobs
        let real_report = store.scrub(false).unwrap();
        assert_eq!(real_report.scanned, 60);
        assert_eq!(real_report.corrupt.len(), 2);
        assert_eq!(real_report.deleted, 2);
        assert!(!blob_path.exists());
        assert!(!blob_path2.exists());

        // Subsequent scrub is clean
        let clean_report = store.scrub(false).unwrap();
        assert_eq!(clean_report.scanned, 58);
        assert_eq!(clean_report.corrupt.len(), 0);
        assert_eq!(clean_report.deleted, 0);
    }

    #[test]
    fn scrub_verifies_and_purges_corrupted_snapshot_manifest_and_marker() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = DiskStore::open(temp.path()).unwrap();

        let b1 = store.put(b"snap-file-1").unwrap();
        let b2 = store.put(b"snap-file-2").unwrap();
        let entries = vec![
            SnapshotEntry::file("f1.txt", b1, 0o644),
            SnapshotEntry::dir("sub"),
            SnapshotEntry::file("sub/f2.txt", b2, 0o644),
        ];

        let manifest = Manifest::new(entries.clone()).unwrap();
        let snap_hash = manifest.hash;
        store.publish_snapshot(entries, false).unwrap();
        let snap_dir = store.root().join("snapshots").join(snap_hash.to_string());
        assert!(snap_dir.is_dir());

        // Healthy scrub
        let report = store.scrub(false).unwrap();
        assert_eq!(report.snapshot_dirs_scanned, 1);
        assert_eq!(report.corrupt_snapshots.len(), 0);
        assert_eq!(report.snapshot_dirs_deleted, 0);

        // Tamper .complete marker
        fs::write(snap_dir.join(".complete"), "invalid-marker").unwrap();
        let dry = store.scrub(true).unwrap();
        assert_eq!(dry.snapshot_dirs_scanned, 1);
        assert_eq!(dry.corrupt_snapshots, vec![snap_hash.to_string()]);
        assert_eq!(dry.snapshot_dirs_deleted, 0);
        assert!(snap_dir.exists());

        // Purge on real scrub
        let real = store.scrub(false).unwrap();
        assert_eq!(real.snapshot_dirs_scanned, 1);
        assert_eq!(real.corrupt_snapshots, vec![snap_hash.to_string()]);
        assert_eq!(real.snapshot_dirs_deleted, 1);
        assert!(!snap_dir.exists());
    }

    #[test]
    fn scrub_verifies_and_purges_corrupted_snapshot_tree() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = DiskStore::open(temp.path()).unwrap();

        let b1 = store.put(b"snap-tree-1").unwrap();
        let entries = vec![
            SnapshotEntry::file("file.txt", b1, 0o644),
        ];

        let manifest = Manifest::new(entries.clone()).unwrap();
        let snap_hash = manifest.hash;
        store.publish_snapshot(entries, false).unwrap();
        let snap_dir = store.root().join("snapshots").join(snap_hash.to_string());
        assert!(snap_dir.is_dir());

        // Tamper file in tree
        let tree_file = snap_dir.join("tree").join("file.txt");
        fs::write(&tree_file, b"corrupted-bytes").unwrap();

        let dry = store.scrub(true).unwrap();
        assert_eq!(dry.corrupt_snapshots, vec![snap_hash.to_string()]);
        assert_eq!(dry.snapshot_dirs_deleted, 0);

        let real = store.scrub(false).unwrap();
        assert_eq!(real.corrupt_snapshots, vec![snap_hash.to_string()]);
        assert_eq!(real.snapshot_dirs_deleted, 1);
        assert!(!snap_dir.exists());
    }
}
