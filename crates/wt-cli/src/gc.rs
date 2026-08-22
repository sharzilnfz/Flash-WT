//! Garbage collection (ticket 06): `wt remove` releases a worktree's
//! references, `wt sweep --age <duration>` reclaims unreferenced store
//! entries past the age threshold.
//!
//! Removal reads the hydration ledger (`wt-hydrated.tsv`, written by
//! ticket 05 into the worktree's git dir) to learn which blobs the
//! worktree referenced, releases one reference per distinct blob, then
//! hands the directory to `git worktree remove`. Releasing happens
//! before removal because `git worktree remove` deletes the git dir —
//! the ledger along with it. A release that hits zero is tolerated as
//! "already released" so an interrupted remove can simply be rerun.
//!
//! Sweep consistency: the underlying deletion order (ref file first,
//! object second) means a kill mid-run leaves only states the next
//! sweep finishes — never a dangling ref file without its object.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use wt_store::Store;

use crate::hydrate::open_store;

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

/// Content ids named by a worktree's hydration ledger, deduplicated:
/// one ledger row per materialized file, but references are claimed
/// once per distinct blob.
fn read_ledger(git_dir: &Path) -> Result<BTreeSet<String>, String> {
    let path = git_dir.join("wt-hydrated.tsv");
    let text = fs::read_to_string(&path)
        .map_err(|e| format!("cannot read hydration ledger {}: {e}", path.display()))?;
    let mut ids = BTreeSet::new();
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        let id = line
            .split('\t')
            .nth(1)
            .ok_or_else(|| format!("malformed ledger row in {}: {line:?}", path.display()))?;
        ids.insert(id.to_owned());
    }
    Ok(ids)
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
    let ledger = if git_dir.join("wt-hydrated.tsv").exists() {
        read_ledger(&git_dir)?
    } else {
        BTreeSet::new()
    };

    let mut released = 0usize;
    if !ledger.is_empty() {
        let mut store = open_store()?;
        for hex in &ledger {
            let id = wt_store::ContentId::from_hex(hex)
                .ok_or_else(|| format!("malformed content id in ledger: {hex}"))?;
            match Store::release_ref(&mut store, &id) {
                Ok(()) => released += 1,
                Err(wt_store::Error::RefCountUnderflow(_)) => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        fs::remove_file(git_dir.join("wt-hydrated.tsv"))
            .map_err(|e| format!("cannot remove ledger: {e}"))?;
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

pub fn sweep(age: &str) -> Result<(), String> {
    let max_age =
        parse_age(age).ok_or_else(|| format!("invalid --age {age:?} (try 0s, 10m, 1h, 7d)"))?;
    let mut store = open_store()?;
    let swept = store.sweep(max_age).map_err(|e| e.to_string())?;
    println!(
        "swept store: examined {}, reclaimed {} entr{}",
        swept.examined,
        swept.reclaimed,
        if swept.reclaimed == 1 { "y" } else { "ies" }
    );
    Ok(())
}
