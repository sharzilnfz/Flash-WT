//! Deep hydration engine coordinating discovery, caching, and delegation
//! to [`wt_store::DiskStore::hydrate`].

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

use wt_store::{ContentId, DiskStore, IngestOptions};

use crate::config::{RunConfig, StrategyPolicy};
use crate::envelope::Diagnostic;
use crate::error::{Error, Result};
use crate::hydration_filter::{collect_matches, pattern_matches};
use crate::timing::StageTimings;
use crate::workspace;

/// Where the per-machine store lives. `$WT_STORE` wins (tests use it
/// for isolation); otherwise XDG cache conventions.
fn store_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("WT_STORE") {
        return Ok(PathBuf::from(dir));
    }
    let base = match std::env::var_os("XDG_CACHE_HOME") {
        Some(xdg) => PathBuf::from(xdg),
        None => {
            let home = std::env::var_os("HOME").ok_or_else(|| {
                Error::Usage("cannot locate a home directory for the store".into())
            })?;
            PathBuf::from(home).join(".cache")
        }
    };
    Ok(base.join("wt").join("store"))
}

fn store_policy(cfg: &RunConfig) -> wt_store::HydratePolicy {
    wt_store::HydratePolicy {
        verify: cfg.verify,
        snapshots: cfg.snapshots,
        v2: cfg.v2,
        strategy: match cfg.strategy_policy {
            StrategyPolicy::Default => wt_copy::StrategyPolicy::Default,
            StrategyPolicy::Hardlink => wt_copy::StrategyPolicy::Hardlink,
            StrategyPolicy::ForceByteCopy => wt_copy::StrategyPolicy::ForceByteCopy,
        },
    }
}

pub fn open_store() -> Result<DiskStore> {
    let dir = store_dir()?;
    DiskStore::open(&dir)
        .map_err(|e| Error::Store(format!("cannot open store at {}: {e}", dir.display())))
}

/// Request parameters for worktree hydration.
pub struct HydrationRequest<'a> {
    pub root: &'a Path,
    pub dest: &'a Path,
    pub patterns: &'a [String],
    pub base_branch: Option<&'a str>,
    pub base_commit: Option<&'a str>,
    pub cfg: &'a RunConfig,
}

/// Threshold constants for the tiny repo bypass policy (Ticket 03).
pub const TINY_REPO_MAX_FILES: u64 = 500;
pub const TINY_REPO_MAX_BYTES: u64 = 8 * 1024 * 1024; // 8 MB

/// Count regular files and total bytes under `dir`, stopping early once limits are reached.
pub fn scan_dir_size_capped(
    dir: &Path,
    current_files: &mut u64,
    current_bytes: &mut u64,
    max_files: u64,
    max_bytes: u64,
) -> std::io::Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    let mut stack = vec![dir.to_path_buf()];
    while let Some(path) = stack.pop() {
        if *current_files >= max_files || *current_bytes >= max_bytes {
            break;
        }
        for entry in std::fs::read_dir(&path)? {
            let entry = entry?;
            let ft = entry.file_type()?;
            if ft.is_dir() {
                stack.push(entry.path());
            } else if ft.is_file() {
                *current_files += 1;
                *current_bytes += entry.metadata()?.len();
                if *current_files >= max_files || *current_bytes >= max_bytes {
                    break;
                }
            }
        }
    }
    Ok(())
}

/// Returns true if target directories total under 500 files and under 8 MB.
pub fn is_tiny_repo(root: &Path, dirs: &[PathBuf]) -> bool {
    if dirs.is_empty() {
        return false;
    }
    let mut files = 0u64;
    let mut bytes = 0u64;
    for rel in dirs {
        let src = root.join(rel);
        if scan_dir_size_capped(
            &src,
            &mut files,
            &mut bytes,
            TINY_REPO_MAX_FILES,
            TINY_REPO_MAX_BYTES,
        )
        .is_err()
        {
            return false;
        }
        if files >= TINY_REPO_MAX_FILES || bytes >= TINY_REPO_MAX_BYTES {
            return false;
        }
    }
    files < TINY_REPO_MAX_FILES && bytes < TINY_REPO_MAX_BYTES
}

/// Determines if the given request should bypass the store entirely.
pub fn should_bypass_tiny_repo(root: &Path, dirs: &[PathBuf], cfg: &RunConfig) -> bool {
    if !cfg.tiny_bypass {
        return false;
    }
    if cfg.strategy_policy == StrategyPolicy::Hardlink {
        return false;
    }
    if cfg.verify {
        return false;
    }
    is_tiny_repo(root, dirs)
}

/// Consolidated report of worktree hydration operations and metrics.
#[allow(dead_code)]
pub struct HydrationReport {
    pub total_files: usize,
    pub total_copied: usize,
    pub bytes_shared_cow: u64,
    pub bytes_copied: u64,
    pub strategy: String,
    pub hydration_method: String,
    pub cache_hit: bool,
    pub snapshot_hashes: Vec<ContentId>,
    pub dirs_hydrated: Vec<PathBuf>,
    pub timings: StageTimings,
    pub diagnostics: Vec<Diagnostic>,
}

/// Deep hydration engine coordinating discovery, caching, and storage delegation.
pub struct HydrationEngine<'a> {
    store: Option<&'a mut DiskStore>,
    owned_store: Option<DiskStore>,
}

impl<'a> HydrationEngine<'a> {
    /// Create a new `HydrationEngine` backed by the given disk store.
    pub fn new(store: &'a mut DiskStore) -> Self {
        Self {
            store: Some(store),
            owned_store: None,
        }
    }

    /// Create a new `HydrationEngine` that will open the store lazily only if needed.
    pub fn auto() -> Self {
        Self {
            store: None,
            owned_store: None,
        }
    }

    fn store_mut(&mut self) -> Result<&mut DiskStore> {
        if let Some(ref mut store) = self.store {
            return Ok(store);
        }
        if self.owned_store.is_none() {
            self.owned_store = Some(open_store()?);
        }
        self.owned_store
            .as_mut()
            .ok_or_else(|| Error::Store("store not initialized".into()))
    }

    fn hydrate_tiny_bypass(
        &mut self,
        req: HydrationRequest<'_>,
        dirs: &[PathBuf],
        mut timings: StageTimings,
    ) -> Result<HydrationReport> {
        let copy_policy = match req.cfg.strategy_policy {
            StrategyPolicy::Default => wt_copy::StrategyPolicy::Default,
            StrategyPolicy::Hardlink => wt_copy::StrategyPolicy::Hardlink,
            StrategyPolicy::ForceByteCopy => wt_copy::StrategyPolicy::ForceByteCopy,
        };
        let copy_engine = wt_copy::CopyEngine::new(copy_policy);

        let stage = Instant::now();
        let mut total_files = 0usize;
        let mut total_copied = 0usize;
        let mut bytes_shared_cow = 0u64;
        let mut bytes_copied = 0u64;
        let mut last_strategy = "clonefile".to_string();
        let mut dirs_hydrated = Vec::new();

        for rel in dirs {
            dirs_hydrated.push(rel.clone());
            let src = req.root.join(rel);
            let dest_dir = req.dest.join(rel);

            if let Some(parent) = dest_dir.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| Error::io("create directory", parent, e))?;
            }

            let receipt = copy_engine
                .copy_dir(&src, &dest_dir, wt_copy::SourcePolicy::Any)
                .map_err(|e| {
                    Error::io_unanchored("bypass copy", &src, std::io::Error::other(e.to_string()))
                })?;

            last_strategy = receipt.strategy.to_string();
            let is_cow = receipt.strategy == "clonefile"
                || receipt.strategy == "reflink"
                || receipt.strategy == "copy-on-write";
            if is_cow {
                bytes_shared_cow += receipt.bytes_copied;
            } else {
                bytes_copied += receipt.bytes_copied;
                total_copied += receipt.files_copied as usize;
            }
            total_files += receipt.files_copied as usize;

            if !req.cfg.json {
                println!(
                    "hydrated {} from {} via clone ({} file{})",
                    rel.display(),
                    src.display(),
                    receipt.files_copied,
                    if receipt.files_copied == 1 { "" } else { "s" }
                );
            }
        }

        crate::toolchain::relocate_toolchains(req.root, req.dest, dirs)?;
        timings.materialize_ms = stage.elapsed().as_millis();

        let hydration_method = if total_files == 0 {
            "none"
        } else if total_copied == 0 {
            "clone"
        } else if total_copied == total_files {
            "byte_copy"
        } else {
            "clone"
        }
        .to_string();

        Ok(HydrationReport {
            total_files,
            total_copied,
            bytes_shared_cow,
            bytes_copied,
            strategy: last_strategy,
            hydration_method,
            cache_hit: false,
            snapshot_hashes: Vec::new(),
            dirs_hydrated,
            timings,
            diagnostics: Vec::new(),
        })
    }

    /// Execute the complete hydration pipeline for the given request.
    pub fn hydrate(&mut self, req: HydrationRequest<'_>) -> Result<HydrationReport> {
        let mut timings = StageTimings::new();
        let dirs = collect_matches(req.root, req.patterns)?;
        let git_dir = workspace::resolve_git_dir(req.dest);
        if !git_dir.exists() {
            let _ = std::fs::create_dir_all(&git_dir);
        }

        if dirs.is_empty() {
            let store = self.store_mut()?;
            store
                .publish_worktree_mirror(
                    req.dest,
                    &git_dir,
                    BTreeSet::new(),
                    std::iter::empty(),
                    req.base_branch,
                    req.base_commit,
                )
                .map_err(|e| Error::Store(format!("cannot publish worktree mirror: {e}")))?;

            let diagnostics =
                crate::base::check_base_movement(store, req.root, req.base_branch);
            for d in &diagnostics {
                if !req.cfg.json {
                    eprintln!("wt: warning: {}", d.message);
                }
            }

            if !req.cfg.json {
                println!("nothing to hydrate");
            }

            return Ok(HydrationReport {
                total_files: 0,
                total_copied: 0,
                bytes_shared_cow: 0,
                bytes_copied: 0,
                strategy: "none".to_string(),
                hydration_method: "none".to_string(),
                cache_hit: false,
                snapshot_hashes: Vec::new(),
                dirs_hydrated: Vec::new(),
                timings,
                diagnostics,
            });
        }

        if should_bypass_tiny_repo(req.root, &dirs, req.cfg) {
            return self.hydrate_tiny_bypass(req, &dirs, timings);
        }

        let store = self.store_mut()?;

        let mut total_files = 0usize;
        let mut total_copied = 0usize;
        let mut bytes_shared_cow = 0u64;
        let mut bytes_copied = 0u64;
        let mut last_strategy = "byte-copy".to_string();
        let mut snapshot_hits_count = 0usize;
        let mut dirs_hydrated = Vec::new();
        let mut report_diagnostics = Vec::new();

        for rel in &dirs {
            dirs_hydrated.push(rel.clone());
            let src = req.root.join(rel);
            let heavy = rel.to_string_lossy().into_owned();

            let pattern = req
                .patterns
                .iter()
                .find(|p| pattern_matches(p, rel))
                .map(String::as_str)
                .unwrap_or("");

            let lockfile_info = wt_store::find_lockfile(req.root, rel).and_then(|lp| {
                let content = std::fs::read(&lp).ok()?;
                let text = std::str::from_utf8(&content).ok()?;
                let safety = wt_store::classify_lockfile(text);
                let hash = wt_store::hash_lockfile(&content);
                Some((safety, hash))
            });

            let pinned_lockfile_hash = match lockfile_info {
                Some((wt_store::DependencySafety::Pinned, hash)) => Some(hash),
                _ => None,
            };
            if req.cfg.snapshots {
                if let Some(lock_hash) = pinned_lockfile_hash {
                    let stage = Instant::now();
                    let pin_req = wt_store::HydrateReq {
                        src: wt_store::HydrateSrc::PinnedLockfile(wt_store::HydratePinned {
                            repo_root: req.root,
                            pattern,
                            src_root: &src,
                            heavy_rel: &heavy,
                            lockfile_hash: lock_hash,
                        }),
                        dest: wt_store::HydrateDest {
                            worktree_root: req.dest,
                            git_dir: &git_dir,
                            base_branch: req.base_branch,
                            base_commit: req.base_commit,
                        },
                        policy: store_policy(req.cfg),
                    };
                    match store
                        .hydrate(pin_req)
                        .map_err(|e| Error::Store(format!("hydration of {heavy} failed: {e}")))?
                    {
                        wt_store::HydrateOutcome::Hydrated(info) => {
                            let hydration_ms = stage.elapsed().as_millis();
                            total_files += info.files_total;
                            snapshot_hits_count += 1;
                            timings.snapshot_engaged = true;
                            timings.snapshot_ms += hydration_ms;
                            timings.snapshot_mode = "hit";
                            if !req.cfg.json {
                                println!(
                                    "hydrated {heavy} from {} via snapshot (one clone, {} file{})",
                                    src.display(),
                                    info.files_total,
                                    if info.files_total == 1 { "" } else { "s" }
                                );
                            }
                            continue;
                        }
                        wt_store::HydrateOutcome::NeedIngest {
                            diagnostic: Some(reason),
                        } => {
                            if !req.cfg.json {
                                eprintln!(
                                    "wt-snapshots: {heavy}: lockfile fast path fell back ({reason})"
                                );
                            }
                        }
                        wt_store::HydrateOutcome::NeedIngest { diagnostic: None } => {}
                        wt_store::HydrateOutcome::Failed(msg) => {
                            return Err(Error::Store(format!(
                                "hydration of {heavy} failed: {msg}"
                            )));
                        }
                    }
                }
            }
            let stage = Instant::now();
            let ingested = store
                .ingest_tree(
                    req.root,
                    &src,
                    &IngestOptions {
                        snapshots: req.cfg.snapshots,
                        exclude: &|rel| crate::toolchain::is_volatile_cache(rel),
                    },
                )
                .map_err(|e| Error::Store(format!("ingest {}: {e}", src.display())))?;
            timings.ingest_ms += stage.elapsed().as_millis();

            let store_req = wt_store::HydrateReq {
                src: wt_store::HydrateSrc::Tree(wt_store::HydrateTree {
                    ingested: &ingested,
                    repo_root: req.root,
                    pattern,
                    src_root: &src,
                    heavy_rel: &heavy,
                    lockfile_hash: pinned_lockfile_hash,
                }),
                dest: wt_store::HydrateDest {
                    worktree_root: req.dest,
                    git_dir: &git_dir,
                    base_branch: req.base_branch,
                    base_commit: req.base_commit,
                },
                policy: store_policy(req.cfg),
            };

            let stage = Instant::now();
            let receipt = match store
                .hydrate(store_req)
                .map_err(|e| Error::Store(format!("hydration of {} failed: {e}", rel.display())))?
            {
                wt_store::HydrateOutcome::Hydrated(r) => r,
                wt_store::HydrateOutcome::NeedIngest { diagnostic } => {
                    return Err(Error::Store(format!(
                        "hydration of {} unexpectedly deferred{}",
                        rel.display(),
                        diagnostic.map(|d| format!(" ({d})")).unwrap_or_default(),
                    )));
                }
                wt_store::HydrateOutcome::Failed(msg) => {
                    return Err(Error::Store(format!(
                        "hydration of {} failed: {msg}",
                        rel.display()
                    )));
                }
            };
            let hydration_ms = stage.elapsed().as_millis();

            total_files += receipt.files_total;
            total_copied += receipt.files_copied;
            bytes_shared_cow += receipt.bytes_shared;
            bytes_copied += receipt.bytes_copied;
            last_strategy = receipt.strategy.clone();

            if receipt.snapshot_hit {
                snapshot_hits_count += 1;
                timings.snapshot_engaged = true;
                timings.snapshot_ms += hydration_ms;
                if let Some(mode) = receipt.strategy.strip_prefix("snapshot-") {
                    match mode {
                        "hit" => timings.snapshot_mode = "hit",
                        "build" => {
                            timings.snapshot_mode = "build";
                            timings.snapshot_built = true;
                        }
                        "v2" => timings.snapshot_mode = "v2",
                        _ => {}
                    }
                }
                timings.v2_cloned += receipt.v2_cloned;
                timings.v2_linked += receipt.v2_linked;
                if !req.cfg.json {
                    println!(
                        "hydrated {heavy} from {} via snapshot (one clone, {} file{})",
                        src.display(),
                        receipt.files_total,
                        if receipt.files_total == 1 { "" } else { "s" }
                    );
                }
            } else {
                timings.materialize_ms += hydration_ms;
                if !req.cfg.json {
                    println!(
                        "hydrated {} from {} via store ({} file{})",
                        rel.display(),
                        src.display(),
                        receipt.files_total,
                        if receipt.files_total == 1 { "" } else { "s" }
                    );
                }
                if !req.cfg.json {
                    for diag in &receipt.diagnostics {
                        eprintln!(
                            "wt-snapshots: {heavy}: falling back to per-file placement ({diag})"
                        );
                    }
                }
            }

            for diag in receipt.diagnostics {
                report_diagnostics.push(Diagnostic::warning("HYDRATION_DIAGNOSTIC", diag));
            }
        }

        crate::toolchain::relocate_toolchains(req.root, req.dest, &dirs)?;

        if !req.cfg.json {
            if req.cfg.strategy_policy == StrategyPolicy::ForceByteCopy {
                println!(
                    "hardlink mode off (WT_NO_HARDLINK): wrote byte copies for all {total_files} file(s)"
                );
            } else {
                match (last_strategy.as_str(), total_copied) {
                    ("hardlink", 0) => println!(
                        "experimental hardlink mode (WT_HARDLINK): linked shared inodes for all {total_files} file(s)"
                    ),
                    ("hardlink", n) => println!(
                        "experimental hardlink mode (WT_HARDLINK): hardlinks refused for {n} of {total_files} file(s); wrote byte copies"
                    ),
                    (_, 0) => {}
                    (name, n) => println!(
                        "{name} unavailable on this filesystem: wrote byte copies for {n} of {total_files} file(s)"
                    ),
                }
            }
        }

        store.flush().map_err(|e| {
            Error::io_unanchored("update verified-blob ledger", store.root(), e)
        })?;

        if !req.cfg.json {
            println!(
                "hydration complete: {total_files} file{} through the store",
                if total_files == 1 { "" } else { "s" }
            );
        }

        let mut diagnostics =
            crate::base::check_base_movement(store, req.root, req.base_branch);
        diagnostics.extend(report_diagnostics);

        if total_copied > 0 && req.cfg.strategy_policy != StrategyPolicy::Hardlink {
            diagnostics.push(Diagnostic::warning(
                "CROSS_DEVICE_COPY_DEGRADATION",
                format!(
                    "Storage boundaries or filesystem refusal forced fallback byte copies for {total_copied} of {total_files} file(s)"
                ),
            ));
        }
        for d in &diagnostics {
            if !req.cfg.json {
                eprintln!("wt: warning: {}", d.message);
            }
        }

        let hydration_method = if total_files == 0 {
            "none"
        } else if snapshot_hits_count == dirs.len()
            || (last_strategy == "copy-on-write" && total_copied == 0)
        {
            "clone"
        } else if last_strategy == "hardlink" && total_copied == 0 {
            "hardlink"
        } else if total_copied == total_files {
            "byte_copy"
        } else {
            match last_strategy.as_str() {
                "copy-on-write" => "clone",
                "reflink" => "reflink",
                "copy_file_range" => "copy_file_range",
                "hardlink" => "hardlink",
                _ => "byte_copy",
            }
        }
        .to_string();

        let cache_hit = (snapshot_hits_count == dirs.len() && !timings.snapshot_built)
            || (total_files > 0 && total_copied == 0 && !timings.snapshot_built);

        Ok(HydrationReport {
            total_files,
            total_copied,
            bytes_shared_cow,
            bytes_copied,
            strategy: last_strategy,
            hydration_method,
            cache_hit,
            snapshot_hashes: Vec::new(),
            dirs_hydrated,
            timings,
            diagnostics,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::fs::{self, File};

    fn test_config() -> RunConfig {
        RunConfig {
            strategy_policy: StrategyPolicy::Default,
            verify: false,
            snapshots: false,
            v2: false,
            timing: false,
            json: false,
            tiny_bypass: true,
        }
    }

    #[test]
    fn is_tiny_repo_empty_directories() {
        let temp = tempfile::tempdir().unwrap();
        assert!(!is_tiny_repo(temp.path(), &[]));
    }

    #[test]
    fn is_tiny_repo_below_limits() {
        let temp = tempfile::tempdir().unwrap();
        let heavy = temp.path().join("heavy");
        fs::create_dir_all(&heavy).unwrap();

        for i in 0..10 {
            fs::write(heavy.join(format!("file_{i}.txt")), b"test payload").unwrap();
        }

        let dirs = vec![PathBuf::from("heavy")];
        assert!(is_tiny_repo(temp.path(), &dirs));
    }

    #[test]
    fn is_tiny_repo_exceeds_file_count() {
        let temp = tempfile::tempdir().unwrap();
        let heavy = temp.path().join("heavy");
        fs::create_dir_all(&heavy).unwrap();

        for i in 0..TINY_REPO_MAX_FILES {
            fs::write(heavy.join(format!("file_{i}.txt")), b"").unwrap();
        }

        let dirs = vec![PathBuf::from("heavy")];
        assert!(!is_tiny_repo(temp.path(), &dirs));
    }

    #[test]
    fn is_tiny_repo_exceeds_byte_size() {
        let temp = tempfile::tempdir().unwrap();
        let heavy = temp.path().join("heavy");
        fs::create_dir_all(&heavy).unwrap();

        let file = File::create(heavy.join("large.bin")).unwrap();
        file.set_len(TINY_REPO_MAX_BYTES).unwrap();

        let dirs = vec![PathBuf::from("heavy")];
        assert!(!is_tiny_repo(temp.path(), &dirs));
    }

    #[test]
    fn should_bypass_tiny_repo_honors_configuration() {
        let temp = tempfile::tempdir().unwrap();
        let heavy = temp.path().join("heavy");
        fs::create_dir_all(&heavy).unwrap();
        fs::write(heavy.join("file.txt"), b"tiny").unwrap();
        let dirs = vec![PathBuf::from("heavy")];

        let mut cfg = test_config();
        assert!(should_bypass_tiny_repo(temp.path(), &dirs, &cfg));

        cfg.tiny_bypass = false;
        assert!(!should_bypass_tiny_repo(temp.path(), &dirs, &cfg));
    }

    #[test]
    fn should_bypass_tiny_repo_ignores_hardlink_and_verify() {
        let temp = tempfile::tempdir().unwrap();
        let heavy = temp.path().join("heavy");
        fs::create_dir_all(&heavy).unwrap();
        fs::write(heavy.join("file.txt"), b"tiny").unwrap();
        let dirs = vec![PathBuf::from("heavy")];

        let mut cfg = test_config();
        cfg.strategy_policy = StrategyPolicy::Hardlink;
        assert!(!should_bypass_tiny_repo(temp.path(), &dirs, &cfg));

        cfg = test_config();
        cfg.verify = true;
        assert!(!should_bypass_tiny_repo(temp.path(), &dirs, &cfg));

        cfg = test_config();
        cfg.strategy_policy = StrategyPolicy::Hardlink;
        cfg.verify = true;
        assert!(!should_bypass_tiny_repo(temp.path(), &dirs, &cfg));
    }
}
