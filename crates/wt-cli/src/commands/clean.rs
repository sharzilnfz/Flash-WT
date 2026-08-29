//! `wt clean`: Unified worktree removal, merged branch cleanup, and store garbage collection
//! (ticket 03 & ticket 05).

use std::collections::HashSet;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::config::RunConfig;
use crate::envelope::{CleanData, Diagnostic};
use crate::error::{Error, Result};
use crate::gc;
use crate::workspace::WorkspaceEngine;

/// Format bytes into human-readable unit (e.g. `1.2 MB`, `450 KB`).
fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

/// A discovered worktree candidate for cleanup.
#[derive(Debug, Clone)]
struct CleanCandidate {
    path: PathBuf,
    branch: String,
    is_merged: bool,
    is_main: bool,
}

pub fn run(
    name: Option<&str>,
    dir: Option<&Path>,
    all: bool,
    force: bool,
    age: Option<Duration>,
    cfg: &RunConfig,
) -> Result<(CleanData, Vec<Diagnostic>)> {
    let engine = WorkspaceEngine::discover()?;

    // Single worktree targeted cleanup
    if let Some(branch_name) = name {
        return clean_single_worktree(&engine, branch_name, dir, age, cfg);
    }

    // Interactive or batch cleanup
    clean_batch_worktrees(&engine, all, force, age, cfg)
}

fn clean_single_worktree(
    engine: &WorkspaceEngine,
    name: &str,
    dir: Option<&Path>,
    age: Option<Duration>,
    cfg: &RunConfig,
) -> Result<(CleanData, Vec<Diagnostic>)> {
    let dest = match dir {
        Some(d) => d.to_path_buf(),
        None => engine.default_dest(name)?,
    };

    let mut diagnostics = Vec::new();
    let mut silent_cfg = *cfg;
    silent_cfg.json = true;

    let (remove_data, mut rm_diags) = gc::remove(name, dir, &silent_cfg)?;
    diagnostics.append(&mut rm_diags);

    // If directory still exists on disk, remove git worktree tracking
    engine.remove_worktree_lenient(&dest);

    // Automatically invoke GC sweep to reclaim unreferenced blobs/snapshots
    let (sweep_data, mut sw_diags) = gc::sweep(age, &silent_cfg)?;
    diagnostics.append(&mut sw_diags);

    let reclaimed_bytes = sweep_data.lease_bytes_reclaimed.unwrap_or(0);

    if !cfg.json {
        println!("✓ Removed worktree {} ({name})", dest.display());
        if remove_data.references_released > 0 {
            println!("✓ Released {} references", remove_data.references_released);
        }
        if sweep_data.reclaimed > 0 || reclaimed_bytes > 0 {
            println!(
                "✓ Reclaimed {} disk space ({} store objects swept)",
                format_bytes(reclaimed_bytes),
                sweep_data.reclaimed
            );
        } else {
            println!("✓ Store clean (0 unreferenced objects swept)");
        }
    }

    let data = CleanData {
        removed_worktrees: vec![dest.display().to_string()],
        branches_removed: vec![name.to_string()],
        references_released: remove_data.references_released,
        mirrors_removed: if remove_data.mirror_removed { 1 } else { 0 },
        reclaimed_bytes,
        sweep_examined: sweep_data.examined,
        sweep_reclaimed: sweep_data.reclaimed,
    };

    Ok((data, diagnostics))
}

fn discover_candidates(engine: &WorkspaceEngine) -> Result<Vec<CleanCandidate>> {
    let mut candidates = Vec::new();
    let mut is_first = true;

    for md in engine.worktree_metadata()? {
        let branch = md.raw.branch.clone().unwrap_or_default();
        let is_merged = engine.is_branch_merged(&branch);

        candidates.push(CleanCandidate {
            path: md.raw.path,
            branch,
            is_merged,
            is_main: is_first,
        });
        is_first = false;
    }

    Ok(candidates)
}

fn clean_batch_worktrees(
    engine: &WorkspaceEngine,
    all: bool,
    force: bool,
    age: Option<Duration>,
    cfg: &RunConfig,
) -> Result<(CleanData, Vec<Diagnostic>)> {
    let root = engine.root();
    let all_candidates = discover_candidates(engine)?;
    let candidates: Vec<CleanCandidate> = all_candidates
        .into_iter()
        .filter(|c| !c.is_main)
        .collect();

    if candidates.is_empty() {
        if !cfg.json {
            println!("No linked worktrees found to clean.");
        }
        return Ok((
            CleanData {
                removed_worktrees: Vec::new(),
                branches_removed: Vec::new(),
                references_released: 0,
                mirrors_removed: 0,
                reclaimed_bytes: 0,
                sweep_examined: 0,
                sweep_reclaimed: 0,
            },
            Vec::new(),
        ));
    }

    let selected_candidates: Vec<CleanCandidate> = if all {
        candidates
    } else if io::stdin().is_terminal() && !cfg.json && !force {
        // Interactive TTY multi-select prompt (ticket 05)
        println!("\nActive worktrees available for cleanup:");
        for (i, c) in candidates.iter().enumerate() {
            let status = if c.is_merged {
                "[merged into HEAD - recommended]"
            } else {
                "[unmerged changes/commits]"
            };
            let check = if c.is_merged { "[x]" } else { "[ ]" };
            println!("  {} {}. {} ({}) {}", check, i + 1, c.path.display(), c.branch, status);
        }
        print!("\nEnter worktree numbers to delete (e.g. '1,2', 'all', Enter for pre-selected [x], 'q' to cancel): ");
        io::stdout().flush().map_err(|e| Error::io("flush stdout", root, e))?;

        let mut input = String::new();
        let stdin = io::stdin();
        stdin.lock().read_line(&mut input).map_err(|e| Error::io("read stdin", root, e))?;
        let trimmed = input.trim().to_lowercase();

        if trimmed == "q" || trimmed == "quit" || trimmed == "n" || trimmed == "no" {
            println!("Cleanup cancelled.");
            return Ok((
                CleanData {
                    removed_worktrees: Vec::new(),
                    branches_removed: Vec::new(),
                    references_released: 0,
                    mirrors_removed: 0,
                    reclaimed_bytes: 0,
                    sweep_examined: 0,
                    sweep_reclaimed: 0,
                },
                Vec::new(),
            ));
        }

        if trimmed.is_empty() {
            // Default to pre-selected merged
            candidates.into_iter().filter(|c| c.is_merged).collect()
        } else if trimmed == "all" {
            candidates
        } else {
            let mut indices = HashSet::new();
            for part in trimmed.split([',', ' ']) {
                if let Ok(num) = part.trim().parse::<usize>() {
                    if num > 0 && num <= candidates.len() {
                        indices.insert(num - 1);
                    }
                }
            }
            candidates
                .into_iter()
                .enumerate()
                .filter(|(idx, _)| indices.contains(idx))
                .map(|(_, c)| c)
                .collect()
        }
    } else {
        // Non-interactive or piped mode: default to merged worktrees
        let merged: Vec<_> = candidates.into_iter().filter(|c| c.is_merged).collect();
        if merged.is_empty() && !force {
            if !cfg.json {
                println!("No merged worktrees found to clean. Use 'wt clean <name>' or 'wt clean --all'.");
            }
            return Ok((
                CleanData {
                    removed_worktrees: Vec::new(),
                    branches_removed: Vec::new(),
                    references_released: 0,
                    mirrors_removed: 0,
                    reclaimed_bytes: 0,
                    sweep_examined: 0,
                    sweep_reclaimed: 0,
                },
                Vec::new(),
            ));
        }
        merged
    };

    if selected_candidates.is_empty() {
        if !cfg.json {
            println!("No worktrees selected for removal.");
        }
        return Ok((
            CleanData {
                removed_worktrees: Vec::new(),
                branches_removed: Vec::new(),
                references_released: 0,
                mirrors_removed: 0,
                reclaimed_bytes: 0,
                sweep_examined: 0,
                sweep_reclaimed: 0,
            },
            Vec::new(),
        ));
    }

    let mut diagnostics = Vec::new();
    let mut removed_worktrees = Vec::new();
    let mut branches_removed = Vec::new();
    let mut references_released = 0usize;
    let mut mirrors_removed = 0usize;

    let mut silent_cfg = *cfg;
    silent_cfg.json = true;

    for candidate in &selected_candidates {
        if let Ok((rm_data, mut rm_diags)) = gc::remove(&candidate.branch, Some(&candidate.path), &silent_cfg) {
            diagnostics.append(&mut rm_diags);
            references_released += rm_data.references_released;
            if rm_data.mirror_removed {
                mirrors_removed += 1;
            }
        }

        engine.remove_worktree_lenient(&candidate.path);

        removed_worktrees.push(candidate.path.display().to_string());
        branches_removed.push(candidate.branch.clone());

        if !cfg.json {
            println!("✓ Removed worktree {} ({})", candidate.path.display(), candidate.branch);
        }
    }

    // Run sweep once after batch removal
    let (sweep_data, mut sw_diags) = gc::sweep(age, &silent_cfg)?;
    diagnostics.append(&mut sw_diags);

    let reclaimed_bytes = sweep_data.lease_bytes_reclaimed.unwrap_or(0);

    if !cfg.json {
        if references_released > 0 {
            println!("✓ Released {references_released} references");
        }
        if sweep_data.reclaimed > 0 || reclaimed_bytes > 0 {
            println!(
                "✓ Reclaimed {} disk space ({} store objects swept)",
                format_bytes(reclaimed_bytes),
                sweep_data.reclaimed
            );
        }
    }

    let data = CleanData {
        removed_worktrees,
        branches_removed,
        references_released,
        mirrors_removed,
        reclaimed_bytes,
        sweep_examined: sweep_data.examined,
        sweep_reclaimed: sweep_data.reclaimed,
    };

    Ok((data, diagnostics))
}
