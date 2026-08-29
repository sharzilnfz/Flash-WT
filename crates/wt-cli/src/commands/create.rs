//! `wt create`: worktree creation plus manifest-driven hydration
//! (tickets 02 + 05), decomposed from main.rs by arch-hardening
//! ticket 03.

use std::path::Path;
use std::time::Instant;

use crate::config::RunConfig;
use crate::envelope::{CreateData, Diagnostic};
use crate::error::{Error, Result};
use crate::hydrate::{HydrationEngine, HydrationRequest, open_store};
use crate::manifest::{self, LoadedPatterns, load_patterns};
use crate::output::{HumanBytes, HumanCount};
use crate::timing::StageTimings;
use crate::workspace::WorkspaceEngine;

pub fn run(
    name: &str,
    base: Option<&str>,
    manifest: Option<&Path>,
    dir: Option<&Path>,
    cfg: &RunConfig,
) -> Result<(CreateData, Vec<Diagnostic>)> {
    create(name, base, manifest, dir, cfg)
}

fn create(
    name: &str,
    base: Option<&str>,
    manifest: Option<&Path>,
    dir: Option<&Path>,
    cfg: &RunConfig,
) -> Result<(CreateData, Vec<Diagnostic>)> {
    let engine = WorkspaceEngine::discover()?;
    let root = engine.root().to_path_buf();
    let dest = match dir {
        Some(d) => d.to_path_buf(),
        None => engine.default_dest(name)?,
    };
    if dest.exists() {
        return Err(Error::Usage(format!("{} already exists", dest.display())));
    }

    let timing_enabled = cfg.timing;
    let mut timings = StageTimings::new();
    let started = Instant::now();
    let start_point = base.unwrap_or("HEAD");
    let base_commit = if let Some(base_ref) = base {
        Some(engine.resolve_commit(base_ref)?)
    } else {
        None
    };

    engine.create_worktree(name, &dest, start_point)?;
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

    let mut store = open_store()?;
    let mut engine = HydrationEngine::new(&mut store);
    let req = HydrationRequest {
        root: &root,
        dest: &dest,
        patterns: &patterns,
        base_branch: base,
        base_commit: base_commit.as_deref(),
        cfg,
    };

    let mut report = engine.hydrate(req)?;
    report.timings.git_worktree_ms = timings.git_worktree_ms;

    if !cfg.json {
        report.timings.emit(started, timing_enabled);

        let total_bytes = report.bytes_shared_cow + report.bytes_copied;
        let total_ms = started.elapsed().as_millis();
        println!("✓ Created worktree {} ({name})", dest.display());
        if report.total_files > 0 {
            println!(
                "✓ Hydrated {} files ({}) via {} in {} ms",
                HumanCount(report.total_files),
                HumanBytes(total_bytes),
                report.hydration_method,
                total_ms
            );
        }
        if let Ok(curr) = std::env::current_dir() {
            if let Ok(rel) = dest.strip_prefix(&curr) {
                println!("  Next: cd {}", rel.display());
            } else if let Some(parent) = dest.parent() {
                if parent == curr.parent().unwrap_or(&curr) {
                    println!(
                        "  Next: cd ../{}",
                        dest.file_name().unwrap_or_default().to_string_lossy()
                    );
                } else {
                    println!("  Next: cd {}", dest.display());
                }
            } else {
                println!("  Next: cd {}", dest.display());
            }
        } else {
            println!("  Next: cd {}", dest.display());
        }
    }

    let data = CreateData {
        worktree_path: dest.display().to_string(),
        branch: name.to_string(),
        cache_hit: report.cache_hit,
        duration_ms: started.elapsed().as_millis() as u64,
        hydration_method: report.hydration_method,
        bytes_shared_cow: report.bytes_shared_cow,
        bytes_copied: report.bytes_copied,
        files_hydrated: report.total_files,
    };

    Ok((data, report.diagnostics))
}
