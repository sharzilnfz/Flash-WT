//! `wt create`: worktree creation plus manifest-driven hydration
//! (tickets 02 + 05), decomposed from main.rs by arch-hardening
//! ticket 03.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use wt_store::{ContentId, DiskStore};

use crate::config::{RunConfig, StrategyPolicy};
use crate::envelope::{CreateData, Diagnostic};
use crate::error::{Error, Result};
use crate::gitops;
use crate::hydrate::{
    Ingested, MaterializeReport, claim_references, claim_snapshot_references, ingest_dir,
    materialize, open_store, publish_mirror,
};
use crate::manifest::{self, LoadedPatterns, collect_matches, load_patterns, pattern_matches};
use crate::snapshots;
use crate::snapshots::Outcome as SnapshotOutcome;
use crate::timing::StageTimings;

pub fn run(
    name: &str,
    manifest: Option<&Path>,
    dir: Option<&Path>,
    cfg: &RunConfig,
) -> Result<(CreateData, Vec<Diagnostic>)> {
    create(name, manifest, dir, cfg)
}

fn create(
    name: &str,
    manifest: Option<&Path>,
    dir: Option<&Path>,
    cfg: &RunConfig,
) -> Result<(CreateData, Vec<Diagnostic>)> {
    let root = gitops::repo_root()?;
    let dest = match dir {
        Some(d) => d.to_path_buf(),
        None => gitops::default_worktree_dest(&root, name)?,
    };
    if dest.exists() {
        return Err(Error::Usage(format!("{} already exists", dest.display())));
    }

    let timing_enabled = cfg.timing;
    let mut timings = StageTimings::new();
    let started = Instant::now();
    // Prefer creating the branch from HEAD; an existing branch falls
    // back to checking it out directly.
    let dest_text = dest.to_string_lossy().into_owned();
    gitops::run(&root, &["worktree", "add", "-b", name, &dest_text, "HEAD"])
        .or_else(|_| gitops::run(&root, &["worktree", "add", &dest_text, name]))?;
    timings.git_worktree_ms = started.elapsed().as_millis();

    if !cfg.json {
        println!(
            "created worktree {} from {}",
            dest.display(),
            root.display()
        );
    }

    let patterns = match load_patterns(&root, manifest)? {
        LoadedPatterns::CreatedStarter { path, patterns } => {
            if !cfg.json {
                println!(
                    "no .wtinclude in {}; using defaults ({})",
                    root.display(),
                    manifest::DEFAULT_PATTERNS.join(" ")
                );
                println!("wrote starter manifest {}", path.display());
            }
            patterns
        }
        LoadedPatterns::Loaded { patterns } => patterns,
    };
    let dirs = collect_matches(&root, &patterns)?;
    if dirs.is_empty() {
        if !cfg.json {
            println!("nothing to hydrate");
            timings.emit(started, timing_enabled);
        }
        let data = CreateData {
            worktree_path: dest.display().to_string(),
            branch: name.to_string(),
            cache_hit: false,
            duration_ms: started.elapsed().as_millis() as u64,
            hydration_method: "none".to_string(),
            bytes_shared_cow: 0,
            bytes_copied: 0,
            files_hydrated: 0,
        };
        return Ok((data, Vec::new()));
    }

    let mut store = open_store()?;
    let mut total_files = 0usize;
    let mut total_copied = 0usize;
    let mut bytes_shared_cow = 0u64;
    let mut bytes_copied = 0u64;
    let mut strategy = "byte-copy";
    let mut combined = Ingested {
        dirs: Vec::new(),
        dir_modes: BTreeMap::new(),
        files: BTreeMap::new(),
        file_sizes: BTreeMap::new(),
        symlinks: BTreeMap::new(),
        modes: BTreeMap::new(),
    };
    // Ticket 08: heavy directories hydrated through snapshots record
    // their manifest hashes here; the mirror names them, not the
    // child blobs (the manifest marks those).
    let mut snapshot_hashes: Vec<ContentId> = Vec::new();
    let mut git_dir = dest.clone(); // replaced by claim_references
    for rel in &dirs {
        let outcome = hydrate_one_dir(&mut store, &patterns, &root, &dest, rel, cfg, &mut timings)?;
        git_dir = outcome.git_dir;
        if let Some(hash) = outcome.snapshot_hash {
            snapshot_hashes.push(hash);
        }
        match outcome.ladder {
            // Snapshot fast path: the clone carried the whole tree.
            None => {
                total_files += outcome.ingested.files.len();
                bytes_shared_cow += outcome.ingested.file_sizes.values().sum::<u64>();
            }
            Some(report) => {
                combined.dirs.extend(outcome.ingested.dirs.iter().cloned());
                for (rel, mode) in &outcome.ingested.dir_modes {
                    combined.dir_modes.insert(rel.clone(), *mode);
                }
                for (rel, id) in &outcome.ingested.files {
                    combined.files.insert(rel.clone(), *id);
                }
                for (rel, size) in &outcome.ingested.file_sizes {
                    combined.file_sizes.insert(rel.clone(), *size);
                }
                total_files += report.files;
                total_copied += report.copied;
                bytes_shared_cow += report.bytes_shared;
                bytes_copied += report.bytes_copied;
                strategy = report.strategy;
            }
        }
    }
    // Ticket 07: one atomic mirror write per successful create is
    // the GC bookkeeping mark-and-sweep marks through. Ticket 08:
    // snapshot-hydrated dirs appear as `snapshot` records here.
    let stage = Instant::now();
    publish_mirror(&mut store, &dest, &git_dir, &combined, &snapshot_hashes)?;
    timings.references_ms += stage.elapsed().as_millis();

    // Post-hydration toolchain relocation pass (ticket 08).
    crate::toolchain::relocate_toolchains(&root, &dest, &dirs)?;

    // Say plainly what happened to shared content.
    if !cfg.json {
        if cfg.strategy_policy == StrategyPolicy::ForceByteCopy {
            println!(
                "hardlink mode off (WT_NO_HARDLINK): wrote byte copies for all {total_files} file(s)"
            );
        } else {
            match (strategy, total_copied) {
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
    // Persist the verified-blob ledger explicitly: the Drop below is
    // a best-effort backup, but a clean run should leave its
    // verifications behind even if something later fails hard.
    store
        .flush()
        .map_err(|e| Error::io_unanchored("update verified-blob ledger", store.root(), e))?;

    if !cfg.json {
        println!(
            "hydration complete: {total_files} file{} through the store",
            { if total_files == 1 { "" } else { "s" } }
        );
        timings.emit(started, timing_enabled);
    }

    let mut diagnostics = Vec::new();
    if total_copied > 0 && cfg.strategy_policy != StrategyPolicy::ForceByteCopy {
        diagnostics.push(Diagnostic::warning(
            "CROSS_DEVICE_COPY_DEGRADATION",
            format!(
                "Storage boundaries or filesystem refusal forced fallback byte copies for {total_copied} of {total_files} file(s)"
            ),
        ));
    }

    let hydration_method = if total_files == 0 {
        "none"
    } else if snapshot_hashes.len() == dirs.len() {
        "clone"
    } else if strategy == "copy-on-write" && total_copied == 0 {
        "clone"
    } else if strategy == "hardlink" && total_copied == 0 {
        "hardlink"
    } else if total_copied == total_files {
        "byte_copy"
    } else {
        match strategy {
            "copy-on-write" => "clone",
            "reflink" => "reflink",
            "copy_file_range" => "copy_file_range",
            "hardlink" => "hardlink",
            _ => "byte_copy",
        }
    }
    .to_string();

    let cache_hit = (snapshot_hashes.len() == dirs.len() && !timings.snapshot_built)
        || (total_files > 0 && total_copied == 0 && !timings.snapshot_built);

    let data = CreateData {
        worktree_path: dest.display().to_string(),
        branch: name.to_string(),
        cache_hit,
        duration_ms: started.elapsed().as_millis() as u64,
        hydration_method,
        bytes_shared_cow,
        bytes_copied,
        files_hydrated: total_files,
    };

    Ok((data, diagnostics))
}

/// What one heavy directory contributed to the worktree and the
/// mirror. Extracted from the former create loop so each directory's
/// hydration is a single call.
struct DirOutcome {
    /// The worktree's resolved git dir (captured while claiming
    /// references).
    git_dir: PathBuf,
    /// Manifest hash when the snapshot fast path served this
    /// directory; the mirror records it instead of the child blobs.
    snapshot_hash: Option<ContentId>,
    ingested: Ingested,
    /// Per-file ladder placement report; `None` means the snapshot
    /// fast path served this whole directory with one clone.
    ladder: Option<MaterializeReport>,
}

/// Ingest, then hydrate, one heavy directory: the snapshot fast path
/// when engaged and able, otherwise the per-file ladder (verify +
/// place). Stage timings and the per-directory progress line are this
/// function's side effects.
fn hydrate_one_dir(
    store: &mut DiskStore,
    patterns: &[String],
    root: &Path,
    dest: &Path,
    rel: &Path,
    cfg: &RunConfig,
    timings: &mut StageTimings,
) -> Result<DirOutcome> {
    let src = root.join(rel);
    let heavy = rel.to_string_lossy().into_owned();

    // v2 selection-index key: the first manifest pattern that
    // matched this heavy directory. Only stability across runs
    // matters, not uniqueness.
    let pattern = patterns
        .iter()
        .find(|p| pattern_matches(p, rel))
        .map(String::as_str)
        .unwrap_or("");

    // Ticket 09: Tiered lockfile validation with mutable dependency safety classification.
    let lockfile_info = wt_store::find_lockfile(root, rel).and_then(|lp| {
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

    if cfg.snapshots && !cfg.verify {
        if let Some(ref lock_hash) = pinned_lockfile_hash {
            let stage = Instant::now();
            match snapshots::try_lockfile_hit(
                store, root, pattern, root, &heavy, dest, lock_hash, cfg,
            ) {
                SnapshotOutcome::Hydrated(h) => {
                    timings.snapshot_ms += stage.elapsed().as_millis();
                    timings.snapshot_engaged = true;
                    timings.snapshot_lookup_ms += h.lookup_ms;
                    timings.snapshot_clonefile_ms += h.clonefile_ms;
                    timings.snapshot_mode = h.mode;
                    timings.v2_cloned += h.cloned_units;
                    timings.v2_linked += h.linked_files;
                    let refs = Instant::now();
                    let empty_ingested = Ingested {
                        dirs: Vec::new(),
                        dir_modes: BTreeMap::new(),
                        files: BTreeMap::new(),
                        file_sizes: BTreeMap::new(),
                        symlinks: BTreeMap::new(),
                        modes: BTreeMap::new(),
                    };
                    let git_dir = claim_snapshot_references(store, dest, &empty_ingested, h.hash)?;
                    timings.references_ms += refs.elapsed().as_millis();
                    if !cfg.json {
                        println!(
                            "hydrated {heavy} from {} via snapshot {} (one clone, {} file{})",
                            src.display(),
                            &h.hash.to_string()[..12],
                            h.files,
                            if h.files == 1 { "" } else { "s" },
                        );
                    }
                    return Ok(DirOutcome {
                        git_dir,
                        snapshot_hash: Some(h.hash),
                        ingested: empty_ingested,
                        ladder: None,
                    });
                }
                SnapshotOutcome::FellBack(Some(reason)) => {
                    eprintln!("wt-snapshots: {heavy}: lockfile fast path fell back ({reason})");
                }
                SnapshotOutcome::FellBack(None) => {}
                SnapshotOutcome::Failed(msg) => {
                    return Err(Error::Store(format!("hydration of {heavy} failed: {msg}")));
                }
            }
        }
    }

    let stage = Instant::now();
    let ingested = ingest_dir(store, root, &src, cfg)?;
    timings.ingest_ms += stage.elapsed().as_millis();

    if cfg.snapshots {
        let stage = Instant::now();
        match snapshots::hydrate(
            store,
            &ingested,
            root,
            pattern,
            &src,
            &heavy,
            dest,
            pinned_lockfile_hash.as_ref(),
            cfg,
        ) {
            SnapshotOutcome::Hydrated(h) => {
                timings.snapshot_ms += stage.elapsed().as_millis();
                timings.snapshot_engaged = true;
                timings.snapshot_lookup_ms += h.lookup_ms;
                timings.snapshot_clonefile_ms += h.clonefile_ms;
                timings.snapshot_mode = h.mode;
                timings.v2_cloned += h.cloned_units;
                timings.v2_linked += h.linked_files;
                if let Some(b) = h.build {
                    timings.snapshot_built = true;
                    timings.build_verify_ms += b.verify_ms;
                    timings.build_link_train_ms += b.link_train_ms;
                    timings.build_publish_ms += b.publish_ms;
                }
                let refs = Instant::now();
                let git_dir = claim_snapshot_references(store, dest, &ingested, h.hash)?;
                timings.references_ms += refs.elapsed().as_millis();
                if !cfg.json {
                    println!(
                        "hydrated {heavy} from {} via snapshot {} (one clone, {} file{})",
                        src.display(),
                        &h.hash.to_string()[..12],
                        h.files,
                        if h.files == 1 { "" } else { "s" },
                    );
                }
                return Ok(DirOutcome {
                    git_dir,
                    snapshot_hash: Some(h.hash),
                    ingested,
                    ladder: None,
                });
            }
            SnapshotOutcome::FellBack(Some(reason)) => {
                eprintln!("wt-snapshots: {heavy}: falling back to per-file placement ({reason})");
            }
            SnapshotOutcome::FellBack(None) => {}
            SnapshotOutcome::Failed(msg) => {
                return Err(Error::Store(format!("hydration of {heavy} failed: {msg}")));
            }
        }
        // Fell through to the per-file ladder below; its cost is
        // counted by the stages it runs itself.
    }

    let stage = Instant::now();
    let git_dir = claim_references(store, dest, &ingested)?;
    timings.references_ms += stage.elapsed().as_millis();
    // Ingested paths are repo-relative (they include the heavy
    // directory itself), so materialize against the worktree root.
    let stage = Instant::now();
    let report = materialize(store, &ingested, dest, cfg)
        .map_err(|e| Error::Store(format!("hydration of {} failed: {e}", rel.display())))?;
    timings.materialize_ms += stage.elapsed().as_millis();
    timings.verify_ms += report.verify_ms;
    timings.place_ms += report.place_ms;
    if !cfg.json {
        println!(
            "hydrated {} from {} via store ({} file{})",
            rel.display(),
            src.display(),
            report.files,
            if report.files == 1 { "" } else { "s" }
        );
    }
    Ok(DirOutcome {
        git_dir,
        snapshot_hash: None,
        ingested,
        ladder: Some(report),
    })
}
