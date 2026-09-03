//! `wt hydrate`: Standalone hydration into an existing worktree or directory (Ticket 09).

use std::path::Path;
use std::time::Instant;

use crate::config::RunConfig;
use crate::envelope::{Diagnostic, HydrateData};
use crate::error::{Error, Result};
use crate::hydrate::{HydrationEngine, HydrationRequest, open_store};
use crate::hydration_filter::{self, LoadedPatterns, ZeroSavingsReason, load_patterns};
use crate::output::{HumanBytes, HumanCount};
use crate::workspace::{self, WorkspaceEngine};

pub fn run(
    path: &Path,
    source: Option<&Path>,
    manifest: Option<&Path>,
    cfg: &RunConfig,
) -> Result<(HydrateData, Vec<Diagnostic>)> {
    if !path.exists() {
        return Err(Error::Usage(format!(
            "destination path {} does not exist",
            path.display()
        )));
    }
    if !path.is_dir() {
        return Err(Error::Usage(format!(
            "destination path {} is not a directory",
            path.display()
        )));
    }
    let dest = path
        .canonicalize()
        .map_err(|e| Error::io_unanchored("canonicalize destination", path, e))?;

    // Determine source repository root
    let source_root = if let Some(src) = source {
        if !src.exists() {
            return Err(Error::Usage(format!(
                "source directory {} does not exist",
                src.display()
            )));
        }
        src.canonicalize()
            .map_err(|e| Error::io_unanchored("canonicalize source", src, e))?
    } else if let Ok(engine) = WorkspaceEngine::discover() {
        engine.root().to_path_buf()
    } else {
        let gitdir = workspace::resolve_git_dir(&dest);
        if let Some(repo) = workspace::repo_root_from_gitdir(&gitdir) {
            repo
        } else {
            return Err(Error::Usage(
                "could not determine source repository; run inside a git repository or specify --source <path>".into(),
            ));
        }
    };

    let timing_enabled = cfg.timing;
    let started = Instant::now();

    let patterns = match load_patterns(&source_root, manifest)? {
        LoadedPatterns::Defaults { patterns } => {
            if !cfg.json {
                println!(
                    "no .wtinclude in {}; using defaults ({})",
                    source_root.display(),
                    hydration_filter::DEFAULT_PATTERNS.join(" ")
                );
            }
            patterns
        }
        LoadedPatterns::Loaded { patterns, .. } => patterns,
    };

    let mut store = open_store()?;
    let mut engine = HydrationEngine::new(&mut store);
    let req = HydrationRequest {
        root: &source_root,
        dest: &dest,
        patterns: &patterns,
        base_branch: None,
        base_commit: None,
        cfg,
    };

    let report = engine.hydrate(req)?;

    if !cfg.json {
        report.timings.emit(started, timing_enabled);
        let total_bytes = report.bytes_shared_cow + report.bytes_copied;
        let total_ms = started.elapsed().as_millis();
        println!(
            "✓ Hydrated {} ({})",
            dest.display(),
            report.hydration_method
        );
        if report.total_files > 0 {
            println!(
                "✓ Hydrated {} files ({}) via {} in {} ms",
                HumanCount(report.total_files),
                HumanBytes(total_bytes),
                report.hydration_method,
                total_ms
            );
        } else {
            let reason = if report.dirs_hydrated.is_empty() {
                ZeroSavingsReason::NoMatchingDirectories
            } else {
                ZeroSavingsReason::NoFilesHydrated
            };
            println!("  {}", reason.human_summary());
        }
    }

    let data = HydrateData {
        destination_path: dest.display().to_string(),
        source_path: source_root.display().to_string(),
        cache_hit: report.cache_hit,
        duration_ms: started.elapsed().as_millis() as u64,
        hydration_method: report.hydration_method,
        bytes_shared_cow: report.bytes_shared_cow,
        bytes_copied: report.bytes_copied,
        files_hydrated: report.total_files,
        dirs_hydrated: report
            .dirs_hydrated
            .into_iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect(),
    };

    Ok((data, report.diagnostics))
}
