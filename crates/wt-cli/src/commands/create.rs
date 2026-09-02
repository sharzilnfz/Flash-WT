//! `wt create`: worktree creation plus manifest-driven hydration
//! (tickets 02 + 05), decomposed from main.rs by arch-hardening
//! ticket 03.

use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::config::RunConfig;
use crate::envelope::{CreateData, Diagnostic};
use crate::error::{Error, Result};
use crate::hydrate::{HydrationEngine, HydrationRequest, open_store};
use crate::manifest::{self, LoadedPatterns, load_patterns};
use crate::output::{HumanBytes, HumanCount};
use crate::signal;
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

/// RAII guard that rolls back a newly created worktree+branch if
/// hydration fails. Defuse on success. Drop is a safety net for
/// panics; explicit rollback on `Err` also fires immediately so the
/// caller sees the cleanup before returning.
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

    fn rollback(&mut self) {
        if self.defused {
            return;
        }
        self.defused = true;
        signal::rollback_create(&self.name, &self.dest, &self.repo_root);
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

    let patterns = match load_patterns(&root, manifest) {
        Ok(lp) => match lp {
            LoadedPatterns::Defaults { patterns } => {
                if !cfg.json {
                    println!(
                        "no .wtinclude in {}; using defaults ({})",
                        root.display(),
                        manifest::DEFAULT_PATTERNS.join(" ")
                    );
                }
                patterns
            }
            LoadedPatterns::Loaded { patterns, .. } => patterns,
        },
        Err(e) => {
            guard.rollback();
            return Err(e);
        }
    };

    let mut store = match open_store() {
        Ok(s) => s,
        Err(e) => {
            guard.rollback();
            return Err(e);
        }
    };
    let mut h_engine = HydrationEngine::new(&mut store);
    let req = HydrationRequest {
        root: &root,
        dest: &dest,
        patterns: &patterns,
        base_branch: base,
        base_commit: base_commit.as_deref(),
        cfg,
    };

    let mut report = match h_engine.hydrate(req) {
        Ok(r) => r,
        Err(e) => {
            guard.rollback();
            return Err(e);
        }
    };
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
