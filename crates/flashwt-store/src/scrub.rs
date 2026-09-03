use std::fs;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::snapshot::{paranoid_verify_tree, read_published as read_published_snapshot};
use crate::{ContentId, DiskStore, Error, Result};

fn verify_snapshot_dir(dir: &Path, name: &str) -> std::result::Result<(), String> {
    let Some(hash) = ContentId::from_hex(name) else {
        return Err("directory name is not a valid 64-hex content hash".to_string());
    };
    let root = dir
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| "missing store root for snapshot directory".to_string())?;
    let manifest = read_published_snapshot(root, &hash)
        .ok_or_else(|| "invalid published snapshot metadata or marker".to_string())?;
    let tree_dir = dir.join("tree");
    paranoid_verify_tree(&tree_dir, &manifest).map_err(|e| e.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScrubReport {
    pub scanned: u64,

    pub corrupt: Vec<ContentId>,

    pub deleted: u64,

    pub snapshot_dirs_scanned: u64,

    pub corrupt_snapshots: Vec<String>,

    pub snapshot_dirs_deleted: u64,
}

impl DiskStore {
    pub fn scrub(&mut self, dry_run: bool) -> Result<ScrubReport> {
        let objects_dir = self.root().join("objects");
        let num_cpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        let num_workers = num_cpus.clamp(1, 256);

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
                        let shard_id = shard_index.fetch_add(1, Ordering::Relaxed);
                        if shard_id >= 256 {
                            break;
                        }
                        let shard_hex = format!("{shard_id:02x}");
                        let shard_dir = objects_dir.join(&shard_hex);
                        if !shard_dir.is_dir() {
                            continue;
                        }

                        let entries = match fs::read_dir(&shard_dir) {
                            Ok(e) => e,
                            Err(e) => {
                                let mut slot = err_slot.lock().unwrap_or_else(|p| p.into_inner());
                                if slot.is_none() {
                                    *slot = Some(Error::Io(e));
                                }
                                break;
                            }
                        };

                        for entry in entries.flatten() {
                            if err_slot.lock().unwrap_or_else(|p| p.into_inner()).is_some() {
                                break;
                            }
                            let Ok(ft) = entry.file_type() else {
                                continue;
                            };
                            if !ft.is_file() {
                                continue;
                            }
                            let name = entry.file_name();
                            let Some(name_str) = name.to_str() else {
                                continue;
                            };
                            let full_hex = format!("{shard_hex}{name_str}");
                            let Some(id) = ContentId::from_hex(&full_hex) else {
                                continue;
                            };

                            worker_scanned += 1;
                            let path = entry.path();
                            match DiskStore::verify_file(&path, &id) {
                                Ok(()) => {}
                                Err(Error::Corrupted(_)) => {
                                    worker_corrupt.push(id);
                                }
                                Err(Error::UnknownContent(_)) => {}
                                Err(Error::Io(e)) => {
                                    let mut slot =
                                        err_slot.lock().unwrap_or_else(|p| p.into_inner());
                                    if slot.is_none() {
                                        *slot = Some(Error::Io(e));
                                    }
                                    break;
                                }
                                Err(e) => {
                                    let mut slot =
                                        err_slot.lock().unwrap_or_else(|p| p.into_inner());
                                    if slot.is_none() {
                                        *slot = Some(e);
                                    }
                                    break;
                                }
                            }
                        }
                    }

                    if worker_scanned > 0 {
                        total_scanned.fetch_add(worker_scanned, Ordering::Relaxed);
                    }
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

        let mut snapshot_candidates = Vec::new();
        let snapshots_dir = self.root().join("snapshots");
        if snapshots_dir.is_dir() {
            if let Ok(entries) = fs::read_dir(&snapshots_dir) {
                for entry in entries.flatten() {
                    let Ok(ft) = entry.file_type() else {
                        continue;
                    };
                    if !ft.is_dir() {
                        continue;
                    }
                    let name = entry.file_name();
                    let Some(name_str) = name.to_str() else {
                        continue;
                    };
                    if name_str == "tmp" || name_str == "worktrees" {
                        continue;
                    }
                    snapshot_candidates.push((name_str.to_string(), entry.path()));
                }
            }
        }

        let snapshot_scanned_count = snapshot_candidates.len() as u64;
        let corrupt_snapshots = Mutex::new(Vec::new());

        if !snapshot_candidates.is_empty() {
            let snap_index = AtomicUsize::new(0);
            std::thread::scope(|s| {
                for _ in 0..num_workers {
                    s.spawn(|| {
                        let mut worker_corrupt_snaps = Vec::new();
                        loop {
                            let idx = snap_index.fetch_add(1, Ordering::Relaxed);
                            if idx >= snapshot_candidates.len() {
                                break;
                            }
                            let (name, path) = &snapshot_candidates[idx];
                            if verify_snapshot_dir(path, name).is_err() {
                                worker_corrupt_snaps.push(name.clone());
                            }
                        }
                        if !worker_corrupt_snaps.is_empty() {
                            let mut guard =
                                corrupt_snapshots.lock().unwrap_or_else(|p| p.into_inner());
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
            let _lock = self.lock_refs()?;
            for id in &corrupt {
                self.delete(id)?;
                deleted += 1;
            }
        }

        self.flush()?;

        let mut snapshot_dirs_deleted = 0u64;
        if !dry_run && !corrupt_snaps.is_empty() {
            for snap_name in &corrupt_snaps {
                let snap_path = snapshots_dir.join(snap_name);
                if snap_path.exists() && fs::remove_dir_all(&snap_path).is_ok() {
                    snapshot_dirs_deleted += 1;
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

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::Manifest;
    use crate::SnapshotEntry;
    use crate::snapshot::PublishOptions;

    #[test]
    fn sharded_parallel_scrub_verifies_blobs_across_multiple_shards() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = DiskStore::open(temp.path()).unwrap();

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

        let dry_report = store.scrub(true).unwrap();
        assert_eq!(dry_report.scanned, 60);
        assert_eq!(dry_report.corrupt.len(), 2);
        assert_eq!(dry_report.deleted, 0);
        assert!(dry_report.corrupt.contains(&id_to_tamper));
        assert!(dry_report.corrupt.contains(&id_to_tamper2));
        assert!(blob_path.exists());
        assert!(blob_path2.exists());

        let real_report = store.scrub(false).unwrap();
        assert_eq!(real_report.scanned, 60);
        assert_eq!(real_report.corrupt.len(), 2);
        assert_eq!(real_report.deleted, 2);
        assert!(!blob_path.exists());
        assert!(!blob_path2.exists());

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
        store
            .publish_snapshot(entries, PublishOptions::default())
            .unwrap();
        let snap_dir = store.root().join("snapshots").join(snap_hash.to_string());
        assert!(snap_dir.is_dir());

        let report = store.scrub(false).unwrap();
        assert_eq!(report.snapshot_dirs_scanned, 1);
        assert_eq!(report.corrupt_snapshots.len(), 0);
        assert_eq!(report.snapshot_dirs_deleted, 0);

        fs::write(snap_dir.join(".complete"), "invalid-marker").unwrap();
        let dry = store.scrub(true).unwrap();
        assert_eq!(dry.snapshot_dirs_scanned, 1);
        assert_eq!(dry.corrupt_snapshots, vec![snap_hash.to_string()]);
        assert_eq!(dry.snapshot_dirs_deleted, 0);
        assert!(snap_dir.exists());

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
        let entries = vec![SnapshotEntry::file("file.txt", b1, 0o644)];

        let manifest = Manifest::new(entries.clone()).unwrap();
        let snap_hash = manifest.hash;
        store
            .publish_snapshot(entries, PublishOptions::default())
            .unwrap();
        let snap_dir = store.root().join("snapshots").join(snap_hash.to_string());
        assert!(snap_dir.is_dir());

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
