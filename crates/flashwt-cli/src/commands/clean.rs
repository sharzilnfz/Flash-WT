use std::collections::HashSet;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::config::RunConfig;
use crate::envelope::{CleanData, Diagnostic};
use crate::error::{Error, Result};
use crate::gc;
use crate::output::HumanBytes;
use crate::workspace::WorkspaceEngine;

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

    if let Some(branch_name) = name {
        return clean_single_worktree(&engine, branch_name, dir, force, age, cfg);
    }

    clean_batch_worktrees(&engine, all, force, age, cfg)
}

fn clean_single_worktree(
    engine: &WorkspaceEngine,
    name: &str,
    dir: Option<&Path>,
    force: bool,
    age: Option<Duration>,
    cfg: &RunConfig,
) -> Result<(CleanData, Vec<Diagnostic>)> {
    let dest = match dir {
        Some(d) => d.to_path_buf(),
        None => engine.default_dest(name)?,
    };

    if dest.exists() {
        let is_dirty = engine.is_worktree_dirty(&dest);
        let is_merged = engine.is_branch_merged(name);
        if !force && (is_dirty || !is_merged) {
            let reason = if is_dirty && !is_merged {
                "worktree has uncommitted changes and unmerged commits"
            } else if is_dirty {
                "worktree has uncommitted changes"
            } else {
                "branch is not merged into HEAD"
            };
            return Err(Error::Usage(format!(
                "refusing to remove {} ({reason}); use --force to override",
                dest.display()
            )));
        }
    }

    let mut diagnostics = Vec::new();
    let mut silent_cfg = *cfg;
    silent_cfg.json = true;

    let (remove_data, mut rm_diags) = gc::remove(name, dir, &silent_cfg)?;
    diagnostics.append(&mut rm_diags);

    if dest.exists() {
        return Err(Error::Store(format!(
            "worktree {} still exists after removal",
            dest.display()
        )));
    }

    let (sweep_data, mut sw_diags) = gc::sweep(age, false, &silent_cfg)?;
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
                HumanBytes(reclaimed_bytes),
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
    let candidates: Vec<CleanCandidate> =
        all_candidates.into_iter().filter(|c| !c.is_main).collect();

    if candidates.is_empty() {
        if !cfg.json {
            println!("No linked worktrees found to clean.");
        }
        return Ok((CleanData::default(), Vec::new()));
    }

    let eligible: Vec<CleanCandidate> = if force {
        candidates.clone()
    } else {
        candidates
            .iter()
            .filter(|c| c.is_merged && !engine.is_worktree_dirty(&c.path))
            .cloned()
            .collect()
    };

    let selected_candidates: Vec<CleanCandidate> = if all {
        eligible
    } else if io::stdin().is_terminal() && !cfg.json && !force {
        println!("\nActive worktrees available for cleanup:");
        for (i, c) in candidates.iter().enumerate() {
            let dirty = engine.is_worktree_dirty(&c.path);
            let status = if dirty {
                "[dirty - uncommitted changes]"
            } else if c.is_merged {
                "[merged into HEAD - recommended]"
            } else {
                "[unmerged changes/commits]"
            };
            let check = if c.is_merged && !dirty { "[x]" } else { "[ ]" };
            println!(
                "  {} {}. {} ({}) {}",
                check,
                i + 1,
                c.path.display(),
                c.branch,
                status
            );
        }
        print!(
            "\nEnter worktree numbers to delete (e.g. '1,2', 'all', Enter for pre-selected [x], 'q' to cancel): "
        );
        io::stdout()
            .flush()
            .map_err(|e| Error::io("flush stdout", root, e))?;

        let mut input = String::new();
        let stdin = io::stdin();
        stdin
            .lock()
            .read_line(&mut input)
            .map_err(|e| Error::io("read stdin", root, e))?;
        let trimmed = input.trim().to_lowercase();

        if trimmed == "q" || trimmed == "quit" || trimmed == "n" || trimmed == "no" {
            println!("Cleanup cancelled.");
            return Ok((CleanData::default(), Vec::new()));
        }

        if trimmed.is_empty() || trimmed == "all" {
            eligible
        } else {
            let mut indices = HashSet::new();
            for part in trimmed.split([',', ' ']) {
                if let Ok(num) = part.trim().parse::<usize>() {
                    if num > 0 && num <= candidates.len() {
                        indices.insert(num - 1);
                    }
                }
            }
            let chosen: Vec<CleanCandidate> = candidates
                .into_iter()
                .enumerate()
                .filter(|(idx, _)| indices.contains(idx))
                .map(|(_, c)| c)
                .collect();
            if force {
                chosen
            } else {
                chosen
                    .into_iter()
                    .filter(|c| c.is_merged && !engine.is_worktree_dirty(&c.path))
                    .collect()
            }
        }
    } else {
        if eligible.is_empty() && !force {
            if !cfg.json {
                println!(
                    "No merged worktrees found to clean. Use 'flashwt clean <name>' or 'flashwt clean --all --force' to include unmerged/dirty."
                );
            }
            return Ok((CleanData::default(), Vec::new()));
        }
        eligible
    };

    if selected_candidates.is_empty() {
        if !cfg.json {
            println!("No worktrees selected for removal.");
        }
        return Ok((CleanData::default(), Vec::new()));
    }

    let mut diagnostics = Vec::new();
    let mut removed_worktrees = Vec::new();
    let mut branches_removed = Vec::new();
    let mut references_released = 0usize;
    let mut mirrors_removed = 0usize;

    let mut silent_cfg = *cfg;
    silent_cfg.json = true;

    for candidate in &selected_candidates {
        match gc::remove(&candidate.branch, Some(&candidate.path), &silent_cfg) {
            Ok((rm_data, mut rm_diags)) => {
                diagnostics.append(&mut rm_diags);
                if candidate.path.exists() {
                    diagnostics.push(Diagnostic::error(
                        "REMOVE_FAILED",
                        format!(
                            "worktree {} still exists after removal",
                            candidate.path.display()
                        ),
                    ));
                    continue;
                }
                references_released += rm_data.references_released;
                if rm_data.mirror_removed {
                    mirrors_removed += 1;
                }
                removed_worktrees.push(candidate.path.display().to_string());
                branches_removed.push(candidate.branch.clone());
                if !cfg.json {
                    println!(
                        "✓ Removed worktree {} ({})",
                        candidate.path.display(),
                        candidate.branch
                    );
                }
            }
            Err(e) => {
                diagnostics.push(Diagnostic::error("REMOVE_FAILED", e.to_string()));
            }
        }
    }

    let (sweep_data, mut sw_diags) = gc::sweep(age, false, &silent_cfg)?;
    diagnostics.append(&mut sw_diags);

    let reclaimed_bytes = sweep_data.lease_bytes_reclaimed.unwrap_or(0);

    if !cfg.json {
        if references_released > 0 {
            println!("✓ Released {references_released} references");
        }
        if sweep_data.reclaimed > 0 || reclaimed_bytes > 0 {
            println!(
                "✓ Reclaimed {} disk space ({} store objects swept)",
                HumanBytes(reclaimed_bytes),
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
