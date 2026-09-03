//! Garbage collection (ticket 06 + fast-hydration ticket 07):
//! `wt remove` releases a worktree's references and retires its
//! store-local mirror, `wt sweep --age <duration>` reclaims store
//! entries past the age threshold.
//!
//! Two collection schemes coexist behind `<store>/gc-mode`:
//!
//! - legacy (default): liveness from `refs/` refcounts exactly as
//!   ticket 06 shipped it. Mirrors are written beside it (dual-write)
//!   and every sweep runs a mark-vs-refs audit; disagreements print
//!   to stderr as `wt-gc-audit:` lines, agreement stays silent.
//! - mark-sweep / mark-sweep-no-refs: liveness from live-mirror
//!   marks plus a grace period (`WT_GC_GRACE`, default 15 minutes;
//!   an explicit `--age` overrides). Refs are ignored for liveness,
//!   and in `-no-refs` mode create/remove no longer touch them at
//!   all. See ADR-0004 for why the cutover is explicit.
//!
//! Removal parses the hydration ledger (`wt-hydrated.tsv`, written by
//! ticket 05 into the worktree's git dir) while the git dir still exists,
//! holds blob ids in memory, releases one reference per distinct blob and
//! removes the store mirror and ledger, then hands the directory to
//! `git worktree remove`. Parsing before removal matters because git removal
//! deletes the git dir with the ledger along with it. A release that hits
//! zero is tolerated as "already released" so an interrupted remove can
//! simply be rerun.

use std::fs;
use std::path::Path;
use std::time::Duration;

use wt_store::{GcMode, PendingCleanup, StoreReclaimer, SweepPolicy};

use crate::config::RunConfig;
use crate::envelope::{Diagnostic, MigrateData, RemoveData, SweepData};
use crate::error::{Error, Result};
use crate::hydrate::open_store;
use crate::workspace::{WorkspaceEngine, git_dir, repo_root_from_gitdir};

/// Default grace period when `WT_GC_GRACE` is unset or unreadable.
const DEFAULT_GRACE: Duration = Duration::from_secs(15 * 60);

/// Default retention cap for unreferenced snapshots (product-handoff
/// §7.4): generous by design — the cap only stops unbounded growth,
/// it does not manage a working set.
const DEFAULT_SNAPSHOT_CAP: usize = 64;

/// Parse a plain duration like `0s`, `90s`, `10m`, `24h`, `7d`.
pub fn parse_age(text: &str) -> Option<Duration> {
    let split = text.find(|c: char| c.is_alphabetic())?;
    let (num, unit) = text.split_at(split);
    let secs = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3_600,
        "d" => 86_400,
        _ => return None,
    };
    let count: u64 = num.parse().ok()?;
    // A huge count must wrap into "invalid", never into a tiny
    // duration that would let sweep collect young content.
    Some(Duration::from_secs(count.checked_mul(secs)?))
}

/// Grace period for mark-and-sweep modes: `WT_GC_GRACE` if it parses
/// (e.g. `15m`, `1h`), else fifteen minutes.
pub fn grace_from_env() -> Result<Duration> {
    match std::env::var("WT_GC_GRACE") {
        Ok(text) => parse_age(&text)
            .ok_or_else(|| Error::Usage(format!("invalid WT_GC_GRACE {text:?} (try 15m, 1h, 7d)"))),
        Err(_) => Ok(DEFAULT_GRACE),
    }
}

/// Retention cap for unreferenced snapshots: `WT_SNAPSHOT_CAP` if it
/// parses as a plain non-negative integer, else
/// [`DEFAULT_SNAPSHOT_CAP`]. Zero is legal and means every aged-out
/// unreferenced snapshot goes at the next sweep; the grace period
/// still applies first.
pub fn snapshot_cap_from_env() -> Result<usize> {
    match std::env::var("WT_SNAPSHOT_CAP") {
        Ok(text) => text
            .parse::<usize>()
            .map_err(|_| Error::Usage(format!("invalid WT_SNAPSHOT_CAP {text:?} (try 64, 128)"))),
        Err(_) => Ok(DEFAULT_SNAPSHOT_CAP),
    }
}

/// Parse a size string like `1048576`, `100KB`, `500MB`, `20GB`, `1TB`, `100KiB`, etc.
pub fn parse_bytes(text: &str) -> Option<u64> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(bytes) = trimmed.parse::<u64>() {
        return Some(bytes);
    }
    let split = trimmed.find(|c: char| c.is_alphabetic())?;
    let (num, unit) = trimmed.split_at(split);
    let count: u64 = num.trim().parse().ok()?;
    let unit_lower = unit.trim().to_lowercase();
    let multiplier: u64 = match unit_lower.as_str() {
        "b" | "byte" | "bytes" => 1,
        "k" | "kb" | "kib" => 1024,
        "m" | "mb" | "mib" => 1024 * 1024,
        "g" | "gb" | "gib" => 1024 * 1024 * 1024,
        "t" | "tb" | "tib" => 1024 * 1024 * 1024 * 1024,
        _ => return None,
    };
    count.checked_mul(multiplier)
}

/// Maximum disk byte budget for unreferenced snapshots: `WT_MAX_SNAPSHOT_BYTES` if set
/// and valid (e.g. `20GB`, `500MB`, `1048576`), else `None`.
pub fn max_snapshot_bytes_from_env() -> Result<Option<u64>> {
    match std::env::var("WT_MAX_SNAPSHOT_BYTES") {
        Ok(text) => parse_bytes(&text).map(Some).ok_or_else(|| {
            Error::Usage(format!(
                "invalid WT_MAX_SNAPSHOT_BYTES {text:?} (try 20GB, 500MB)"
            ))
        }),
        Err(_) => Ok(None),
    }
}

pub fn remove(
    name: &str,
    dir: Option<&Path>,
    cfg: &RunConfig,
) -> Result<(RemoveData, Vec<Diagnostic>)> {
    let engine = WorkspaceEngine::discover()?;
    let root = engine.root().to_path_buf();

    let dest = match dir {
        Some(d) => d.to_path_buf(),
        None => engine.default_dest(name)?,
    };
    if !dest.join(".git").exists() {
        return Err(Error::Usage(format!(
            "{} is not a worktree",
            dest.display()
        )));
    }

    // The linked git dir holds the ledger. Resolve it while the
    // worktree still exists; for linked worktrees this lands inside
    // the main repo's .git/worktrees/<name>.
    let git_dir = git_dir(&dest)?;

    let mut store = open_store()?;

    let mut diagnostics = Vec::new();
    if let Some(diag) = crate::base::check_worktree_base_movement(&store, &root, &dest, &git_dir) {
        if !cfg.json {
            eprintln!("wt: warning: {}", diag.message);
        }
        diagnostics.push(diag);
    }

    let mut reclaimer = StoreReclaimer::new(&mut store);
    let receipt = reclaimer.retire_worktree(&dest, &git_dir)?;

    remove_worktree_files(&engine, &dest)?;

    if !cfg.json {
        println!(
            "removed worktree {}; released {} reference{}",
            dest.display(),
            receipt.references_released,
            if receipt.references_released == 1 {
                ""
            } else {
                "s"
            }
        );
    }

    let data = RemoveData {
        worktree_path: dest.display().to_string(),
        branch: name.to_string(),
        references_released: receipt.references_released,
        mirror_removed: receipt.mirror_removed,
    };

    Ok((data, diagnostics))
}

fn dir_size(path: &Path) -> u64 {
    let mut total = 0;
    if let Ok(meta) = fs::symlink_metadata(path) {
        if meta.is_file() {
            total += meta.len();
        } else if meta.is_dir() {
            if let Ok(entries) = fs::read_dir(path) {
                for entry in entries.flatten() {
                    total += dir_size(&entry.path());
                }
            }
        }
    }
    total
}

/// CLI-owned git removal for `remove`: `git worktree remove --force`,
/// then `rm -rf` fallback. Git errors are ignored when the directory is
/// gone; a surviving directory is an error.
fn remove_worktree_files(engine: &WorkspaceEngine, worktree: &Path) -> Result<()> {
    let _ = engine.remove_worktree_force(worktree);
    if worktree.exists() {
        fs::remove_dir_all(worktree).map_err(|e| Error::Store(e.to_string()))?;
    }
    if worktree.exists() {
        return Err(Error::Store(format!(
            "worktree {} still exists after removal",
            worktree.display()
        )));
    }
    let _ = engine.git(&["worktree", "prune"]);
    Ok(())
}

/// CLI-owned cleanup for one reaped lease. Best-effort: sweep already
/// unclaimed the store metadata, so git and filesystem failures are
/// swallowed and the next sweep retries nothing.
fn cleanup_pending(pending: &PendingCleanup) {
    if let Some(root) = repo_root_from_gitdir(&pending.gitdir) {
        let engine = WorkspaceEngine::from_root(root);
        if pending.worktree.exists() {
            let _ = engine.git(&[
                "worktree",
                "remove",
                "--force",
                &pending.worktree.to_string_lossy(),
            ]);
        }
        let _ = engine.git(&["worktree", "prune"]);
        let _ = engine.delete_branch(&pending.branch);
    }
    if pending.worktree.exists() {
        let _ = fs::remove_dir_all(&pending.worktree);
    }
    if pending.gitdir.exists() {
        let _ = fs::remove_dir_all(&pending.gitdir);
    }
}

pub fn sweep(
    age: Option<Duration>,
    dry_run: bool,
    cfg: &RunConfig,
) -> Result<(SweepData, Vec<Diagnostic>)> {
    let mut store = open_store()?;
    let mode = store.gc_mode();
    let mut reclaimer = StoreReclaimer::new(&mut store);

    let policy = match mode {
        GcMode::Legacy => {
            let max_age = age.unwrap_or(Duration::from_secs(7 * 24 * 60 * 60));
            SweepPolicy {
                grace: max_age,
                snapshot_cap: DEFAULT_SNAPSHOT_CAP,
                max_snapshot_bytes: None,
                dry_run,
            }
        }
        GcMode::MarkSweep | GcMode::MarkSweepNoRefs => {
            let grace = match age {
                Some(explicit) => explicit,
                None => grace_from_env()?,
            };
            let snapshot_cap = snapshot_cap_from_env()?;
            let max_snapshot_bytes = max_snapshot_bytes_from_env()?;
            SweepPolicy {
                grace,
                snapshot_cap,
                max_snapshot_bytes,
                dry_run,
            }
        }
    };

    let (leases_examined, leases_reclaimed, pending) = reclaimer.sweep_leases(&policy)?;

    let mut lease_bytes_reclaimed = 0u64;
    for item in &pending {
        if item.worktree.exists() {
            lease_bytes_reclaimed += dir_size(&item.worktree);
        }
    }
    if !dry_run {
        for item in &pending {
            cleanup_pending(item);
        }
    }
    let mut summary = reclaimer.sweep_objects(&policy)?;
    summary.leases_examined = leases_examined;
    summary.leases_reclaimed = leases_reclaimed;
    summary.lease_bytes_reclaimed = lease_bytes_reclaimed;

    let total_reclaimed_bytes = summary.reclaimed_blob_bytes + summary.lease_bytes_reclaimed;

    match summary.mode {
        GcMode::Legacy => {
            for line in &summary.audit_disagreements {
                eprintln!("{line}");
            }
            if !cfg.json {
                if dry_run {
                    println!(
                        "dry run: would reclaim {} unreferenced blob{} ({} bytes), {} dead lease{} ({} bytes)",
                        summary.reclaimed_blobs,
                        if summary.reclaimed_blobs == 1 {
                            ""
                        } else {
                            "s"
                        },
                        summary.reclaimed_blob_bytes,
                        summary.leases_reclaimed,
                        if summary.leases_reclaimed == 1 {
                            ""
                        } else {
                            "s"
                        },
                        summary.lease_bytes_reclaimed,
                    );
                } else if summary.leases_reclaimed > 0 {
                    println!(
                        "swept store: examined {}, reclaimed {} entr{}, reclaimed {} lease{} ({} bytes)",
                        summary.examined_blobs,
                        summary.reclaimed_blobs,
                        if summary.reclaimed_blobs == 1 {
                            "y"
                        } else {
                            "ies"
                        },
                        summary.leases_reclaimed,
                        if summary.leases_reclaimed == 1 {
                            ""
                        } else {
                            "s"
                        },
                        summary.lease_bytes_reclaimed,
                    );
                } else {
                    println!(
                        "swept store: examined {}, reclaimed {} entr{}",
                        summary.examined_blobs,
                        summary.reclaimed_blobs,
                        if summary.reclaimed_blobs == 1 {
                            "y"
                        } else {
                            "ies"
                        }
                    );
                }
            }
            let data = SweepData {
                mode: "legacy".to_string(),
                examined: summary.examined_blobs as usize,
                reclaimed: summary.reclaimed_blobs as usize,
                mirrors_removed: None,
                snapshot_dirs_removed: None,
                snapshot_cap_evicted: None,
                deferred_by_grace: None,
                leases_examined: Some(summary.leases_examined),
                leases_reclaimed: Some(summary.leases_reclaimed),
                lease_bytes_reclaimed: Some(summary.lease_bytes_reclaimed),
                dry_run: if dry_run { Some(true) } else { None },
                unreferenced_blobs: if dry_run {
                    Some(summary.reclaimed_blobs as usize)
                } else {
                    None
                },
                dead_leases: if dry_run {
                    Some(summary.leases_reclaimed)
                } else {
                    None
                },
                reclaimed_bytes: if dry_run {
                    Some(total_reclaimed_bytes)
                } else {
                    None
                },
            };
            Ok((data, Vec::new()))
        }
        GcMode::MarkSweep | GcMode::MarkSweepNoRefs => {
            if summary.deferred_by_grace {
                eprintln!(
                    "wt-gc-audit: malformed young mirror deferred this sweep; rerun after the grace period"
                );
            }
            if !cfg.json {
                if dry_run {
                    println!(
                        "dry run: would reclaim {} unreferenced blob{} ({} bytes), {} dead lease{} ({} bytes)",
                        summary.reclaimed_blobs,
                        if summary.reclaimed_blobs == 1 {
                            ""
                        } else {
                            "s"
                        },
                        summary.reclaimed_blob_bytes,
                        summary.leases_reclaimed,
                        if summary.leases_reclaimed == 1 {
                            ""
                        } else {
                            "s"
                        },
                        summary.lease_bytes_reclaimed,
                    );
                } else if summary.leases_reclaimed > 0 {
                    println!(
                        "swept store (mark-and-sweep): examined {}, reclaimed {}, mirrors removed {}, snapshots removed {}, cap evicted {}, reclaimed {} lease{} ({} bytes)",
                        summary.examined_blobs,
                        summary.reclaimed_blobs,
                        summary.mirrors_removed,
                        summary.snapshot_dirs_removed,
                        summary.snapshot_cap_evicted,
                        summary.leases_reclaimed,
                        if summary.leases_reclaimed == 1 {
                            ""
                        } else {
                            "s"
                        },
                        summary.lease_bytes_reclaimed,
                    );
                } else {
                    println!(
                        "swept store (mark-and-sweep): examined {}, reclaimed {}, mirrors removed {}, snapshots removed {}, cap evicted {}",
                        summary.examined_blobs,
                        summary.reclaimed_blobs,
                        summary.mirrors_removed,
                        summary.snapshot_dirs_removed,
                        summary.snapshot_cap_evicted,
                    );
                }
            }
            let data = SweepData {
                mode: "mark-sweep".to_string(),
                examined: summary.examined_blobs as usize,
                reclaimed: summary.reclaimed_blobs as usize,
                mirrors_removed: Some(summary.mirrors_removed as usize),
                snapshot_dirs_removed: Some(summary.snapshot_dirs_removed as usize),
                snapshot_cap_evicted: Some(summary.snapshot_cap_evicted as usize),
                deferred_by_grace: Some(summary.deferred_by_grace),
                leases_examined: Some(summary.leases_examined),
                leases_reclaimed: Some(summary.leases_reclaimed),
                lease_bytes_reclaimed: Some(summary.lease_bytes_reclaimed),
                dry_run: if dry_run { Some(true) } else { None },
                unreferenced_blobs: if dry_run {
                    Some(summary.reclaimed_blobs as usize)
                } else {
                    None
                },
                dead_leases: if dry_run {
                    Some(summary.leases_reclaimed)
                } else {
                    None
                },
                reclaimed_bytes: if dry_run {
                    Some(total_reclaimed_bytes)
                } else {
                    None
                },
            };
            Ok((data, Vec::new()))
        }
    }
}

/// The explicit one-way cutover (ADR-0004): activate mark-and-sweep,
/// or drop legacy refcount files entirely with a loud warning that
/// pre-cutover binaries must not use this store afterwards.
pub fn migrate(
    activate: bool,
    drop_refs: bool,
    cfg: &RunConfig,
) -> Result<(MigrateData, Vec<Diagnostic>)> {
    let mut store = open_store()?;
    if drop_refs {
        eprintln!(
            "WARNING: dropping legacy refs makes this store unreadable-by-refcount for \
pre-cutover binaries; they may collect live data. Make sure every wt binary that \
touches {} understands mirrors.",
            store.root().display()
        );
        let purged = store.purge_legacy_refs()?;
        store
            .set_gc_mode(GcMode::MarkSweepNoRefs)
            .map_err(|e| Error::Store(e.to_string()))?;
        if !cfg.json {
            println!(
                "gc-mode set to {}; purged {purged} legacy ref file{}",
                GcMode::MARK_SWEEP_NO_REFS,
                if purged == 1 { "" } else { "s" }
            );
        }
        let data = MigrateData {
            gc_mode: GcMode::MARK_SWEEP_NO_REFS.to_string(),
            purged_legacy_refs: Some(purged),
        };
        Ok((data, Vec::new()))
    } else if activate {
        store
            .set_gc_mode(GcMode::MarkSweep)
            .map_err(|e| Error::Store(e.to_string()))?;
        if !cfg.json {
            println!(
                "gc-mode set to {}: sweep now collects from live-mirror marks plus the \
grace period; refs/ stay maintained but ignored",
                GcMode::MARK_SWEEP
            );
        }
        let data = MigrateData {
            gc_mode: GcMode::MARK_SWEEP.to_string(),
            purged_legacy_refs: None,
        };
        Ok((data, Vec::new()))
    } else {
        Err(Error::Usage("must specify a migration action".into()))
    }
}
