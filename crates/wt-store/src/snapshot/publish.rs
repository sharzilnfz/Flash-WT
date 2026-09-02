//! The snapshot publish protocol: one private helper
//! ([`DiskStore::stage_and_publish`]) owns everything both routes
//! used to copy-paste — staging dir setup, metadata writes, the
//! atomic rename with EEXIST/ENOTEMPTY winner-collision validation,
//! and loser cleanup. Full and incremental builds differ only in the
//! closure that fills the staged tree.
//!
//! Both public publish functions return [`Result<PublishOutcome,
//! BuildError>`] (flat — no inner store-error layer) and, in their
//! `_with_timing` variants, a [`PublishReceipt`] carrying the phase
//! timings instead of borrowing a `&mut` out-param.

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use super::manifest::{EntryKind, Manifest, SnapshotEntry};
use super::tree::{TreeTimings, clone_dir_recursive, paranoid_verify_tree};
use crate::snapdiff::SnapshotDiff;
use crate::{ContentId, DiskStore};

/// Where the tree lives inside a build temp directory.
const TREE_SUBDIR: &str = "tree";

/// Internal phase timings of one [`DiskStore::publish_snapshot`]
/// build. Milliseconds, best-effort: observation only, never a behavior
/// input. Step 0 instrumentation feeds these straight into
/// `wt-stage snapshot-build-*` lines.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SnapshotBuildTiming {
    /// Blob verification (hash/stat) before linking into staging.
    pub verify_ms: u64,
    /// Staging-tree construction: mkdirs, hardlinks, chmods, symlinks.
    pub link_train_ms: u64,
    /// Manifest serialization/hash, `.complete`, atomic renames.
    pub publish_ms: u64,
    /// v2 incremental rebuilds only: milliseconds spent in the ONE
    /// recursive `clonefile(2)` that copies the old snapshot's whole
    /// tree into staging. Zero for full builds.
    pub clone_units_ms: u64,
    /// v2 incremental rebuilds only: 1 when the whole-tree clone
    /// succeeded (the delta is then applied in place inside the
    /// private copy), 0 otherwise — including when the attempt aborted
    /// and the caller fell back to a full build.
    pub clone_units: usize,
    /// v2 incremental rebuilds only: regular files freshly hardlinked
    /// from blobs while applying the delta (added plus content-modified
    /// entries; mode-only flips chmod in place and do not count). Zero
    /// for full builds, which place everything through `link_train_ms`
    /// instead of counting.
    pub linked_files: usize,
}

/// What a publish did, plus how long its phases took. Returned by the
/// `_with_timing` publish variants so callers who care about Step 0
/// instrumentation get timings without a `&mut` parameter threading
/// through the API; callers who don't use the plain variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublishReceipt {
    /// What happened to the snapshot tree.
    pub outcome: PublishOutcome,
    /// Phase timings for Step 0 instrumentation.
    pub timing: SnapshotBuildTiming,
}

/// What [`DiskStore::publish_snapshot`] did with its temp tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishOutcome {
    /// Our temp tree won the rename; we own the published snapshot.
    Published,
    /// Another writer's valid snapshot was already there; our temp
    /// was discarded and the winner should be used.
    WinnerValid,
    /// Another writer's directory is there but NOT valid. We left it
    /// alone (never overwrite debris we cannot prove ours): treat
    /// this as a miss and fall back.
    WinnerInvalid,
}

/// Why [`DiskStore::publish_snapshot`] failed.
#[derive(Debug)]
pub enum BuildError {
    /// The blob vanished between ingest and link — a sweep raced us.
    /// Re-put the source content, re-verify, and retry ONCE.
    MissingBlob(ContentId),
    /// Anything fatal.
    Fatal(String),
    /// An unexpected store-level IO failure (staging setup, metadata
    /// write, rename).
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

/// How a publish seeds its staging directory.
enum StageSeed {
    /// Pre-create an empty `tree/` inside staging (full build).
    FreshTree,
    /// Deliberately NO pre-made `tree/`: the fill closure clones the
    /// old published snapshot's whole tree onto that exact path
    /// (`clonefile(2)` refuses an existing target). The old snapshot
    /// is captured by the closure itself.
    CloneTree,
}

/// Options controlling snapshot publication in [`DiskStore::publish_snapshot`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PublishOptions {
    /// Optional lockfile hash to associate with the published snapshot manifest.
    pub lockfile_hash: Option<ContentId>,
    /// Base snapshot hash for incremental rebuild. If set, an incremental tree clone & diff
    /// will be used rather than a fresh tree build.
    pub base_snapshot: Option<ContentId>,
    /// Full hash verification on blobs before linking / whole staged tree proof pass.
    pub paranoid: bool,
}

impl PublishOptions {
    /// Default publish options (full build, no lockfile, standard verification).
    pub fn new() -> Self {
        Self::default()
    }

    /// Set whether full paranoid read-and-hash verification is performed.
    pub fn paranoid(mut self, paranoid: bool) -> Self {
        self.paranoid = paranoid;
        self
    }

    /// Set an optional pinned lockfile hash for the snapshot manifest.
    pub fn lockfile_hash(mut self, lockfile_hash: Option<ContentId>) -> Self {
        self.lockfile_hash = lockfile_hash;
        self
    }

    /// Set an optional base snapshot hash to enable incremental v2 tree cloning.
    pub fn base_snapshot(mut self, base_snapshot: Option<ContentId>) -> Self {
        self.base_snapshot = base_snapshot;
        self
    }
}

impl DiskStore {
    /// Directory of the published snapshot for `hash`.
    ///
    /// (Defined here rather than beside [`snapshot_path`] so all
    /// `DiskStore` snapshot methods stay discoverable in one place.)
    pub fn snapshot_path(&self, hash: &ContentId) -> PathBuf {
        super::snapshot_path(self.root(), hash)
    }

    /// Shared validity check for this store's snapshots. See
    /// [`super::read_published`].
    pub fn find_snapshot(&self, hash: &ContentId) -> Option<Manifest> {
        super::read_published(self.root(), hash)
    }

    /// Build the snapshot tree for `entries` in `<root>/snapshots/tmp/<uuid>`
    /// and atomically rename it into place.
    ///
    /// Every file blob is verified BEFORE linking, per policy: full read-and-hash
    /// when `options.paranoid`, verified-ledger trust otherwise ([`DiskStore::ensure_verified`]).
    /// When `options.base_snapshot` is provided, seeds staging with a recursive `clonefile(2)`
    /// of the base snapshot and applies the manifest diff in place.
    ///
    /// Concurrent publish: the rename is the single atomic act. If it loses (EEXIST/ENOTEMPTY),
    /// the winner is validated — valid means discard our temp and use theirs; invalid debris stays
    /// untouched and the caller treats this as a miss. See [`PublishOutcome`].
    ///
    /// Returns [`BuildError::MissingBlob`] when a blob disappeared mid-build (sweep race):
    /// the CALLER re-puts the source content and retries once.
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

    /// The v2 delta application, run against an already-cloned or
    /// fresh `tree_dir`. Split out of the closure so it can be a
    /// readable method; see
    /// [`Self::publish_snapshot_incremental`] for the contract.
    fn fill_incremental_tree(
        &self,
        tree_dir: &Path,
        manifest: &Manifest,
        old_hash: &ContentId,
        paranoid: bool,
        timing: &mut SnapshotBuildTiming,
    ) -> Result<(), BuildError> {
        // The old snapshot must still be valid: selection promised it,
        // but a sweep may have taken it since. Abort rather than guess.
        let Some(old_manifest) = super::read_published(self.root(), old_hash) else {
            return Err(BuildError::Fatal(format!(
                "old snapshot {old_hash} is no longer valid; cannot rebuild incrementally"
            )));
        };
        let diff = SnapshotDiff::compute(&old_manifest.entries, &manifest.entries);

        let old_tree = super::snapshot_tree_path(self.root(), old_hash);

        // ONE whole-tree clone. Any refusal abandons the incremental
        // attempt entirely — the caller falls back to a full build.
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

        // Old entries by relpath, for mode-only-flip detection.
        let old_by_rel: std::collections::HashMap<&str, &SnapshotEntry> = old_manifest
            .entries
            .iter()
            .map(|e| (e.rel.as_str(), e))
            .collect();
        let mut tree_timings = TreeTimings {
            verify_ms: timing.verify_ms,
            link_train_ms: timing.link_train_ms,
        };

        // Deletions first, deepest paths first so removing a directory
        // takes its descendants before their own turn comes up (and
        // NotFound from that double cover is ignored below).
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

        // Content-modified entries: drop whatever the clone left at
        // the path, place fresh through the shared helper. Mode-only
        // flips on same-ref files just chmod the private inode.
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

        // Added entries: place_entry creates missing parent dirs
        // itself (boundary dirs included).
        for entry in diff.added {
            self.place_entry(&entry, tree_dir, paranoid, &mut tree_timings)?;
            if entry.kind == EntryKind::File {
                timing.linked_files += 1;
            }
        }

        // Empty dirs the manifest requires but the delta left missing:
        // an explicitly added empty dir, or one whose children were all
        // deleted without an explicit entry surviving the clone.
        for entry in manifest.entries.iter().filter(|e| e.kind == EntryKind::Dir) {
            if !tree_dir.join(&entry.rel).exists() {
                self.place_entry(entry, tree_dir, false, &mut tree_timings)?;
            }
        }

        // Paranoid proof pass over the WHOLE staged tree. See the
        // trust model in the doc comment above.
        if paranoid {
            paranoid_verify_tree(tree_dir, manifest)?;
        }

        timing.verify_ms = tree_timings.verify_ms;
        timing.link_train_ms = tree_timings.link_train_ms;
        Ok(())
    }

    /// THE shared publish protocol (ticket 01 dedupe). Owns: manifest
    /// construction+hashing, temp-dir setup under
    /// `<root>/snapshots/tmp/`, seed-dependent staging prep, handing
    /// the staged `tree/` to `fill_tree`, metadata writes
    /// (`manifest.tsv`, `.complete`), the atomic rename with
    /// EEXIST/ENOTEMPTY winner validation, and loser/debris cleanup.
    /// Both public publish functions are thin wrappers supplying their
    /// seed and closure.
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

        // Staging prep counts as the start of the link train.
        let stage = Instant::now();
        let tmp_base = self.root().join("snapshots").join("tmp");
        fs::create_dir_all(&tmp_base)?;
        let tmp = tempfile::Builder::new()
            .prefix("build-")
            .tempdir_in(&tmp_base)?;
        let tmp_path = tmp.path().to_path_buf();
        match seed {
            StageSeed::FreshTree => {
                // The clonable tree lives under tree/ so metadata
                // files never leak into a cloned worktree.
                fs::create_dir_all(tmp_path.join(TREE_SUBDIR))?;
            }
            // NOTE: tree/ is deliberately NOT pre-created — it is the
            // clone destination, and clonefile refuses an existing
            // target.
            StageSeed::CloneTree => {}
        }
        timing.link_train_ms += stage.elapsed().as_millis() as u64;

        let tree_dir = tmp_path.join(TREE_SUBDIR);
        // On any fill error the TempDir handle drops right here and
        // removes our partial tree: only ever cache debris, never a
        // published name.
        fill_tree(&tree_dir, &manifest, &mut timing)?;

        let stage = Instant::now();
        // Durability ordering: the metadata that PROVES this snapshot
        // valid (manifest.tsv, .complete) is fsynced before the
        // rename, and the snapshots directory is fsynced after. A
        // crash mid-publish can then never leave the final name over
        // empty/unwritten bytes — the worst case is no published name
        // at all (pure cache debris GC already understands).
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
                // Dropping the TempDir handle now tries to remove the
                // OLD temp path, which no longer exists: a harmless
                // no-op that leaves the published tree untouched.
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
