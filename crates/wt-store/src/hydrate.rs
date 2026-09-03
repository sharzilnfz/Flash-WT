//! Unified hydration engine and storage ledger orchestration.
//!
//! Encapsulates snapshot projection, fallback verified file materialization
//! via [`wt_copy::CopyEngine`], sidecar ledger (`wt-hydrated.tsv`) persistence,
//! and garbage collection mirror publication.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::ingest::Ingested;
use crate::snapshot::{SnapshotOutcome, SnapshotProjectionEngine, SnapshotProjectionRequest};
use crate::{ContentId, DiskStore, GcMode, Result};

/// Placement and verification policy for one hydration run.
#[derive(Debug, Clone, Copy)]
pub struct HydratePolicy {
    /// Re-hash every blob before placement; also bypasses snapshot hits.
    pub verify: bool,
    /// Whether whole-directory snapshot projection is enabled.
    pub snapshots: bool,
    /// Whether v2 incremental snapshot rebuilds are enabled.
    pub v2: bool,
    /// Placement strategy for fallback materialization.
    pub strategy: wt_copy::StrategyPolicy,
}

/// Destination identity for one hydrated directory.
#[derive(Debug, Clone, Copy)]
pub struct HydrateDest<'a> {
    /// Root directory of the destination worktree.
    pub worktree_root: &'a Path,
    /// Git directory path of the destination worktree.
    pub git_dir: &'a Path,
    /// Optional base branch name.
    pub base_branch: Option<&'a str>,
    /// Optional base commit hash.
    pub base_commit: Option<&'a str>,
}

/// Already-ingested tree source for the full hydration path.
#[derive(Debug, Clone, Copy)]
pub struct HydrateTree<'a> {
    /// Everything the ingest walk found, with source-root-relative paths.
    pub ingested: &'a Ingested,
    /// Source repository root.
    pub repo_root: &'a Path,
    /// Inclusion pattern string for this heavy tree.
    pub pattern: &'a str,
    /// Source checkout root.
    pub src_root: &'a Path,
    /// Relative path to the heavy directory.
    pub heavy_rel: &'a str,
    /// Pinned lockfile hash when the tree ships a strictly pinned lockfile.
    pub lockfile_hash: Option<ContentId>,
}

/// Pinned-lockfile fast-path intent: a hit needs no ingest walk.
#[derive(Debug, Clone, Copy)]
pub struct HydratePinned<'a> {
    /// Source repository root.
    pub repo_root: &'a Path,
    /// Inclusion pattern string for this heavy tree.
    pub pattern: &'a str,
    /// Source checkout root.
    pub src_root: &'a Path,
    /// Relative path to the heavy directory.
    pub heavy_rel: &'a str,
    /// SHA-256 of the strictly pinned lockfile.
    pub lockfile_hash: ContentId,
}

/// Where the bytes come from: a full tree or a lockfile-hit intent.
#[derive(Debug, Clone, Copy)]
pub enum HydrateSrc<'a> {
    /// Full path: ingest already ran, project or materialize it.
    Tree(HydrateTree<'a>),
    /// Fast path: clone a published snapshot without walking the tree.
    PinnedLockfile(HydratePinned<'a>),
}

/// Compact hydration request: source, destination, policy.
#[derive(Debug, Clone, Copy)]
pub struct HydrateReq<'a> {
    /// What to hydrate.
    pub src: HydrateSrc<'a>,
    /// Where to put it.
    pub dest: HydrateDest<'a>,
    /// How to place and verify it.
    pub policy: HydratePolicy,
}

/// Outcome of one [`DiskStore::hydrate`] call.
#[derive(Debug, Clone)]
pub enum HydrateOutcome {
    /// Bytes landed; the caller reports and continues.
    Hydrated(HydrationReceipt),
    /// The fast path missed; the caller ingests then retries as a tree.
    NeedIngest {
        /// Diagnostic carried for one-time printing, never fatal.
        diagnostic: Option<String>,
    },
    /// The fast path failed loudly; the caller fails the create.
    Failed(String),
}

/// Execution report returned by [`DiskStore::hydrate`].
#[derive(Debug, Clone)]
pub struct HydrationReceipt {
    /// Strategy name applied.
    pub strategy: String,
    /// Total files processed.
    pub files_total: usize,
    /// Number of files physically copied.
    pub files_copied: usize,
    /// Bytes shared via CoW or links.
    pub bytes_shared: u64,
    /// Bytes copied directly.
    pub bytes_copied: u64,
    /// Whether a whole-directory snapshot was hit.
    pub snapshot_hit: bool,
    /// Elapsed duration in milliseconds.
    pub elapsed_ms: u128,
    /// Diagnostic warning strings.
    pub diagnostics: Vec<String>,
    /// Incremental rebuild cloned units count.
    pub v2_cloned: usize,
    /// Incremental rebuild freshly linked files count.
    pub v2_linked: usize,
    /// Incremental decision if evaluated.
    pub incremental_decision: Option<crate::snapshot::IncrementalDecision>,
    /// Diagnostic reason if incremental rebuild fell back.
    pub incremental_fallback_reason: Option<String>,
}

impl DiskStore {
    /// Hydrate one heavy directory behind a compact interface.
    ///
    /// A [`HydrateSrc::PinnedLockfile`] intent attempts the O(1) lockfile
    /// fast path with no ingest walk: a hit lands bytes plus ledger and
    /// mirror records, a miss reports [`HydrateOutcome::NeedIngest`] so the
    /// caller ingests and retries as [`HydrateSrc::Tree`]. A tree runs
    /// snapshot projection when enabled, else verified fallback
    /// materialization. Ledger and mirror writes live here in every path;
    /// callers never touch either format.
    pub fn hydrate(&mut self, req: HydrateReq<'_>) -> Result<HydrateOutcome> {
        match req.src {
            HydrateSrc::PinnedLockfile(pin) => self.hydrate_pinned(pin, req.dest, req.policy),
            HydrateSrc::Tree(tree) => self
                .hydrate_tree(tree, req.dest, req.policy)
                .map(HydrateOutcome::Hydrated),
        }
    }

    /// O(1) lockfile fast path: clone a published snapshot whose manifest
    /// header matches the pinned lockfile hash, without walking the tree.
    fn hydrate_pinned(
        &mut self,
        pin: HydratePinned<'_>,
        dest: HydrateDest<'_>,
        policy: HydratePolicy,
    ) -> Result<HydrateOutcome> {
        let start = Instant::now();
        fs::create_dir_all(dest.worktree_root)?;
        fs::create_dir_all(dest.git_dir)?;

        match SnapshotProjectionEngine::try_lockfile_hit(
            self,
            pin.repo_root,
            pin.pattern,
            pin.src_root,
            pin.heavy_rel,
            dest.worktree_root,
            &pin.lockfile_hash,
            policy.verify,
        ) {
            SnapshotOutcome::Hydrated(info) => {
                let manifest_id = info.hash;
                if self.gc_mode() != GcMode::MarkSweepNoRefs {
                    if let Some(manifest) =
                        crate::snapshot::read_published(self.root(), &manifest_id)
                    {
                        let mut distinct_blobs = BTreeSet::new();
                        for entry in &manifest.entries {
                            if let Some(blob) = entry.blob {
                                distinct_blobs.insert(blob);
                            }
                        }
                        for id in &distinct_blobs {
                            self.add_ref(id)?;
                        }
                    }
                }
                append_ledger(dest.git_dir, &BTreeMap::new(), Some(&info.hash))?;
                self.publish_worktree_mirror(
                    dest.worktree_root,
                    dest.git_dir,
                    BTreeSet::new(),
                    std::iter::once(&manifest_id),
                    dest.base_branch,
                    dest.base_commit,
                )?;
                Ok(HydrateOutcome::Hydrated(HydrationReceipt {
                    strategy: "snapshot-hit".to_string(),
                    files_total: info.files,
                    files_copied: 0,
                    bytes_shared: 0,
                    bytes_copied: 0,
                    snapshot_hit: true,
                    elapsed_ms: start.elapsed().as_millis(),
                    diagnostics: Vec::new(),
                    v2_cloned: 0,
                    v2_linked: 0,
                    incremental_decision: None,
                    incremental_fallback_reason: None,
                }))
            }
            SnapshotOutcome::FellBack(reason) => {
                Ok(HydrateOutcome::NeedIngest { diagnostic: reason })
            }
            SnapshotOutcome::Failed(msg) => Ok(HydrateOutcome::Failed(msg)),
        }
    }

    /// Full path over an already-ingested tree: snapshot projection when
    /// enabled and available, else verified fallback materialization via
    /// `wt_copy::CopyEngine`.
    fn hydrate_tree(
        &mut self,
        tree: HydrateTree<'_>,
        dest: HydrateDest<'_>,
        policy: HydratePolicy,
    ) -> Result<HydrationReceipt> {
        let start = Instant::now();
        let mut diagnostics = Vec::new();
        let ingested = tree.ingested;

        fs::create_dir_all(dest.worktree_root)?;
        fs::create_dir_all(dest.git_dir)?;

        if policy.snapshots {
            let proj_req = SnapshotProjectionRequest {
                dirs: &ingested.dirs,
                dir_modes: &ingested.dir_modes,
                files: &ingested.files,
                file_sizes: &ingested.file_sizes,
                symlinks: &ingested.symlinks,
                modes: &ingested.modes,
                repo_root: tree.repo_root,
                pattern: tree.pattern,
                src_root: tree.src_root,
                heavy_rel: tree.heavy_rel,
                dest_root: dest.worktree_root,
                lockfile_hash: tree.lockfile_hash.as_ref(),
                verify: policy.verify,
                snapshots_enabled: policy.snapshots,
                v2_enabled: policy.v2,
            };

            match SnapshotProjectionEngine::hydrate(self, &proj_req) {
                SnapshotOutcome::Hydrated(info) => {
                    let distinct_blobs: BTreeSet<&ContentId> = ingested.files.values().collect();
                    if self.gc_mode() != GcMode::MarkSweepNoRefs {
                        for id in &distinct_blobs {
                            self.add_ref(id)?;
                        }
                    }

                    append_ledger(dest.git_dir, &ingested.files, Some(&info.hash))?;

                    let manifest_id = info.hash;
                    self.publish_worktree_mirror(
                        dest.worktree_root,
                        dest.git_dir,
                        BTreeSet::new(),
                        std::iter::once(&manifest_id),
                        dest.base_branch,
                        dest.base_commit,
                    )?;

                    let total_bytes: u64 = ingested.file_sizes.values().sum();
                    return Ok(HydrationReceipt {
                        strategy: format!("snapshot-{}", info.mode),
                        files_total: ingested.files.len(),
                        files_copied: 0,
                        bytes_shared: total_bytes,
                        bytes_copied: 0,
                        snapshot_hit: true,
                        elapsed_ms: start.elapsed().as_millis(),
                        diagnostics,
                        v2_cloned: info.cloned_units,
                        v2_linked: info.linked_files,
                        incremental_decision: info.incremental_decision,
                        incremental_fallback_reason: info.incremental_fallback_reason,
                    });
                }
                SnapshotOutcome::FellBack(diag) => {
                    if let Some(msg) = diag {
                        diagnostics.push(msg);
                    }
                }
                SnapshotOutcome::Failed(msg) => {
                    diagnostics.push(msg);
                }
            }
        }

        // Fallback materialization
        for rel in &ingested.dirs {
            let dir_path = resolve_dest_path(dest.worktree_root, tree.heavy_rel, rel);
            fs::create_dir_all(&dir_path)?;
        }

        let materializer =
            wt_copy::Materializer::for_paths(policy.strategy, self.root(), dest.worktree_root);
        let mut items = Vec::with_capacity(ingested.files.len());
        for (rel, id) in &ingested.files {
            if policy.verify {
                self.get(id).map(|_| ())?;
            } else {
                self.ensure_verified(id)?;
            }
            let src = self.blob_path(id);
            let dest_path = resolve_dest_path(dest.worktree_root, tree.heavy_rel, rel);
            let mode = ingested.modes.get(rel).copied();
            let size = ingested.file_sizes.get(rel).copied().unwrap_or(0);
            items.push(wt_copy::BatchItem {
                src,
                dest: dest_path,
                mode,
                size,
            });
        }
        let batch = materializer
            .materialize_batch(&items)
            .map_err(crate::Error::Io)?;
        let total_copied = batch.placed.saturating_sub(batch.shared_cow);
        let total_bytes_shared = batch.bytes_shared;
        let total_bytes_copied = batch.bytes_copied;

        for (rel, target) in &ingested.symlinks {
            let dest_path = resolve_dest_path(dest.worktree_root, tree.heavy_rel, rel);
            if let Some(parent) = dest_path.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent)?;
                }
            }
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(target, &dest_path)?;
            }
        }

        for rel in ingested.dirs.iter().rev() {
            if let Some(&mode) = ingested.dir_modes.get(rel) {
                let path = resolve_dest_path(dest.worktree_root, tree.heavy_rel, rel);
                {
                    let _ = fs::set_permissions(&path, fs::Permissions::from_mode(mode));
                }
            }
        }

        let distinct_blobs: BTreeSet<&ContentId> = ingested.files.values().collect();
        if self.gc_mode() != GcMode::MarkSweepNoRefs {
            for id in &distinct_blobs {
                self.add_ref(id)?;
            }
        }

        append_ledger(dest.git_dir, &ingested.files, None)?;

        self.publish_worktree_mirror(
            dest.worktree_root,
            dest.git_dir,
            distinct_blobs,
            std::iter::empty(),
            dest.base_branch,
            dest.base_commit,
        )?;

        let strategy_str = match policy.strategy {
            wt_copy::StrategyPolicy::Default => "copy-on-write",
            wt_copy::StrategyPolicy::Hardlink => "hardlink",
            wt_copy::StrategyPolicy::ForceByteCopy => "byte-copy",
        };

        Ok(HydrationReceipt {
            strategy: strategy_str.to_string(),
            files_total: ingested.files.len(),
            files_copied: total_copied,
            bytes_shared: total_bytes_shared,
            bytes_copied: total_bytes_copied,
            snapshot_hit: false,
            elapsed_ms: start.elapsed().as_millis(),
            diagnostics,
            v2_cloned: 0,
            v2_linked: 0,
            incremental_decision: None,
            incremental_fallback_reason: None,
        })
    }
}

/// Single ledger writer for every hydration path. File rows are always
/// `rel blob id`; a snapshot hydration appends one `snapshot` row.
/// Readers accept the legacy two-column fallback rows, but nothing new
/// writes them.
fn append_ledger(
    git_dir: &Path,
    files: &BTreeMap<String, ContentId>,
    snapshot: Option<&ContentId>,
) -> Result<()> {
    let sidecar_file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(git_dir.join("wt-hydrated.tsv"))?;
    let mut sidecar = io::BufWriter::with_capacity(128 * 1024, sidecar_file);
    for (rel, id) in files {
        writeln!(sidecar, "{rel}\tblob\t{id}")?;
    }
    if let Some(hash) = snapshot {
        writeln!(sidecar, "-\tsnapshot\t{hash}")?;
    }
    sidecar.flush()?;
    Ok(())
}

fn resolve_dest_path(worktree_root: &Path, heavy_rel: &str, rel: &str) -> PathBuf {
    if heavy_rel.is_empty() || rel == heavy_rel || rel.starts_with(&format!("{heavy_rel}/")) {
        worktree_root.join(rel)
    } else {
        worktree_root.join(heavy_rel).join(rel)
    }
}
