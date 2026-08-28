// Tests assert with unwrap/expect by design: a panic IS the failure
// signal under test, so the workspace restriction lints stay off here.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Ticket 06: garbage collection. Removing a worktree releases its
//! references, and an age-based sweep reclaims unreferenced store
//! entries, so the store never grows without bound. Everything is
//! asserted through the CLI seam: exit codes, stdout/stderr, and the
//! files the store leaves on disk.

mod common;

use std::fs;
use std::path::Path;
use std::thread;

use common::{Fixture, list_files};

/// Total file count under the store root — how much content it holds.
/// The ingest validation cache and the verified-blob ledger beside the
/// store are not content and are never swept, so they do not count.
fn store_files(store: &Path) -> usize {
    list_files(store)
        .into_iter()
        .filter(|p| {
            let name = p.file_name();
            name != Some(std::ffi::OsStr::new("ingest-cache.tsv"))
                && name != Some(std::ffi::OsStr::new("verified.tsv"))
                && !p.starts_with(store.join("snapshots"))
                && !p.starts_with(store.join("worktrees"))
        })
        .count()
}

/// Resolve a worktree's real git dir from its `.git` pointer file and
/// return the content ids in its hydration ledger.
fn ledger_ids(wt_root: &Path) -> Vec<String> {
    let pointer = fs::read_to_string(wt_root.join(".git")).expect("worktree .git pointer");
    let git_dir = pointer
        .trim()
        .strip_prefix("gitdir: ")
        .expect(".git points at a gitdir");
    let ledger = fs::read_to_string(Path::new(git_dir).join("wt-hydrated.tsv"))
        .expect("hydration ledger exists");
    ledger
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split('\t').collect();
            match fields.as_slice() {
                [_, id] => Some((*id).to_owned()),
                [_, "blob", id] => Some((*id).to_owned()),
                _ => None,
            }
        })
        .collect()
}

#[test]
fn help_lists_remove_and_sweep() {
    let out = Fixture::heavy_repo(1).wt(&["--help"]);
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("remove"));
    assert!(text.contains("sweep"));
}

#[test]
fn remove_deletes_worktree_and_releases_references() {
    let fx = Fixture::heavy_repo(50);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let base = tempfile::tempdir().expect("tempdir");
    let store = base.path().join("store");

    let created = fx.wt_with_store(&["create", "one"], &store);
    assert!(
        created.status.success(),
        "create failed: {}",
        String::from_utf8_lossy(&created.stderr)
    );
    let wt_root = fx.repo.parent().unwrap().join("origin-one");
    let ids = ledger_ids(&wt_root);
    assert!(!ids.is_empty(), "ledger must name hydrated content");

    // Every referenced blob carries a ref-count file on disk.
    for id in &ids {
        assert!(
            store.join("refs").join(id).is_file(),
            "missing ref file for {id}"
        );
    }

    let removed = fx.wt_with_store(&["remove", "one"], &store);
    assert!(
        removed.status.success(),
        "remove failed: {}",
        String::from_utf8_lossy(&removed.stderr)
    );

    // The worktree directory itself is gone.
    assert!(
        !wt_root.exists(),
        "removed worktree still exists at {}",
        wt_root.display()
    );
    // Its references are gone from the store (ref files deleted or at
    // zero), observable before any sweep runs.
    for id in &ids {
        match fs::read_to_string(store.join("refs").join(id)) {
            Ok(count) => assert_eq!(count.trim(), "0", "reference to {id} survived removal"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => panic!("cannot read ref file for {id}: {e}"),
        }
    }
}

#[test]
fn referenced_entries_survive_aggressive_sweep() {
    let fx = Fixture::heavy_repo(50);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let base = tempfile::tempdir().expect("tempdir");
    let store = base.path().join("store");

    for name in ["one", "two"] {
        let out = fx.wt_with_store(&["create", name], &store);
        assert!(out.status.success());
    }
    // Distinct content so the two worktrees do not share everything.
    fs::write(fx.repo.join("heavy").join("unique.txt"), "only in two\n").unwrap();
    let out = fx.wt_with_store(&["create", "three"], &store);
    assert!(out.status.success());

    let removed = fx.wt_with_store(&["remove", "one"], &store);
    assert!(removed.status.success());

    let swept = fx.wt_with_store(&["sweep", "--age", "0s"], &store);
    assert!(
        swept.status.success(),
        "sweep failed: {}",
        String::from_utf8_lossy(&swept.stderr)
    );

    // The surviving worktrees' content is still in the store.
    assert!(
        store_files(&store) > 0,
        "aggressive sweep deleted referenced entries"
    );

    // And the store can still serve them: a fresh worktree hydrates
    // byte-identical heavy content.
    let out = fx.wt_with_store(&["create", "four"], &store);
    assert!(
        out.status.success(),
        "post-sweep create failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let src = list_files(&fx.repo.join("heavy"));
    let dest = list_files(&fx.repo.parent().unwrap().join("origin-four").join("heavy"));
    assert_eq!(
        src.len(),
        dest.len(),
        "hydrated tree incomplete after sweep"
    );
    for (a, b) in src.iter().zip(dest.iter()) {
        assert_eq!(fs::read(a).unwrap(), fs::read(b).unwrap());
    }
}

#[test]
fn full_lifecycle_create_create_remove_sweep_leaves_minimal_store() {
    let fx = Fixture::heavy_repo(50);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let base = tempfile::tempdir().expect("tempdir");
    let store = base.path().join("store");

    for name in ["one", "two"] {
        let out = fx.wt_with_store(&["create", name], &store);
        assert!(out.status.success());
    }
    assert!(store_files(&store) > 0, "store stayed empty after creates");

    for name in ["one", "two"] {
        let out = fx.wt_with_store(&["remove", name], &store);
        assert!(
            out.status.success(),
            "remove {name} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    let swept = fx.wt_with_store(&["sweep", "--age", "0s"], &store);
    assert!(swept.status.success());
    let stdout = String::from_utf8_lossy(&swept.stdout);
    assert!(
        stdout.contains("reclaimed"),
        "sweep must say what it reclaimed:\n{stdout}"
    );

    // Delete everything, run the sweep: nothing is left.
    assert_eq!(store_files(&store), 0, "store did not shrink to empty");

    // A second sweep is a no-op, not an error.
    let again = fx.wt_with_store(&["sweep", "--age", "0s"], &store);
    assert!(again.status.success());
}

#[test]
fn interrupted_sweep_state_is_reclaimed_and_store_stays_usable() {
    let fx = Fixture::heavy_repo(50);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let base = tempfile::tempdir().expect("tempdir");
    let store = base.path().join("store");

    let out = fx.wt_with_store(&["create", "one"], &store);
    assert!(out.status.success());
    // The worktree stays alive here: its remaining references must
    // survive; only the interrupted entry may go.
    let before = store_files(&store);

    // Simulate a kill mid-sweep: one entry already had its ref file
    // unlinked but not yet its object — exactly the intermediate state
    // the sweep's deletion order produces.
    let ids = ledger_ids(&fx.repo.parent().unwrap().join("origin-one"));
    let victim = store.join("refs").join(&ids[0]);
    fs::remove_file(&victim).expect("unlink ref file to simulate interruption");

    let swept = fx.wt_with_store(&["sweep", "--age", "0s"], &store);
    assert!(
        swept.status.success(),
        "sweep must survive partial state: {}",
        String::from_utf8_lossy(&swept.stderr)
    );
    assert!(
        String::from_utf8_lossy(&swept.stdout).contains("reclaimed 1"),
        "exactly the interrupted entry should be reclaimed"
    );
    // Ref file (already gone) plus the orphaned object.
    assert_eq!(store_files(&store), before - 2);

    // The store remains fully usable afterwards.
    let out = fx.wt_with_store(&["create", "two"], &store);
    assert!(
        out.status.success(),
        "post-sweep create failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// --- age threshold ---

#[test]
fn fresh_unreferenced_entries_survive_a_nonzero_age_threshold() {
    let fx = Fixture::heavy_repo(10);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let base = tempfile::tempdir().expect("tempdir");
    let store = base.path().join("store");

    let out = fx.wt_with_store(&["create", "one"], &store);
    assert!(out.status.success());
    let removed = fx.wt_with_store(&["remove", "one"], &store);
    assert!(removed.status.success());

    // Entries written seconds ago are younger than one hour.
    let swept = fx.wt_with_store(&["sweep", "--age", "1h"], &store);
    assert!(swept.status.success());
    assert!(store_files(&store) > 0, "sweep ignored its age threshold");

    thread::sleep(std::time::Duration::from_millis(1100));
    let swept = fx.wt_with_store(&["sweep", "--age", "1s"], &store);
    assert!(swept.status.success());
    assert_eq!(store_files(&store), 0, "aged entries were not reclaimed");
}
