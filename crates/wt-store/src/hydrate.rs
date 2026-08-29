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
use std::path::Path;
use std::time::Instant;

use crate::snapshot::{SnapshotOutcome, SnapshotProjectionEngine, SnapshotProjectionRequest};
use crate::{ContentId, DiskStore, GcMode, Result, Store};

/// Request parameters for whole-worktree and heavy-directory hydration.
#[derive(Debug, Clone)]
pub struct HydrationRequest<'a> {
    /// Root directory of the destination worktree.
    pub worktree_root: &'a Path,
    /// Git directory path of the destination worktree.
    pub git_dir: &'a Path,
    /// Relative directory paths to create.
    pub dirs: &'a [String],
    /// Explicit permissions for directories.
    pub dir_modes: &'a BTreeMap<String, u32>,
    /// Relative file path to content-addressed ID table.
    pub files: &'a BTreeMap<String, ContentId>,
    /// File size mapping in bytes.
    pub file_sizes: &'a BTreeMap<String, u64>,
    /// Relative symbolic links.
    pub symlinks: &'a BTreeMap<String, String>,
    /// Explicit permissions for files.
    pub modes: &'a BTreeMap<String, u32>,
    /// Source repository root.
    pub repo_root: &'a Path,
    /// Inclusion pattern string.
    pub pattern: &'a str,
    /// Source checkout root.
    pub src_root: &'a Path,
    /// Relative path to heavy directory.
    pub heavy_rel: &'a str,
    /// Pinned lockfile hash if present.
    pub lockfile_hash: Option<&'a ContentId>,
    /// Optional base branch name.
    pub base_branch: Option<&'a str>,
    /// Optional base commit hash.
    pub base_commit: Option<&'a str>,
    /// Whether paranoid verification is required.
    pub verify: bool,
    /// Whether snapshot projection is enabled.
    pub snapshots_enabled: bool,
    /// Whether v2 incremental snapshot rebuilds are enabled.
    pub v2_enabled: bool,
    /// Placement strategy policy.
    pub strategy_policy: wt_copy::StrategyPolicy,
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
}

impl DiskStore {
    /// Hydrate a worktree using either snapshot projection (when enabled and available)
    /// or verified fallback file materialization via `wt_copy::CopyEngine`.
    pub fn hydrate(&mut self, req: HydrationRequest<'_>) -> Result<HydrationReceipt> {
        let start = Instant::now();
        let mut diagnostics = Vec::new();

        fs::create_dir_all(req.worktree_root)?;
        fs::create_dir_all(req.git_dir)?;

        if req.snapshots_enabled {
            let proj_req = SnapshotProjectionRequest {
                dirs: req.dirs,
                dir_modes: req.dir_modes,
                files: req.files,
                file_sizes: req.file_sizes,
                symlinks: req.symlinks,
                modes: req.modes,
                repo_root: req.repo_root,
                pattern: req.pattern,
                src_root: req.src_root,
                heavy_rel: req.heavy_rel,
                dest_root: req.worktree_root,
                lockfile_hash: req.lockfile_hash,
                verify: req.verify,
                snapshots_enabled: req.snapshots_enabled,
                v2_enabled: req.v2_enabled,
            };

            match SnapshotProjectionEngine::hydrate(self, &proj_req) {
                SnapshotOutcome::Hydrated(info) => {
                    let distinct_blobs: BTreeSet<&ContentId> = req.files.values().collect();
                    if self.gc_mode() != GcMode::MarkSweepNoRefs {
                        for id in &distinct_blobs {
                            self.add_ref(id)?;
                        }
                    }

                    let sidecar_file = fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(req.git_dir.join("wt-hydrated.tsv"))?;
                    let mut sidecar = io::BufWriter::with_capacity(128 * 1024, sidecar_file);
                    for (rel, id) in req.files {
                        writeln!(sidecar, "{rel}\tblob\t{id}")?;
                    }
                    writeln!(sidecar, "-\tsnapshot\t{}", info.hash)?;
                    sidecar.flush()?;

                    let manifest_id = info.hash;
                    self.publish_worktree_mirror(
                        req.worktree_root,
                        req.git_dir,
                        distinct_blobs,
                        std::iter::once(&manifest_id),
                        req.base_branch,
                        req.base_commit,
                    )?;

                    let total_bytes: u64 = req.file_sizes.values().sum();
                    return Ok(HydrationReceipt {
                        strategy: format!("snapshot-{}", info.mode),
                        files_total: req.files.len(),
                        files_copied: 0,
                        bytes_shared: total_bytes,
                        bytes_copied: 0,
                        snapshot_hit: true,
                        elapsed_ms: start.elapsed().as_millis(),
                        diagnostics,
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
        let dest_heavy = if req.heavy_rel.is_empty() {
            req.worktree_root.to_path_buf()
        } else {
            req.worktree_root.join(req.heavy_rel)
        };
        fs::create_dir_all(&dest_heavy)?;

        for rel in req.dirs {
            let dir_path = dest_heavy.join(rel);
            fs::create_dir_all(&dir_path)?;
        }

        let engine = wt_copy::CopyEngine::new(req.strategy_policy);
        let files: Vec<(&String, &ContentId)> = req.files.iter().collect();
        let num_cpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        let num_workers = num_cpus.clamp(4, 8).min(files.len()).max(1);

        let next_file_idx = std::sync::atomic::AtomicUsize::new(0);
        let total_copied = std::sync::atomic::AtomicUsize::new(0);
        let total_bytes_shared = std::sync::atomic::AtomicU64::new(0);
        let total_bytes_copied = std::sync::atomic::AtomicU64::new(0);
        let err_slot: std::sync::Mutex<Option<crate::Error>> = std::sync::Mutex::new(None);

        std::thread::scope(|s| {
            for _ in 0..num_workers {
                s.spawn(|| {
                    let mut worker_copied = 0usize;
                    let mut worker_bytes_shared = 0u64;
                    let mut worker_bytes_copied = 0u64;

                    loop {
                        if err_slot.lock().unwrap_or_else(|p| p.into_inner()).is_some() {
                            break;
                        }

                        let idx = next_file_idx.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if idx >= files.len() {
                            break;
                        }

                        let (rel, id) = files[idx];
                        let size = req.file_sizes.get(rel).copied().unwrap_or(0);

                        let v_res = if req.verify {
                            self.get(id).map(|_| ())
                        } else {
                            self.ensure_verified(id)
                        };

                        if let Err(e) = v_res {
                            let mut slot = err_slot.lock().unwrap_or_else(|p| p.into_inner());
                            if slot.is_none() {
                                *slot = Some(e);
                            }
                            break;
                        }

                        let src = self.blob_path(id);
                        let dest = dest_heavy.join(rel);
                        if let Some(parent) = dest.parent() {
                            if !parent.exists() {
                                let _ = fs::create_dir_all(parent);
                            }
                        }
                        let mode = req.modes.get(rel).copied();

                        let outcome = match engine.materialize_file(&src, &dest, mode) {
                            Ok(outcome) => outcome,
                            Err(e) => {
                                let mut slot = err_slot.lock().unwrap_or_else(|p| p.into_inner());
                                if slot.is_none() {
                                    *slot = Some(crate::Error::Io(e));
                                }
                                break;
                            }
                        };

                        if outcome.is_shared_cow {
                            worker_bytes_shared += size;
                        } else {
                            worker_copied += 1;
                            worker_bytes_copied += size;
                        }
                    }

                    total_copied.fetch_add(worker_copied, std::sync::atomic::Ordering::Relaxed);
                    total_bytes_shared
                        .fetch_add(worker_bytes_shared, std::sync::atomic::Ordering::Relaxed);
                    total_bytes_copied
                        .fetch_add(worker_bytes_copied, std::sync::atomic::Ordering::Relaxed);
                });
            }
        });

        if let Some(err) = err_slot.into_inner().unwrap_or_default() {
            return Err(err);
        }

        for (rel, target) in req.symlinks {
            let dest = dest_heavy.join(rel);
            if let Some(parent) = dest.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent)?;
                }
            }
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(target, &dest)?;
            }
        }

        for rel in req.dirs.iter().rev() {
            if let Some(&mode) = req.dir_modes.get(rel) {
                let path = dest_heavy.join(rel);
                #[cfg(unix)]
                {
                    let _ = fs::set_permissions(&path, fs::Permissions::from_mode(mode));
                }
            }
        }

        let distinct_blobs: BTreeSet<&ContentId> = req.files.values().collect();
        if self.gc_mode() != GcMode::MarkSweepNoRefs {
            for id in &distinct_blobs {
                self.add_ref(id)?;
            }
        }

        let sidecar_file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(req.git_dir.join("wt-hydrated.tsv"))?;
        let mut sidecar = io::BufWriter::with_capacity(128 * 1024, sidecar_file);
        for (rel, id) in req.files {
            writeln!(sidecar, "{rel}\t{id}")?;
        }
        sidecar.flush()?;

        self.publish_worktree_mirror(
            req.worktree_root,
            req.git_dir,
            distinct_blobs,
            std::iter::empty(),
            req.base_branch,
            req.base_commit,
        )?;

        let strategy_str = match req.strategy_policy {
            wt_copy::StrategyPolicy::Default => "copy-on-write",
            wt_copy::StrategyPolicy::Hardlink => "hardlink",
            wt_copy::StrategyPolicy::ForceByteCopy => "byte-copy",
        };

        Ok(HydrationReceipt {
            strategy: strategy_str.to_string(),
            files_total: req.files.len(),
            files_copied: total_copied.into_inner(),
            bytes_shared: total_bytes_shared.into_inner(),
            bytes_copied: total_bytes_copied.into_inner(),
            snapshot_hit: false,
            elapsed_ms: start.elapsed().as_millis(),
            diagnostics,
        })
    }
}
