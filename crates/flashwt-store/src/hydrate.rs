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

pub const ZERO_SAVINGS_NO_MATCHING_DIRS: &str =
    "ZERO_SAVINGS: hydration saved 0 bytes; no matching heavy directories found and the worktree relies strictly on git tracking";

pub const ZERO_SAVINGS_NO_FILES_HYDRATED: &str =
    "ZERO_SAVINGS: hydration saved 0 bytes; no files hydrated from matching directories and the worktree relies strictly on git tracking";

#[derive(Debug, Clone, Copy)]
pub struct HydratePolicy {
    pub verify: bool,

    pub snapshots: bool,

    pub v2: bool,

    pub strategy: flashwt_copy::StrategyPolicy,
}

impl Default for HydratePolicy {
    fn default() -> Self {
        HydratePolicy {
            verify: false,
            snapshots: true,
            v2: true,
            strategy: flashwt_copy::StrategyPolicy::Default,
        }
    }
}

#[derive(Debug, Clone)]
pub struct WorkspaceHydrateReq<'a> {
    pub repo_root: &'a Path,

    pub worktree_root: &'a Path,

    pub git_dir: &'a Path,

    pub patterns: &'a [String],

    pub base_branch: Option<&'a str>,

    pub base_commit: Option<&'a str>,

    pub policy: HydratePolicy,
}

#[derive(Debug, Clone, Copy)]
struct HydrateDest<'a> {
    pub worktree_root: &'a Path,

    pub git_dir: &'a Path,

    pub base_branch: Option<&'a str>,

    pub base_commit: Option<&'a str>,
}

#[derive(Debug, Clone, Copy)]
struct HydrateTree<'a> {
    pub ingested: &'a Ingested,

    pub repo_root: &'a Path,

    pub pattern: &'a str,

    pub src_root: &'a Path,

    pub heavy_rel: &'a str,

    pub lockfile_hash: Option<ContentId>,
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

    pub snapshot_hash: Option<ContentId>,
}

impl DiskStore {

    pub fn hydrate_workspace(&mut self, req: WorkspaceHydrateReq<'_>) -> Result<HydrationReceipt> {
        let start = Instant::now();
        let matched_dirs = collect_matches(req.repo_root, req.patterns)?;

        let has_files = !matched_dirs.is_empty()
            && matched_dirs
                .iter()
                .any(|rel| has_any_files(&req.repo_root.join(rel)));

        if !has_files {
            fs::create_dir_all(req.worktree_root)?;
            fs::create_dir_all(req.git_dir)?;

            self.publish_worktree_mirror(
                req.worktree_root,
                req.git_dir,
                std::iter::empty(),
                std::iter::empty(),
                req.base_branch,
                req.base_commit,
            )?;

            self.flush()?;

            let diagnostic = if matched_dirs.is_empty() {
                ZERO_SAVINGS_NO_MATCHING_DIRS.to_string()
            } else {
                ZERO_SAVINGS_NO_FILES_HYDRATED.to_string()
            };

            return Ok(HydrationReceipt {
                strategy: "none".to_string(),
                files_total: 0,
                files_copied: 0,
                bytes_shared: 0,
                bytes_copied: 0,
                snapshot_hit: false,
                elapsed_ms: start.elapsed().as_millis(),
                diagnostics: vec![diagnostic],
                v2_cloned: 0,
                v2_linked: 0,
                incremental_decision: None,
                incremental_fallback_reason: None,
                incremental_hit_rate: None,
                copy_backend: None,
                refusal_reason: None,
                snapshot_hash: None,
            });
        }

        let mut total_files = 0;
        let mut files_copied = 0;
        let mut bytes_shared = 0u64;
        let mut bytes_copied = 0u64;
        let mut diagnostics = Vec::new();
        let mut all_blob_ids = BTreeSet::new();
        let mut snapshot_ids = BTreeSet::new();
        let mut last_strategy = "copy-on-write".to_string();
        let mut last_copy_backend = None;
        let mut last_refusal_reason = None;
        let mut v2_cloned = 0;
        let mut v2_linked = 0;
        let mut incremental_decision = None;
        let mut incremental_fallback_reason = None;
        let mut incremental_hit_rate = None;
        let mut nonempty_dir_count = 0;
        let mut snapshot_hits_count = 0;

        fs::create_dir_all(req.worktree_root)?;
        fs::create_dir_all(req.git_dir)?;

        let can_snapshot = req.policy.snapshots
            && !req.policy.verify
            && req.policy.strategy == flashwt_copy::StrategyPolicy::Default;

        for rel in &matched_dirs {
            let src = req.repo_root.join(rel);
            if !has_any_files(&src) {
                continue;
            }
            nonempty_dir_count += 1;

            let pattern = req
                .patterns
                .iter()
                .find(|p| pattern_matches(p, rel))
                .map(String::as_str)
                .unwrap_or("");

            let heavy_rel = rel.to_string_lossy();
            let pinned_lockfile_hash = crate::lockfile::find_lockfile(req.repo_root, rel).and_then(|lp| {
                let content = fs::read(&lp).ok()?;
                let text = std::str::from_utf8(&content).ok()?;
                let safety = crate::lockfile::classify_lockfile(text);
                if safety == crate::lockfile::DependencySafety::Pinned {
                    Some(crate::lockfile::hash_lockfile(&content))
                } else {
                    None
                }
            });

            let mut hit_this_dir = false;
            if can_snapshot {
                if let Some(lock_hash) = pinned_lockfile_hash {
                    match SnapshotProjectionEngine::try_lockfile_hit(
                        self,
                        req.repo_root,
                        pattern,
                        req.repo_root,
                        &heavy_rel,
                        req.worktree_root,
                        &lock_hash,
                        req.policy.verify,
                    ) {
                        SnapshotOutcome::Hydrated(info) => {
                            let manifest_id = info.hash;
                            snapshot_ids.insert(manifest_id);
                            if let Some(manifest) = crate::snapshot::read_published(self.root(), &manifest_id) {
                                for entry in &manifest.entries {
                                    if let Some(blob) = entry.blob {
                                        bytes_shared += fs::metadata(self.object_path(&blob))
                                            .map(|m| m.len())
                                            .unwrap_or(0);
                                        if self.gc_mode() != GcMode::MarkSweepNoRefs {
                                            self.add_ref(&blob)?;
                                        }
                                    }
                                }
                            }
                            append_ledger(req.git_dir, &BTreeMap::new(), Some(&info.hash))?;
                            total_files += info.files;
                            hit_this_dir = true;
                            snapshot_hits_count += 1;
                            last_strategy = "snapshot-hit".to_string();
                            last_copy_backend = Some("clonefile".to_string());
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
            }

            if !hit_this_dir {
                let mut tree_policy = req.policy;
                if tree_policy.verify {
                    tree_policy.snapshots = false;
                }

                let ingested = self.ingest_tree(
                    req.repo_root,
                    &src,
                    &crate::IngestOptions {
                        snapshots: can_snapshot && !req.policy.verify,
                        exclude: &|rel_str| is_volatile_cache(rel_str),
                    },
                )?;

                let tree_receipt = self.hydrate_tree(
                    HydrateTree {
                        ingested: &ingested,
                        repo_root: req.repo_root,
                        pattern,
                        src_root: &src,
                        heavy_rel: &heavy_rel,
                        lockfile_hash: pinned_lockfile_hash,
                    },
                    HydrateDest {
                        worktree_root: req.worktree_root,
                        git_dir: req.git_dir,
                        base_branch: req.base_branch,
                        base_commit: req.base_commit,
                    },
                    tree_policy,
                )?;

                if !tree_receipt.strategy.starts_with("snapshot") {
                    for blob in ingested.files.values() {
                        all_blob_ids.insert(*blob);
                    }
                } else if let Some(hash) = tree_receipt.snapshot_hash {
                    snapshot_ids.insert(hash);
                }

                total_files += tree_receipt.files_total;
                files_copied += tree_receipt.files_copied;
                bytes_shared += tree_receipt.bytes_shared;
                bytes_copied += tree_receipt.bytes_copied;
                last_strategy = tree_receipt.strategy;
                last_copy_backend = tree_receipt.copy_backend;
                if tree_receipt.refusal_reason.is_some() {
                    last_refusal_reason = tree_receipt.refusal_reason;
                }
                diagnostics.extend(tree_receipt.diagnostics);
                v2_cloned += tree_receipt.v2_cloned;
                v2_linked += tree_receipt.v2_linked;
                if tree_receipt.incremental_decision.is_some() {
                    incremental_decision = tree_receipt.incremental_decision;
                }
                if tree_receipt.incremental_fallback_reason.is_some() {
                    incremental_fallback_reason = tree_receipt.incremental_fallback_reason;
                }
                if tree_receipt.incremental_hit_rate.is_some() {
                    incremental_hit_rate = tree_receipt.incremental_hit_rate;
                }
                if tree_receipt.snapshot_hit {
                    snapshot_hits_count += 1;
                }
            }
        }

        if !all_blob_ids.is_empty() || !snapshot_ids.is_empty() {
            if self.gc_mode() != GcMode::MarkSweepNoRefs {
                for id in &all_blob_ids {
                    self.add_ref(id)?;
                }
            }

            self.publish_worktree_mirror(
                req.worktree_root,
                req.git_dir,
                &all_blob_ids,
                &snapshot_ids,
                req.base_branch,
                req.base_commit,
            )?;
        }

        self.flush()?;

        let overall_strategy = if nonempty_dir_count == 0 {
            "none".to_string()
        } else if snapshot_hits_count == nonempty_dir_count {
            "snapshot-hit".to_string()
        } else if snapshot_hits_count > 0 {
            "mixed".to_string()
        } else {
            last_strategy
        };
        let overall_snapshot_hit = nonempty_dir_count > 0 && snapshot_hits_count == nonempty_dir_count;

        Ok(HydrationReceipt {
            strategy: overall_strategy,
            files_total: total_files,
            files_copied,
            bytes_shared,
            bytes_copied,
            snapshot_hit: overall_snapshot_hit,
            elapsed_ms: start.elapsed().as_millis(),
            diagnostics,
            v2_cloned,
            v2_linked,
            incremental_decision,
            incremental_fallback_reason,
            incremental_hit_rate,
            copy_backend: last_copy_backend,
            refusal_reason: last_refusal_reason,
            snapshot_hash: if snapshot_ids.len() == 1 {
                snapshot_ids.iter().copied().next()
            } else {
                None
            },
        })
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
                        snapshot_hit: info.mode == "hit",
                        elapsed_ms: start.elapsed().as_millis(),
                        diagnostics,
                        v2_cloned: info.cloned_units,
                        v2_linked: info.linked_files,
                        incremental_decision: info.incremental_decision,
                        incremental_fallback_reason: info.incremental_fallback_reason,
                        incremental_hit_rate: info.incremental_hit_rate,
                        copy_backend: Some("clonefile".to_string()),
                        refusal_reason: None,
                        snapshot_hash: Some(info.hash),
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
            snapshot_hash: None,
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

pub fn pattern_matches(pattern: &str, rel: &Path) -> bool {
    let pat = pattern.trim_end_matches('/').trim_start_matches('/');
    if pat.is_empty() {
        return false;
    }
    let segs: Vec<&str> = pat.split('/').collect();
    let rel_text = rel.to_string_lossy();
    let rel_clean = rel_text.trim_start_matches('/').trim_end_matches('/');
    if rel_clean.is_empty() {
        return false;
    }
    let path_segs: Vec<&str> = rel_clean.split('/').collect();
    if pat.contains('/') {
        glob_match(&segs, &path_segs)
    } else {
        path_segs.iter().any(|seg| segment_match(pat, seg))
    }
}

fn glob_match(pat: &[&str], path: &[&str]) -> bool {
    match pat.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => (0..=path.len()).any(|i| glob_match(rest, &path[i..])),
        Some((p, rest)) => match path.split_first() {
            Some((seg, tail)) if segment_match(p, seg) => glob_match(rest, tail),
            _ => false,
        },
    }
}

fn segment_match(pattern: &str, segment: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == segment,
        Some((prefix, suffix)) => {
            segment.len() >= prefix.len() + suffix.len()
                && segment.starts_with(prefix)
                && segment.ends_with(suffix)
        }
    }
}

pub fn is_volatile_cache(rel_path: &str) -> bool {
    let normalized = rel_path.trim_start_matches('/').trim_end_matches('/');
    let segs: Vec<&str> = normalized.split('/').collect();

    for (i, &seg) in segs.iter().enumerate() {
        if seg == "target" {
            if segs.get(i + 1) == Some(&"incremental") {
                return true;
            }
            if segs.get(i + 2) == Some(&"incremental") {
                return true;
            }
        }
        if seg == "incremental" && i > 0 && segs[..i].contains(&"target") {
            return true;
        }
        if seg == ".next" && segs.get(i + 1) == Some(&"cache") {
            return true;
        }
        if seg == "cache" && i > 0 && segs[..i].ends_with(&[".next"]) {
            return true;
        }
        if seg == "node_modules" && segs.get(i + 1) == Some(&".vite") {
            return true;
        }
        if seg == ".vite" && i > 0 && segs[..i].ends_with(&["node_modules"]) {
            return true;
        }
    }

    false
}

pub fn collect_matches(root: &Path, patterns: &[String]) -> Result<Vec<PathBuf>> {
    let mut matched = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    let positive_patterns: Vec<&str> = patterns
        .iter()
        .map(String::as_str)
        .filter(|p| !p.starts_with('!'))
        .collect();
    let negative_patterns: Vec<&str> = patterns
        .iter()
        .map(String::as_str)
        .filter(|p| p.starts_with('!'))
        .map(|p| p.trim_start_matches('!'))
        .collect();

    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() || entry.file_name() == ".git" {
                continue;
            }
            let rel = path.strip_prefix(root).map_err(|_| {
                crate::Error::Io(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("pattern matched path outside repository root: {}", path.display()),
                ))
            })?;
            let rel_str = rel.to_string_lossy();
            if negative_patterns.iter().any(|p| pattern_matches(p, rel))
                || is_volatile_cache(&rel_str)
            {
                continue;
            }
            if positive_patterns.iter().any(|p| pattern_matches(p, rel)) {
                matched.push(rel.to_path_buf());
            } else {
                stack.push(path);
            }
        }
    }
    matched.sort();
    Ok(matched
        .into_iter()
        .scan(None::<PathBuf>, |prev, rel| {
            let covered = prev.as_ref().is_some_and(|p| rel.starts_with(p));
            if !covered {
                *prev = Some(rel.clone());
            }
            Some((!covered).then_some(rel))
        })
        .flatten()
        .collect())
}

fn has_any_files(dir: &Path) -> bool {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() || path.is_symlink() {
                return true;
            }
            if path.is_dir() {
                stack.push(path);
            }
        }
    }
    false
}
