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
//! Removal reads the hydration ledger (`wt-hydrated.tsv`, written by
//! ticket 05 into the worktree's git dir) to learn which blobs the
//! worktree referenced, releases one reference per distinct blob,
//! then hands the directory to `git worktree remove`. Releasing
//! happens before removal because `git worktree remove` deletes the
//! git dir — the ledger along with it. A release that hits zero is
//! tolerated as "already released" so an interrupted remove can
//! simply be rerun.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use wt_store::{ContentId, GcMode, Store};

use crate::hydrate::open_store;

/// Default grace period when `WT_GC_GRACE` is unset or unreadable.
const DEFAULT_GRACE: Duration = Duration::from_secs(15 * 60);

fn run_git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
    }
}

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
    Some(Duration::from_secs(count * secs))
}

/// Grace period for mark-and-sweep modes: `WT_GC_GRACE` if it parses
/// (e.g. `15m`, `1h`), else fifteen minutes.
pub fn grace_from_env() -> Result<Duration, String> {
    match std::env::var("WT_GC_GRACE") {
        Ok(text) => parse_age(&text)
            .ok_or_else(|| format!("invalid WT_GC_GRACE {text:?} (try 15m, 1h, 7d)")),
        Err(_) => Ok(DEFAULT_GRACE),
    }
}

/// Content ids named by a worktree's hydration ledger, deduplicated:
/// one ledger row per materialized file, but references are claimed
/// once per distinct blob.
///
/// Ticket 08: rows may be typed. `<rel>\t<id>` is the legacy blob row;
/// `<rel>\tblob\t<id>` and `-\tsnapshot\t<id>` are the snapshot-era
/// forms, so removal knows which ids are blobs (release refs) and
/// which name snapshots (nothing to release).
fn read_ledger(git_dir: &Path) -> Result<(BTreeSet<String>, BTreeSet<String>), String> {
    let path = git_dir.join("wt-hydrated.tsv");
    let text = fs::read_to_string(&path)
        .map_err(|e| format!("cannot read hydration ledger {}: {e}", path.display()))?;
    let mut blobs = BTreeSet::new();
    let mut snapshots = BTreeSet::new();
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        match fields.as_slice() {
            [_, id] => {
                blobs.insert(id.to_string());
            }
            [_, kind, id] => match *kind {
                "blob" => {
                    blobs.insert(id.to_string());
                }
                "snapshot" => {
                    snapshots.insert(id.to_string());
                }
                other => {
                    return Err(format!(
                        "unknown ledger row type {other:?} in {}",
                        path.display()
                    ));
                }
            },
            _ => {
                return Err(format!(
                    "malformed ledger row in {}: {line:?}",
                    path.display()
                ));
            }
        }
    }
    Ok((blobs, snapshots))
}

pub fn remove(name: &str, dir: Option<&Path>) -> Result<(), String> {
    let root_out = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|e| e.to_string())?;
    if !root_out.status.success() {
        return Err("not inside a git repository".into());
    }
    let root = PathBuf::from(String::from_utf8_lossy(&root_out.stdout).trim());

    let dest = match dir {
        Some(d) => d.to_path_buf(),
        None => root
            .parent()
            .ok_or("repository root has no parent")?
            .join(format!(
                "{}-{name}",
                root.file_name()
                    .ok_or("cannot name repository directory")?
                    .to_string_lossy()
            )),
    };
    if !dest.join(".git").exists() {
        return Err(format!("{} is not a worktree", dest.display()));
    }

    // The linked git dir holds the ledger. Resolve it while the
    // worktree still exists; for linked worktrees this lands inside
    // the main repo's .git/worktrees/<name>.
    let git_dir_text = run_git(&dest, &["rev-parse", "--absolute-git-dir"])?;
    let git_dir = PathBuf::from(&git_dir_text);
    let (ledger_blobs, ledger_snapshots) = if git_dir.join("wt-hydrated.tsv").exists() {
        read_ledger(&git_dir)?
    } else {
        (BTreeSet::new(), BTreeSet::new())
    };
    let ledger = &ledger_blobs;

    let mut store = open_store()?;
    let no_refs = store.gc_mode() == GcMode::MarkSweepNoRefs;

    // Ticket 07: make sure the store mirror exists before anything
    // destructive happens. A missing mirror with a live sidecar is
    // repaired here ("the next wt create or wt remove rewrites the
    // mirror"), so even a crash right after this point leaves a
    // correct root behind. Ticket 08: the repair carries BOTH record
    // types — file records mark blobs directly, snapshot records mark
    // through their manifests.
    if (!ledger.is_empty() || !ledger_snapshots.is_empty())
        && store
            .mirror_is_missing(&dest, &git_dir)
            .map_err(|e| e.to_string())?
    {
        let ids: Vec<ContentId> = ledger
            .iter()
            .map(|hex| {
                ContentId::from_hex(hex)
                    .ok_or_else(|| format!("malformed content id in ledger: {hex}"))
            })
            .collect::<Result<_, _>>()?;
        let snaps: Vec<ContentId> = ledger_snapshots
            .iter()
            .map(|hex| {
                ContentId::from_hex(hex)
                    .ok_or_else(|| format!("malformed content id in ledger: {hex}"))
            })
            .collect::<Result<_, _>>()?;
        store
            .publish_worktree_mirror(&dest, &git_dir, ids.iter(), snaps.iter())
            .map_err(|e| e.to_string())?;
    }

    let mut released = 0usize;
    if !ledger_blobs.is_empty() && !no_refs {
        for hex in &ledger_blobs {
            let id = ContentId::from_hex(hex)
                .ok_or_else(|| format!("malformed content id in ledger: {hex}"))?;
            match Store::release_ref(&mut store, &id) {
                Ok(()) => released += 1,
                Err(wt_store::Error::RefCountUnderflow(_)) => {}
                Err(e) => return Err(e.to_string()),
            }
        }
    }

    if !ledger_blobs.is_empty() || !ledger_snapshots.is_empty() {
        fs::remove_file(git_dir.join("wt-hydrated.tsv"))
            .map_err(|e| format!("cannot remove ledger: {e}"))?;
        // Retire the mirror now that both the sidecar and the
        // worktree are going away.
        store
            .remove_worktree_mirror(&dest, &git_dir)
            .map_err(|e| e.to_string())?;
    }

    run_git(&root, &["worktree", "remove", &dest.to_string_lossy()])
        .map_err(|e| format!("git worktree remove failed (references already released): {e}"))?;

    println!(
        "removed worktree {}; released {released} reference{}",
        dest.display(),
        if released == 1 { "" } else { "s" }
    );
    Ok(())
}

pub fn sweep(age: Option<&str>) -> Result<(), String> {
    let mut store = open_store()?;
    match store.gc_mode() {
        GcMode::Legacy => {
            let age_text = age.unwrap_or("7d");
            let max_age = parse_age(age_text)
                .ok_or_else(|| format!("invalid --age {age_text:?} (try 0s, 10m, 1h, 7d)"))?;
            let swept = store.sweep(max_age).map_err(|e| e.to_string())?;
            // Dual-write parity evidence: compare what mirrors would
            // keep against what refcounts kept. Agreement prints
            // nothing; any disagreement is loud on stderr.
            for line in store
                .audit_marks_against_refs(grace_from_env()?)
                .map_err(|e| e.to_string())?
            {
                eprintln!("{line}");
            }
            println!(
                "swept store: examined {}, reclaimed {} entr{}",
                swept.examined,
                swept.reclaimed,
                if swept.reclaimed == 1 { "y" } else { "ies" }
            );
            Ok(())
        }
        GcMode::MarkSweep | GcMode::MarkSweepNoRefs => {
            let grace = match age {
                Some(explicit) => parse_age(explicit)
                    .ok_or_else(|| format!("invalid --age {explicit:?} (try 0s, 10m, 1h, 7d)"))?,
                None => grace_from_env()?,
            };
            let swept = store.sweep_mark_sweep(grace).map_err(|e| e.to_string())?;
            if swept.deferred_by_grace {
                eprintln!(
                    "wt-gc-audit: malformed young mirror deferred this sweep; rerun after the grace period"
                );
            }
            println!(
                "swept store (mark-and-sweep): examined {}, reclaimed {}, mirrors removed {}, snapshots removed {}",
                swept.examined, swept.reclaimed, swept.mirrors_removed, swept.snapshot_dirs_removed,
            );
            Ok(())
        }
    }
}

/// The explicit one-way cutover (ADR-0004): activate mark-and-sweep,
/// or drop legacy refcount files entirely with a loud warning that
/// pre-cutover binaries must not use this store afterwards.
pub fn migrate(activate: bool, drop_refs: bool) -> Result<(), String> {
    let mut store = open_store()?;
    if drop_refs {
        eprintln!(
            "WARNING: dropping legacy refs makes this store unreadable-by-refcount for \
pre-cutover binaries; they may collect live data. Make sure every wt binary that \
touches {} understands mirrors.",
            store.root().display()
        );
        let purged = store.purge_legacy_refs().map_err(|e| e.to_string())?;
        store
            .set_gc_mode(GcMode::MarkSweepNoRefs)
            .map_err(|e| e.to_string())?;
        println!(
            "gc-mode set to {}; purged {purged} legacy ref file{}",
            GcMode::MARK_SWEEP_NO_REFS,
            if purged == 1 { "" } else { "s" }
        );
    } else if activate {
        store
            .set_gc_mode(GcMode::MarkSweep)
            .map_err(|e| e.to_string())?;
        println!(
            "gc-mode set to {}: sweep now collects from live-mirror marks plus the \
grace period; refs/ stay maintained but ignored",
            GcMode::MARK_SWEEP
        );
    }
    Ok(())
}
