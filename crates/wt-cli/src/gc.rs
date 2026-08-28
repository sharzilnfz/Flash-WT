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
use std::time::Duration;

use wt_store::{ContentId, GcMode, Store};

use crate::config::RunConfig;
use crate::envelope::{Diagnostic, MigrateData, RemoveData, SweepData};
use crate::error::{Error, Result};
use crate::gitops;
use crate::hydrate::open_store;

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

/// Content ids named by a worktree's hydration ledger, deduplicated:
/// one ledger row per materialized file, but references are claimed
/// once per distinct blob.
///
/// Ticket 08: rows may be typed. `<rel>\t<id>` is the legacy blob row;
/// `<rel>\tblob\t<id>` and `-\tsnapshot\t<id>` are the snapshot-era
/// forms, so removal knows which ids are blobs (release refs) and
/// which name snapshots (nothing to release).
fn read_ledger(git_dir: &Path) -> Result<(BTreeSet<String>, BTreeSet<String>)> {
    let path = git_dir.join("wt-hydrated.tsv");
    let text =
        fs::read_to_string(&path).map_err(|e| Error::io("read hydration ledger", &path, e))?;
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
                    return Err(Error::Store(format!(
                        "unknown ledger row type {other:?} in {}",
                        path.display()
                    )));
                }
            },
            _ => {
                return Err(Error::Store(format!(
                    "malformed ledger row in {}: {line:?}",
                    path.display()
                )));
            }
        }
    }
    Ok((blobs, snapshots))
}

pub fn remove(name: &str, dir: Option<&Path>, cfg: &RunConfig) -> Result<(RemoveData, Vec<Diagnostic>)> {
    let root = gitops::repo_root()?;

    let dest = match dir {
        Some(d) => d.to_path_buf(),
        None => gitops::default_worktree_dest(&root, name)?,
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
    let git_dir_text = gitops::run(&dest, &["rev-parse", "--absolute-git-dir"])?;
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
        && store.mirror_is_missing(&dest, &git_dir)?
    {
        let ids: Vec<ContentId> = ledger
            .iter()
            .map(|hex| {
                ContentId::from_hex(hex)
                    .ok_or_else(|| Error::Store(format!("malformed content id in ledger: {hex}")))
            })
            .collect::<Result<Vec<_>>>()?;
        let snaps: Vec<ContentId> = ledger_snapshots
            .iter()
            .map(|hex| {
                ContentId::from_hex(hex)
                    .ok_or_else(|| Error::Store(format!("malformed content id in ledger: {hex}")))
            })
            .collect::<Result<Vec<_>>>()?;
        store.publish_worktree_mirror(&dest, &git_dir, ids.iter(), snaps.iter())?;
    }

    let mut released = 0usize;
    if !ledger_blobs.is_empty() && !no_refs {
        for hex in &ledger_blobs {
            let id = ContentId::from_hex(hex)
                .ok_or_else(|| Error::Store(format!("malformed content id in ledger: {hex}")))?;
            match Store::release_ref(&mut store, &id) {
                Ok(()) => released += 1,
                Err(wt_store::Error::RefCountUnderflow(_)) => {}
                Err(e) => return Err(e.into()),
            }
        }
    }

    let mirror_removed = !ledger_blobs.is_empty() || !ledger_snapshots.is_empty();
    if mirror_removed {
        fs::remove_file(git_dir.join("wt-hydrated.tsv")).map_err(|e| {
            Error::io_unanchored("remove ledger", git_dir.join("wt-hydrated.tsv"), e)
        })?;
        // Retire the mirror now that both the sidecar and the
        // worktree are going away.
        store.remove_worktree_mirror(&dest, &git_dir)?;
    }

    gitops::run(&root, &["worktree", "remove", &dest.to_string_lossy()]).map_err(|e| {
        Error::Git(format!(
            "git worktree remove failed (references already released): {e}"
        ))
    })?;

    if !cfg.json {
        println!(
            "removed worktree {}; released {released} reference{}",
            dest.display(),
            if released == 1 { "" } else { "s" }
        );
    }

    let data = RemoveData {
        worktree_path: dest.display().to_string(),
        branch: name.to_string(),
        references_released: released,
        mirror_removed,
    };

    Ok((data, Vec::new()))
}

pub fn sweep(age: Option<Duration>, cfg: &RunConfig) -> Result<(SweepData, Vec<Diagnostic>)> {
    let mut store = open_store()?;
    match store.gc_mode() {
        GcMode::Legacy => {
            let max_age = age.unwrap_or(Duration::from_secs(7 * 24 * 60 * 60));
            let swept = store.sweep(max_age)?;
            // Dual-write parity evidence: compare what mirrors would
            // keep against what refcounts kept. Agreement prints
            // nothing; any disagreement is loud on stderr.
            for line in store.audit_marks_against_refs(grace_from_env()?)? {
                eprintln!("{line}");
            }
            if !cfg.json {
                println!(
                    "swept store: examined {}, reclaimed {} entr{}",
                    swept.examined,
                    swept.reclaimed,
                    if swept.reclaimed == 1 { "y" } else { "ies" }
                );
            }
            let data = SweepData {
                mode: "legacy".to_string(),
                examined: swept.examined as usize,
                reclaimed: swept.reclaimed as usize,
                mirrors_removed: None,
                snapshot_dirs_removed: None,
                snapshot_cap_evicted: None,
                deferred_by_grace: None,
            };
            Ok((data, Vec::new()))
        }
        GcMode::MarkSweep | GcMode::MarkSweepNoRefs => {
            let grace = match age {
                Some(explicit) => explicit,
                None => grace_from_env()?,
            };
            let swept = store.sweep_mark_sweep(grace, snapshot_cap_from_env()?)?;
            if swept.deferred_by_grace {
                eprintln!(
                    "wt-gc-audit: malformed young mirror deferred this sweep; rerun after the grace period"
                );
            }
            if !cfg.json {
                println!(
                    "swept store (mark-and-sweep): examined {}, reclaimed {}, mirrors removed {}, snapshots removed {}, cap evicted {}",
                    swept.examined,
                    swept.reclaimed,
                    swept.mirrors_removed,
                    swept.snapshot_dirs_removed,
                    swept.snapshot_cap_evicted,
                );
            }
            let data = SweepData {
                mode: "mark-sweep".to_string(),
                examined: swept.examined as usize,
                reclaimed: swept.reclaimed as usize,
                mirrors_removed: Some(swept.mirrors_removed as usize),
                snapshot_dirs_removed: Some(swept.snapshot_dirs_removed as usize),
                snapshot_cap_evicted: Some(swept.snapshot_cap_evicted as usize),
                deferred_by_grace: Some(swept.deferred_by_grace),
            };
            Ok((data, Vec::new()))
        }
    }
}

/// The explicit one-way cutover (ADR-0004): activate mark-and-sweep,
/// or drop legacy refcount files entirely with a loud warning that
/// pre-cutover binaries must not use this store afterwards.
pub fn migrate(activate: bool, drop_refs: bool, cfg: &RunConfig) -> Result<(MigrateData, Vec<Diagnostic>)> {
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
