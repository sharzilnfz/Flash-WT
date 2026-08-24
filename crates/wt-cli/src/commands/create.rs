//! `wt create`: worktree creation plus manifest-driven hydration
//! (tickets 02 + 05), decomposed from main.rs by arch-hardening
//! ticket 03.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use wt_store::{ContentId, DiskStore};

use crate::gitops;
use crate::hydrate::{
    claim_references, claim_snapshot_references, ingest_dir, materialize, open_store,
    publish_mirror, snapshots_enabled, Ingested, MaterializeReport,
};
use crate::manifest::{self, collect_matches, load_patterns, pattern_matches, LoadedPatterns};
use crate::snapshots;
use crate::snapshots::Outcome as SnapshotOutcome;
use crate::timing::StageTimings;

pub fn run(name: &str, manifest: Option<&Path>, dir: Option<&Path>) -> Result<(), String> {
    create(name, manifest, dir)
}

fn create(name: &str, manifest: Option<&Path>, dir: Option<&Path>) -> Result<(), String> {
    let root = gitops::repo_root()?;
    let dest = match dir {
        Some(d) => d.to_path_buf(),
        None => gitops::default_worktree_dest(&root, name)?,
    };
    if dest.exists() {
        return Err(format!("{} already exists", dest.display()));
    }

    let timing_enabled = std::env::var_os("WT_TIMING").is_some();
    let mut timings = StageTimings::new();
    let started = Instant::now();
    // Prefer creating the branch from HEAD; an existing branch falls
    // back to checking it out directly.
    let dest_text = dest.to_string_lossy().into_owned();
    gitops::run(&root, &["worktree", "add", "-b", name, &dest_text, "HEAD"])
        .or_else(|_| gitops::run(&root, &["worktree", "add", &dest_text, name]))?;
    timings.git_worktree_ms = started.elapsed().as_millis();

    println!(
        "created worktree {} from {}",
        dest.display(),
        root.display()
    );

    let patterns = match load_patterns(&root, manifest)? {
        LoadedPatterns::CreatedStarter { path, patterns } => {
            println!(
                "no .wtinclude in {}; using defaults ({})",
                root.display(),
                manifest::DEFAULT_PATTERNS.join(" ")
            );
            println!("wrote starter manifest {}", path.display());
            patterns
        }
        LoadedPatterns::Loaded { patterns } => patterns,
    };
    let dirs = collect_matches(&root, &patterns)?;
    if dirs.is_empty() {
        println!("nothing to hydrate");
        timings.emit(started, timing_enabled);
        return Ok(());
    }

    let mut store = open_store()?;
    let paranoid = std::env::var_os("WT_VERIFY").is_some();
    let snapshot_gate = snapshots_enabled();
    let mut total_files = 0usize;
    let mut total_copied = 0usize;
    let mut strategy = "byte-copy";
    let mut combined = Ingested {
        dirs: Vec::new(),
        files: BTreeMap::new(),
        symlinks: BTreeMap::new(),
        modes: BTreeMap::new(),
    };
    // Ticket 08: heavy directories hydrated through snapshots record
    // their manifest hashes here; the mirror names them, not the
    // child blobs (the manifest marks those).
    let mut snapshot_hashes: Vec<ContentId> = Vec::new();
    let mut git_dir = dest.clone(); // replaced by claim_references
    for rel in &dirs {
        let outcome = hydrate_one_dir(&mut store, &patterns, &root, &dest, rel, paranoid, snapshot_gate, &mut timings)?;
        git_dir = outcome.git_dir;
        if let Some(hash) = outcome.snapshot_hash {
            snapshot_hashes.push(hash);
        }
        match outcome.ladder {
            // Snapshot fast path: the clone carried the whole tree.
            None => total_files += outcome.ingested.files.len(),
            Some(report) => {
                combined.dirs.extend(outcome.ingested.dirs.iter().cloned());
                for (rel, id) in &outcome.ingested.files {
                    combined.files.insert(rel.clone(), *id);
                }
                total_files += report.files;
                total_copied += report.copied;
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

    // Say plainly what happened to shared content.
    if std::env::var_os("WT_NO_HARDLINK").is_some() {
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
    // Persist the verified-blob ledger explicitly: the Drop below is
    // a best-effort backup, but a clean run should leave its
    // verifications behind even if something later fails hard.
    store
        .flush()
        .map_err(|e| format!("cannot update verified-blob ledger: {e}"))?;
    println!(
        "hydration complete: {total_files} file{} through the store",
        {
            if total_files == 1 {
                ""
            } else {
                "s"
            }
        }
    );
    timings.emit(started, timing_enabled);
    Ok(())
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
#[allow(clippy::too_many_arguments)]
fn hydrate_one_dir(
    store: &mut DiskStore,
    patterns: &[String],
    root: &Path,
    dest: &Path,
    rel: &Path,
    paranoid: bool,
    snapshot_gate: bool,
    timings: &mut StageTimings,
) -> Result<DirOutcome, String> {
    let src = root.join(rel);
    let stage = Instant::now();
    let ingested = ingest_dir(store, root, &src)?;
    timings.ingest_ms += stage.elapsed().as_millis();
    let heavy = rel.to_string_lossy().into_owned();

    if snapshot_gate {
        let stage = Instant::now();
        // v2 selection-index key: the first manifest pattern that
        // matched this heavy directory. Only stability across runs
        // matters, not uniqueness.
        let pattern = patterns
            .iter()
            .find(|p| pattern_matches(p, rel))
            .map(String::as_str)
            .unwrap_or("");
        match snapshots::hydrate(
            store, &ingested, root, pattern, &src, &heavy, dest, paranoid,
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
                let git_dir =
                    claim_snapshot_references(store, dest, &ingested, h.hash)?;
                timings.references_ms += refs.elapsed().as_millis();
                println!(
                    "hydrated {heavy} from {} via snapshot {} (one clone, {} file{})",
                    src.display(),
                    &h.hash.to_string()[..12],
                    h.files,
                    if h.files == 1 { "" } else { "s" },
                );
                return Ok(DirOutcome {
                    git_dir,
                    snapshot_hash: Some(h.hash),
                    ingested,
                    ladder: None,
                });
            }
            SnapshotOutcome::FellBack(Some(reason)) => {
                eprintln!(
                    "wt-snapshots: {heavy}: falling back to per-file placement ({reason})"
                );
            }
            SnapshotOutcome::FellBack(None) => {}
            SnapshotOutcome::Failed(msg) => {
                return Err(format!("hydration of {heavy} failed: {msg}"))
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
    let report = materialize(store, &ingested, dest)
        .map_err(|e| format!("hydration of {} failed: {e}", rel.display()))?;
    timings.materialize_ms += stage.elapsed().as_millis();
    timings.verify_ms += report.verify_ms;
    timings.place_ms += report.place_ms;
    println!(
        "hydrated {} from {} via store ({} file{})",
        rel.display(),
        src.display(),
        report.files,
        if report.files == 1 { "" } else { "s" }
    );
    Ok(DirOutcome {
        git_dir,
        snapshot_hash: None,
        ingested,
        ladder: Some(report),
    })
}
