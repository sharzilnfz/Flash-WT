//! `wt create`: worktree creation plus manifest-driven hydration
//! (tickets 02 + 05), decomposed from main.rs by arch-hardening
//! ticket 03.

use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::config::RunConfig;
use crate::envelope::{CreateData, Diagnostic};
use crate::error::{Error, Result};
use crate::hydrate::{HydrationEngine, HydrationRequest};
use crate::hydration_filter::{self, LoadedPatterns, ZeroSavingsReason, load_patterns};
use crate::output::{HumanBytes, HumanCount};
use crate::signal;
use crate::timing::StageTimings;
use crate::workspace::WorkspaceEngine;

/// RAII guard that rolls back a newly created worktree+branch if
/// hydration fails. Defuse on success.
struct CreateGuard {
    name: String,
    dest: PathBuf,
    repo_root: PathBuf,
    defused: bool,
}

impl CreateGuard {
    fn defuse(&mut self) {
        self.defused = true;
        signal::clear_create();
    }
}

impl Drop for CreateGuard {
    fn drop(&mut self) {
        if !self.defused {
            signal::rollback_create(&self.name, &self.dest, &self.repo_root);
            signal::clear_create();
        }
    }
}

pub fn run(
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

    // Register for signal-driven cleanup and arm RAII guard for
    // transactional rollback on any subsequent failure.
    signal::register_create(signal::ActiveCreate {
        name: name.to_string(),
        dest: dest.clone(),
        repo_root: root.clone(),
    });
    let mut guard = CreateGuard {
        name: name.to_string(),
        dest: dest.clone(),
        repo_root: root.clone(),
        defused: false,
    };

    if !cfg.json {
        println!(
            "created worktree {} from {}",
            dest.display(),
            root.display()
        );
    }

    let lp = load_patterns(&root, manifest)?;
    let patterns = match lp {
        LoadedPatterns::Defaults { patterns } => {
            if !cfg.json {
                println!(
                    "no .wtinclude in {}; using defaults ({})",
                    root.display(),
                    hydration_filter::DEFAULT_PATTERNS.join(" ")
                );
            }
            patterns
        }
        LoadedPatterns::Loaded { patterns, .. } => patterns,
    };

    let mut h_engine = HydrationEngine::auto();
    let req = HydrationRequest {
        root: &root,
        dest: &dest,
        patterns: &patterns,
        base_branch: base,
        base_commit: base_commit.as_deref(),
        cfg,
    };

    let mut report = h_engine.hydrate(req)?;
    report.timings.git_worktree_ms = timings.git_worktree_ms;

    // Success: defuse guard so Drop does not delete the worktree.
    guard.defuse();

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
            if report.total_copied > 0 || report.hydration_method == "byte_copy" {
                if let Some(refusal) = &report.refusal_reason {
                    println!("  Copy mechanism: byte-copy (acceleration refused: {refusal})");
                }
            }
        } else {
            let reason = if report.dirs_hydrated.is_empty() {
                ZeroSavingsReason::NoMatchingDirectories
            } else {
                ZeroSavingsReason::NoFilesHydrated
            };
            println!("  {}", reason.human_summary());
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

    let (copy_mechanism, copy_fallback_reason) =
        if report.total_copied > 0 || report.hydration_method == "byte_copy" {
            (
                Some(
                    report
                        .copy_backend
                        .clone()
                        .unwrap_or_else(|| "byte-copy".to_string()),
                ),
                report.refusal_reason.clone(),
            )
        } else {
            (None, None)
        };

    let data = CreateData {
        worktree_path: dest.display().to_string(),
        branch: name.to_string(),
        cache_hit: report.cache_hit,
        duration_ms: started.elapsed().as_millis() as u64,
        hydration_method: report.hydration_method,
        bytes_shared_cow: report.bytes_shared_cow,
        bytes_copied: report.bytes_copied,
        files_hydrated: report.total_files,
        incremental_decision: report.incremental_decision,
        incremental_fallback_reason: report.incremental_fallback_reason,
        copy_mechanism,
        copy_fallback_reason,
    };

    Ok((data, report.diagnostics))
}
