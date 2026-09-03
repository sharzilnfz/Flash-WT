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

#[derive(Debug, Clone, Copy)]
pub struct HydratePolicy {
    pub verify: bool,

    pub snapshots: bool,

    pub v2: bool,

    pub strategy: flashwt_copy::StrategyPolicy,
}

#[derive(Debug, Clone, Copy)]
pub struct HydrateDest<'a> {
    pub worktree_root: &'a Path,

    pub git_dir: &'a Path,

    pub base_branch: Option<&'a str>,

    pub base_commit: Option<&'a str>,
}

#[derive(Debug, Clone, Copy)]
pub struct HydrateTree<'a> {
    pub ingested: &'a Ingested,

    pub repo_root: &'a Path,

    pub pattern: &'a str,

    pub src_root: &'a Path,

    pub heavy_rel: &'a str,

    pub lockfile_hash: Option<ContentId>,
}

#[derive(Debug, Clone, Copy)]
pub struct HydratePinned<'a> {
    pub repo_root: &'a Path,

    pub pattern: &'a str,

    pub src_root: &'a Path,

    pub heavy_rel: &'a str,

    pub lockfile_hash: ContentId,
}

#[derive(Debug, Clone, Copy)]
pub enum HydrateSrc<'a> {
    Tree(HydrateTree<'a>),

    PinnedLockfile(HydratePinned<'a>),
}

#[derive(Debug, Clone, Copy)]
pub struct HydrateReq<'a> {
    pub src: HydrateSrc<'a>,

    pub dest: HydrateDest<'a>,

    pub policy: HydratePolicy,
}

#[derive(Debug, Clone)]
pub enum HydrateOutcome {
    Hydrated(HydrationReceipt),

    NeedIngest { diagnostic: Option<String> },

    Failed(String),
}

#[derive(Debug, Clone)]
pub struct HydrationReceipt {
    pub strategy: String,

    pub files_total: usize,

    pub files_copied: usize,

    pub bytes_shared: u64,

    pub bytes_copied: u64,

    pub snapshot_hit: bool,

    pub elapsed_ms: u128,

    pub diagnostics: Vec<String>,

    pub v2_cloned: usize,

    pub v2_linked: usize,

    pub incremental_decision: Option<crate::snapshot::IncrementalDecision>,

    pub incremental_fallback_reason: Option<String>,

    pub incremental_hit_rate: Option<f64>,

    pub copy_backend: Option<String>,

    pub refusal_reason: Option<String>,
}

impl DiskStore {
    pub fn hydrate(&mut self, req: HydrateReq<'_>) -> Result<HydrateOutcome> {
        match req.src {
            HydrateSrc::PinnedLockfile(pin) => self.hydrate_pinned(pin, req.dest, req.policy),
            HydrateSrc::Tree(tree) => self
                .hydrate_tree(tree, req.dest, req.policy)
                .map(HydrateOutcome::Hydrated),
        }
    }

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
                    incremental_hit_rate: None,
                    copy_backend: Some("clonefile".to_string()),
                    refusal_reason: None,
                }))
            }
            SnapshotOutcome::FellBack(reason) => {
                Ok(HydrateOutcome::NeedIngest { diagnostic: reason })
            }
            SnapshotOutcome::Failed(msg) => Ok(HydrateOutcome::Failed(msg)),
        }
    }

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
                        incremental_hit_rate: info.incremental_hit_rate,
                        copy_backend: Some("clonefile".to_string()),
                        refusal_reason: None,
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

        for rel in &ingested.dirs {
            let dir_path = resolve_dest_path(dest.worktree_root, tree.heavy_rel, rel);
            fs::create_dir_all(&dir_path)?;
        }

        let materializer =
            flashwt_copy::Materializer::for_paths(policy.strategy, self.root(), dest.worktree_root);
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
            items.push(flashwt_copy::BatchItem {
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
            flashwt_copy::StrategyPolicy::Default => "copy-on-write",
            flashwt_copy::StrategyPolicy::Hardlink => "hardlink",
            flashwt_copy::StrategyPolicy::ForceByteCopy => "byte-copy",
        };

        let copy_backend = Some(batch.backend_name.to_string());
        let refusal_reason = batch.refusal_reason.clone();
        if let Some(refusal) = &refusal_reason {
            diagnostics.push(format!(
                "acceleration refused ({refusal}); falling back to byte copies"
            ));
        }

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
            incremental_hit_rate: None,
            copy_backend,
            refusal_reason,
        })
    }
}

fn append_ledger(
    git_dir: &Path,
    files: &BTreeMap<String, ContentId>,
    snapshot: Option<&ContentId>,
) -> Result<()> {
    let sidecar_file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(git_dir.join("flashwt-hydrated.tsv"))?;
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
