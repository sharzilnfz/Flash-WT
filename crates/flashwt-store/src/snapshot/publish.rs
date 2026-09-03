use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use super::manifest::{EntryKind, Manifest, SnapshotEntry};
use super::tree::{TreeTimings, clone_dir_recursive, paranoid_verify_tree};
use crate::snapdiff::SnapshotDiff;
use crate::{ContentId, DiskStore};

const TREE_SUBDIR: &str = "tree";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SnapshotBuildTiming {
    pub verify_ms: u64,

    pub link_train_ms: u64,

    pub publish_ms: u64,

    pub clone_units_ms: u64,

    pub clone_units: usize,

    pub linked_files: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublishReceipt {
    pub outcome: PublishOutcome,

    pub timing: SnapshotBuildTiming,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishOutcome {
    Published,

    WinnerValid,

    WinnerInvalid,
}

#[derive(Debug)]
pub enum BuildError {
    MissingBlob(ContentId),

    Fatal(String),

    Io(crate::Error),
}

impl From<String> for BuildError {
    fn from(e: String) -> Self {
        BuildError::Fatal(e)
    }
}

impl From<io::Error> for BuildError {
    fn from(e: io::Error) -> Self {
        BuildError::Io(crate::Error::Io(e))
    }
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildError::MissingBlob(id) => {
                write!(f, "content {id} vanished during snapshot build")
            }
            BuildError::Fatal(msg) => f.write_str(msg),
            BuildError::Io(crate::Error::Io(e)) => write!(f, "store io error: {e}"),
            BuildError::Io(other) => write!(f, "{other}"),
        }
    }
}

enum StageSeed {
    FreshTree,

    CloneTree,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PublishOptions {
    pub lockfile_hash: Option<ContentId>,

    pub base_snapshot: Option<ContentId>,

    pub paranoid: bool,
}

impl PublishOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn paranoid(mut self, paranoid: bool) -> Self {
        self.paranoid = paranoid;
        self
    }

    pub fn lockfile_hash(mut self, lockfile_hash: Option<ContentId>) -> Self {
        self.lockfile_hash = lockfile_hash;
        self
    }

    pub fn base_snapshot(mut self, base_snapshot: Option<ContentId>) -> Self {
        self.base_snapshot = base_snapshot;
        self
    }
}

impl DiskStore {
    pub fn snapshot_path(&self, hash: &ContentId) -> PathBuf {
        super::snapshot_path(self.root(), hash)
    }

    pub fn find_snapshot(&self, hash: &ContentId) -> Option<Manifest> {
        super::read_published(self.root(), hash)
    }

    pub fn publish_snapshot(
        &self,
        entries: Vec<SnapshotEntry>,
        options: PublishOptions,
    ) -> Result<PublishReceipt, BuildError> {
        if let Some(old_hash) = options.base_snapshot {
            self.stage_and_publish(
                entries,
                options.lockfile_hash,
                StageSeed::CloneTree,
                &mut |tree_dir, manifest, timing| {
                    self.fill_incremental_tree(
                        tree_dir,
                        manifest,
                        &old_hash,
                        options.paranoid,
                        timing,
                    )
                },
            )
        } else {
            self.stage_and_publish(
                entries,
                options.lockfile_hash,
                StageSeed::FreshTree,
                &mut |tree_dir, manifest, timing| {
                    let mut tree_timings = TreeTimings {
                        verify_ms: timing.verify_ms,
                        link_train_ms: timing.link_train_ms,
                    };
                    self.build_tree(tree_dir, manifest, options.paranoid, &mut tree_timings)?;
                    timing.verify_ms = tree_timings.verify_ms;
                    timing.link_train_ms = tree_timings.link_train_ms;
                    Ok(())
                },
            )
        }
    }

    pub fn publish_manifest(
        &self,
        manifest: &Manifest,
        mut options: PublishOptions,
    ) -> Result<PublishReceipt, BuildError> {
        if options.lockfile_hash.is_none() {
            options.lockfile_hash = manifest.lockfile_hash;
        }
        self.publish_snapshot(manifest.entries.clone(), options)
    }

    fn fill_incremental_tree(
        &self,
        tree_dir: &Path,
        manifest: &Manifest,
        old_hash: &ContentId,
        paranoid: bool,
        timing: &mut SnapshotBuildTiming,
    ) -> Result<(), BuildError> {
        let Some(old_manifest) = super::read_published(self.root(), old_hash) else {
            return Err(BuildError::Fatal(format!(
                "old snapshot {old_hash} is no longer valid; cannot rebuild incrementally"
            )));
        };
        let diff = SnapshotDiff::compute(&old_manifest.entries, &manifest.entries);

        let old_tree = super::snapshot_tree_path(self.root(), old_hash);

        let stage = Instant::now();
        if let Err(e) = clone_dir_recursive(&old_tree, tree_dir) {
            return Err(BuildError::Fatal(format!(
                "whole-tree clone {} -> {} failed ({e}); falling back to a full build",
                old_tree.display(),
                tree_dir.display()
            )));
        }
        timing.clone_units_ms += stage.elapsed().as_millis() as u64;
        timing.clone_units = 1usize;

        let old_by_rel: std::collections::HashMap<&str, &SnapshotEntry> = old_manifest
            .entries
            .iter()
            .map(|e| (e.rel.as_str(), e))
            .collect();
        let mut tree_timings = TreeTimings {
            verify_ms: timing.verify_ms,
            link_train_ms: timing.link_train_ms,
        };

        let mut deleted: Vec<&SnapshotEntry> = diff.deleted.iter().collect();
        deleted.sort_by_key(|e| std::cmp::Reverse(e.rel.len()));
        let stage = Instant::now();
        for entry in deleted {
            let dest = tree_dir.join(&entry.rel);
            let result = match fs::symlink_metadata(&dest) {
                Ok(md) if md.file_type().is_dir() => fs::remove_dir_all(&dest),
                Ok(_) => fs::remove_file(&dest),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e),
            };
            if let Err(e) = result {
                return Err(BuildError::Fatal(format!(
                    "cannot delete staged {}: {e}",
                    dest.display()
                )));
            }
        }
        tree_timings.link_train_ms += stage.elapsed().as_millis() as u64;

        for entry in diff.modified {
            let mode_only = entry.kind == EntryKind::File
                && old_by_rel
                    .get(entry.rel.as_str())
                    .is_some_and(|old| old.blob == entry.blob);
            if mode_only {
                let stage = Instant::now();
                let dest = tree_dir.join(&entry.rel);
                fs::set_permissions(&dest, fs::Permissions::from_mode(entry.mode)).map_err(
                    |e| BuildError::Fatal(format!("cannot chmod {}: {e}", dest.display())),
                )?;
                tree_timings.link_train_ms += stage.elapsed().as_millis() as u64;
                continue;
            }
            let stage = Instant::now();
            let dest = tree_dir.join(&entry.rel);
            match fs::symlink_metadata(&dest) {
                Ok(md) if md.file_type().is_dir() => {
                    let _ = fs::remove_dir_all(&dest);
                }
                Ok(_) => {
                    let _ = fs::remove_file(&dest);
                }
                Err(_) => {}
            }
            tree_timings.link_train_ms += stage.elapsed().as_millis() as u64;
            self.place_entry(&entry, tree_dir, paranoid, &mut tree_timings)?;
            if entry.kind == EntryKind::File {
                timing.linked_files += 1;
            }
        }

        for entry in diff.added {
            self.place_entry(&entry, tree_dir, paranoid, &mut tree_timings)?;
            if entry.kind == EntryKind::File {
                timing.linked_files += 1;
            }
        }

        for entry in manifest.entries.iter().filter(|e| e.kind == EntryKind::Dir) {
            if !tree_dir.join(&entry.rel).exists() {
                self.place_entry(entry, tree_dir, false, &mut tree_timings)?;
            }
        }

        if paranoid {
            paranoid_verify_tree(tree_dir, manifest)?;
        }

        timing.verify_ms = tree_timings.verify_ms;
        timing.link_train_ms = tree_timings.link_train_ms;
        Ok(())
    }

    fn stage_and_publish(
        &self,
        entries: Vec<SnapshotEntry>,
        lockfile_hash: Option<ContentId>,
        seed: StageSeed,
        fill_tree: &mut dyn FnMut(
            &Path,
            &Manifest,
            &mut SnapshotBuildTiming,
        ) -> Result<(), BuildError>,
    ) -> Result<PublishReceipt, BuildError> {
        let mut timing = SnapshotBuildTiming::default();

        let stage = Instant::now();
        let mut unique_blobs = std::collections::BTreeSet::new();
        let mut total_size = 0u64;
        for e in &entries {
            if let Some(blob) = e.blob {
                if unique_blobs.insert(blob) {
                    if let Ok(meta) = fs::metadata(self.blob_path(&blob)) {
                        total_size += meta.len();
                    }
                }
            }
        }
        let manifest = Manifest::new_with_lockfile_and_size(entries, lockfile_hash, total_size)?;
        timing.publish_ms += stage.elapsed().as_millis() as u64;

        let stage = Instant::now();
        let tmp_base = self.root().join("snapshots").join("tmp");
        fs::create_dir_all(&tmp_base)?;
        let tmp = tempfile::Builder::new()
            .prefix("build-")
            .tempdir_in(&tmp_base)?;
        let tmp_path = tmp.path().to_path_buf();
        match seed {
            StageSeed::FreshTree => {
                fs::create_dir_all(tmp_path.join(TREE_SUBDIR))?;
            }

            StageSeed::CloneTree => {}
        }
        timing.link_train_ms += stage.elapsed().as_millis() as u64;

        let tree_dir = tmp_path.join(TREE_SUBDIR);

        fill_tree(&tree_dir, &manifest, &mut timing)?;

        let stage = Instant::now();

        crate::fsutil::durable_write(
            &tmp_path.join("manifest.tsv"),
            manifest.serialize().as_bytes(),
        )?;
        crate::fsutil::durable_write(
            &tmp_path.join(".complete"),
            format!("v1\t{}\n", manifest.hash).as_bytes(),
        )?;

        let final_path = self.snapshot_path(&manifest.hash);
        let outcome = match fs::rename(&tmp_path, &final_path) {
            Ok(()) => {
                crate::fsutil::sync_parent_dir(&final_path)?;
                PublishOutcome::Published
            }
            Err(e) if matches!(e.raw_os_error(), Some(libc::EEXIST) | Some(libc::ENOTEMPTY)) => {
                if super::read_published(self.root(), &manifest.hash).is_some() {
                    PublishOutcome::WinnerValid
                } else {
                    PublishOutcome::WinnerInvalid
                }
            }
            Err(e) => return Err(e.into()),
        };
        drop(tmp);
        timing.publish_ms += stage.elapsed().as_millis() as u64;

        Ok(PublishReceipt { outcome, timing })
    }
}
