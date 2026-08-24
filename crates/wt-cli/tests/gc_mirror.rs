// Tests assert with unwrap/expect by design: a panic IS the failure
// signal under test, so the workspace restriction lints stay off here.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Fast-hydration ticket 07: store-local mark-and-sweep. The store
//! mirror written once per create is the GC root; sweep modes are
//! gated by `<store>/gc-mode` (`wt store migrate`). Everything is
//! asserted through the CLI seam, like tests/gc.rs.

mod common;

use std::fs;
use std::fs::FileTimes;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, UNIX_EPOCH};

use common::{list_files, Fixture};

/// Regular files under the store root that are not content and never
/// swept (mirrors live under worktrees/, which these tests inspect
/// directly instead of hiding).
fn content_files(store: &Path) -> Vec<std::path::PathBuf> {
    list_files(store)
        .into_iter()
        .filter(|p| {
            let name = p.file_name();
            name != Some(std::ffi::OsStr::new("ingest-cache.tsv"))
                && name != Some(std::ffi::OsStr::new("verified.tsv"))
                && name != Some(std::ffi::OsStr::new("gc-mode"))
                && p.parent().unwrap().file_name() != Some(std::ffi::OsStr::new("worktrees"))
        })
        .collect()
}

fn mirror_files(store: &Path) -> Vec<std::path::PathBuf> {
    list_files(&store.join("worktrees"))
}

/// Resolve a worktree's real git dir from its `.git` pointer file.
fn git_dir_of(wt_root: &Path) -> std::path::PathBuf {
    let pointer = fs::read_to_string(wt_root.join(".git")).expect("worktree .git pointer");
    PathBuf::from(
        pointer
            .trim()
            .strip_prefix("gitdir: ")
            .expect(".git points at a gitdir"),
    )
}

/// Content ids in a worktree's hydration sidecar.
fn ledger_ids(wt_root: &Path) -> Vec<String> {
    let ledger = fs::read_to_string(git_dir_of(wt_root).join("wt-hydrated.tsv"))
        .expect("hydration ledger exists");
    ledger
        .lines()
        .filter(|l| !l.is_empty())
        .map(|line| {
            line.split('\t')
                .nth(1)
                .expect("ledger row has id")
                .to_owned()
        })
        .collect()
}

fn object_path(store: &Path, id: &str) -> std::path::PathBuf {
    store.join("objects").join(&id[..2]).join(&id[2..])
}

/// Push every blob's mtime far into the past so even `--age 0s`
/// sweeps treat them as aged out.
fn age_objects(store: &Path) {
    for path in list_files(&store.join("objects")) {
        let f = fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("open blob");
        f.set_times(FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(1000)))
            .expect("age blob mtime");
    }
}

fn age_file(path: &Path) {
    let f = fs::OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap_or_else(|e| panic!("open {}: {e}", path.display()));
    f.set_times(FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(1000)))
        .expect("age mtime");
}

#[test]
fn help_lists_store_migrate() {
    let out = Fixture::heavy_repo(1).wt(&["store", "migrate", "--help"]);
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("--activate-mark-sweep"));
    assert!(text.contains("--drop-legacy-refs"));
}

// --- mark-sweep retains what live mirrors mark ---

#[test]
fn live_worktree_blobs_survive_mark_sweep_past_any_age() {
    let fx = Fixture::heavy_repo(20);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();
    let base = tempfile::tempdir().expect("tempdir");
    let store = base.path().join("store");

    let created = fx.wt_with_store(&["create", "one"], &store);
    assert!(
        created.status.success(),
        "create failed: {}",
        String::from_utf8_lossy(&created.stderr)
    );
    let activated = fx.wt_with_store(&["store", "migrate", "--activate-mark-sweep"], &store);
    assert!(activated.status.success());

    // Every blob is ancient now; only the mirror keeps them alive.
    age_objects(&store);
    let ids = ledger_ids(&fx.repo.parent().unwrap().join("origin-one"));
    assert!(!ids.is_empty());
    assert_eq!(mirror_files(&store).len(), 1, "one create wrote one mirror");

    let swept = fx.wt_with_store(&["sweep", "--age", "0s"], &store);
    assert!(
        swept.status.success(),
        "mark-sweep failed: {}",
        String::from_utf8_lossy(&swept.stderr)
    );
    for id in &ids {
        assert!(
            object_path(&store, id).is_file(),
            "marked blob {id} was collected from under a live worktree"
        );
    }

    // And the store still serves the tree.
    let again = fx.wt_with_store(&["create", "two"], &store);
    assert!(
        again.status.success(),
        "post-sweep create failed: {}",
        String::from_utf8_lossy(&again.stderr)
    );
}

#[test]
fn rm_rf_worktree_without_prune_becomes_collectable_after_grace() {
    let fx = Fixture::heavy_repo(10);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();
    let base = tempfile::tempdir().expect("tempdir");
    let store = base.path().join("store");

    let created = fx.wt_with_store(&["create", "doomed"], &store);
    assert!(created.status.success());
    let wt_root = fx.repo.parent().unwrap().join("origin-doomed");
    let ids = ledger_ids(&wt_root);

    // Out-of-band deletion: no `git worktree prune` ever runs.
    fs::remove_dir_all(&wt_root).expect("rm -rf the worktree");

    let activated = fx.wt_with_store(&["store", "migrate", "--activate-mark-sweep"], &store);
    assert!(activated.status.success());

    // Within the grace period nothing goes, no matter how old the
    // blobs are: the mirror is young, its root may still be valid.
    age_objects(&store);
    let swept = fx.wt_with_store(&["sweep", "--age", "1h"], &store);
    assert!(swept.status.success());
    for id in &ids {
        assert!(
            object_path(&store, id).is_file(),
            "{id} collected inside grace"
        );
    }

    // Past the grace period the dead root stops protecting anything,
    // and the stale mirror itself goes with it.
    for mirror in mirror_files(&store) {
        age_file(&mirror);
    }
    let swept = fx.wt_with_store(&["sweep", "--age", "0s"], &store);
    assert!(
        swept.status.success(),
        "sweep failed: {}",
        String::from_utf8_lossy(&swept.stderr)
    );
    assert!(
        String::from_utf8_lossy(&swept.stdout).contains("mirrors removed 1"),
        "stale mirror must be reported as removed:\n{}",
        String::from_utf8_lossy(&swept.stdout)
    );
    for id in &ids {
        assert!(
            !object_path(&store, id).exists(),
            "{id} survived the death of its only root"
        );
    }
    assert!(
        content_files(&store).is_empty(),
        "nothing else should remain"
    );
}

// --- mirror damage handling ---

#[test]
fn torn_final_line_is_ignored_and_live_content_survives() {
    let fx = Fixture::heavy_repo(10);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();
    let base = tempfile::tempdir().expect("tempdir");
    let store = base.path().join("store");

    assert!(fx
        .wt_with_store(&["create", "one"], &store)
        .status
        .success());
    let ids = ledger_ids(&fx.repo.parent().unwrap().join("origin-one"));

    let mirror = &mirror_files(&store)[0];
    let mut text = fs::read_to_string(mirror).expect("read mirror");
    assert!(text.ends_with('\n'));
    text.push_str("file\tdeadbeef"); // torn append, no newline
    fs::write(mirror, text).expect("tear the mirror");
    let _ = mirror;

    assert!(fx
        .wt_with_store(&["store", "migrate", "--activate-mark-sweep"], &store)
        .status
        .success());
    age_objects(&store);

    let swept = fx.wt_with_store(&["sweep", "--age", "0s"], &store);
    assert!(
        swept.status.success(),
        "{}",
        String::from_utf8_lossy(&swept.stderr)
    );
    for id in &ids {
        assert!(
            object_path(&store, id).is_file(),
            "torn line cost us blob {id}"
        );
    }
}

#[test]
fn malformed_young_mirror_defers_deletion_and_is_reported() {
    let fx = Fixture::heavy_repo(10);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();
    let base = tempfile::tempdir().expect("tempdir");
    let store = base.path().join("store");

    assert!(fx
        .wt_with_store(&["create", "one"], &store)
        .status
        .success());
    let ids = ledger_ids(&fx.repo.parent().unwrap().join("origin-one"));

    // A corrupted mirror: garbage where the v1 header should be. Its
    // fresh mtime means it may still be mid-repair inside the grace
    // window (one hour here), so deletion decisions must not treat
    // it as empty.
    let mut mirrors = mirror_files(&store);
    let mirror = mirrors.remove(0);
    fs::write(&mirror, "not a mirror at all\n").expect("corrupt mirror");

    assert!(fx
        .wt_with_store(&["store", "migrate", "--activate-mark-sweep"], &store)
        .status
        .success());
    age_objects(&store);

    let swept = fx.wt_with_store(&["sweep", "--age", "1h"], &store);
    assert!(
        swept.status.success(),
        "a corrupt mirror must not break sweep: {}",
        String::from_utf8_lossy(&swept.stderr)
    );
    let stderr = String::from_utf8_lossy(&swept.stderr);
    assert!(
        stderr.contains("deferred") || stderr.contains("invalid mirror"),
        "the malformed mirror must be reported: {stderr}"
    );
    for id in &ids {
        assert!(
            object_path(&store, id).is_file(),
            "malformed mirror must not silently empty the root set ({id})"
        );
    }
    // The bad mirror stays on disk for diagnosis.
    assert_eq!(mirror_files(&store).len(), 1);
}

#[test]
fn missing_mirror_still_allows_remove_and_leaves_no_mirror_behind() {
    let fx = Fixture::heavy_repo(10);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();
    let base = tempfile::tempdir().expect("tempdir");
    let store = base.path().join("store");

    assert!(fx
        .wt_with_store(&["create", "one"], &store)
        .status
        .success());
    let wt_root = fx.repo.parent().unwrap().join("origin-one");
    let refs_before = ledger_ids(&wt_root);

    // Lose the mirror. The plan says the next create/remove rewrites
    // it — here remove repairs then retires it, and must not fail.
    fs::remove_file(&mirror_files(&store)[0]).expect("delete mirror");

    let removed = fx.wt_with_store(&["remove", "one"], &store);
    assert!(
        removed.status.success(),
        "remove without mirror failed: {}",
        String::from_utf8_lossy(&removed.stderr)
    );
    assert!(!wt_root.exists());
    assert!(
        mirror_files(&store).is_empty(),
        "no mirror may survive remove"
    );

    // Legacy refs were still released through the sidecar fallback.
    for id in refs_before {
        match fs::read_to_string(store.join("refs").join(id)) {
            Ok(count) => assert_eq!(count.trim(), "0"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => panic!("cannot read ref file: {e}"),
        }
    }
}

#[test]
fn interrupted_mark_sweep_state_reconciles_on_the_next_sweep() {
    let fx = Fixture::heavy_repo(20);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();
    let base = tempfile::tempdir().expect("tempdir");
    let store = base.path().join("store");

    assert!(fx
        .wt_with_store(&["create", "one"], &store)
        .status
        .success());
    assert!(fx
        .wt_with_store(&["store", "migrate", "--activate-mark-sweep"], &store)
        .status
        .success());

    // Kill mid-sweep, simulated: one blob already unlinked, its
    // siblings not yet. The next sweep reconciles without touching
    // what the live mirror still marks.
    let wt_root = fx.repo.parent().unwrap().join("origin-one");
    let ids = ledger_ids(&wt_root);
    fs::remove_file(object_path(&store, &ids[0])).expect("simulate partial sweep");

    let swept = fx.wt_with_store(&["sweep", "--age", "1h"], &store);
    assert!(
        swept.status.success(),
        "{}",
        String::from_utf8_lossy(&swept.stderr)
    );
    for id in &ids[1..] {
        assert!(
            object_path(&store, id).is_file(),
            "{id} lost to a partial sweep"
        );
    }
    assert!(!object_path(&store, &ids[0]).exists());

    // The store remains fully usable afterwards.
    let again = fx.wt_with_store(&["create", "two"], &store);
    assert!(
        again.status.success(),
        "{}",
        String::from_utf8_lossy(&again.stderr)
    );
}

// --- WT_TIMING=1 stage timings ---

#[test]
fn timing_env_emits_wt_stage_lines_to_stderr_and_stays_silent_without_it() {
    let fx = Fixture::heavy_repo(10);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();
    let base = tempfile::tempdir().expect("tempdir");
    let store = base.path().join("store");

    let quiet = fx.wt_with_store(&["create", "quiet"], &store);
    assert!(quiet.status.success());
    assert!(
        !String::from_utf8_lossy(&quiet.stderr).contains("wt-stage"),
        "no stage lines without WT_TIMING"
    );

    let timed = Command::new(env!("CARGO_BIN_EXE_wt"))
        .args(["create", "loud"])
        .env("WT_STORE", &store)
        .env("WT_TIMING", "1")
        .current_dir(&fx.repo)
        .output()
        .expect("run wt binary");
    assert!(timed.status.success());
    let stderr = String::from_utf8_lossy(&timed.stderr);
    let stages: Vec<&str> = stderr
        .lines()
        .filter(|l| l.starts_with("wt-stage "))
        .collect();
    let names: Vec<&str> = stages
        .iter()
        .filter_map(|l| l.split_once('=').map(|(n, _)| n))
        .collect();
    let ms: Vec<Option<u64>> = stages
        .iter()
        .map(|l| l.split('=').nth(1).and_then(|v| v.parse().ok()))
        .collect();
    assert!(
        ms.iter().all(|v| v.is_some()),
        "stage values must be integer milliseconds:\n{stderr}"
    );
    // Step 0 added finer-grained stage lines (git-worktree,
    // verify/place, snapshot sub-stages). The legacy four must still
    // be present, in their original relative order; the set is now a
    // superset, so no exact-match assertion.
    let legacy = [
        "wt-stage ingest",
        "wt-stage references",
        "wt-stage materialize",
        "wt-stage total",
    ];
    let mut cursor = 0usize;
    for want in legacy {
        let found = names[cursor..]
            .iter()
            .position(|n| *n == want)
            .unwrap_or_else(|| panic!("legacy stage {want} missing or reordered:\n{stderr}"));
        cursor += found + 1;
    }
}

// --- legacy mode: audit parity + unchanged behavior ---

#[test]
fn legacy_sweep_audit_reports_zero_disagreements_on_normal_fixtures() {
    let fx = Fixture::heavy_repo(30);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();
    let base = tempfile::tempdir().expect("tempdir");
    let store = base.path().join("store");

    for name in ["one", "two"] {
        assert!(fx.wt_with_store(&["create", name], &store).status.success());
    }
    let removed = fx.wt_with_store(&["remove", "one"], &store);
    assert!(removed.status.success());

    // No gc-mode marker exists: this sweep runs exactly like ticket
    // 06's, with the mark-vs-refs audit beside it. Normal fixtures
    // agree, so stderr carries no audit lines.
    let swept = fx.wt_with_store(&["sweep", "--age", "0s"], &store);
    assert!(swept.status.success());
    let stderr = String::from_utf8_lossy(&swept.stderr);
    assert!(
        !stderr.contains("wt-gc-audit"),
        "audit disagreement on a healthy fixture:\n{stderr}"
    );
    assert!(
        !content_files(&store).is_empty(),
        "live content was collected"
    );
}

#[test]
fn dual_write_keeps_refs_maintained_until_explicit_cutover() {
    let fx = Fixture::heavy_repo(10);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();
    let base = tempfile::tempdir().expect("tempdir");
    let store = base.path().join("store");

    assert!(fx
        .wt_with_store(&["store", "migrate", "--activate-mark-sweep"], &store)
        .status
        .success());
    assert_eq!(
        fs::read_to_string(store.join("gc-mode")).unwrap().trim(),
        "mark-sweep"
    );

    assert!(fx
        .wt_with_store(&["create", "one"], &store)
        .status
        .success());
    let ids = ledger_ids(&fx.repo.parent().unwrap().join("origin-one"));
    for id in &ids {
        assert!(
            store.join("refs").join(id).is_file(),
            "activation must keep maintaining legacy refs (downgrade safety)"
        );
    }

    let removed = fx.wt_with_store(&["remove", "one"], &store);
    assert!(removed.status.success());
    for id in &ids {
        match fs::read_to_string(store.join("refs").join(id)) {
            Ok(count) => assert_eq!(count.trim(), "0"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => panic!("cannot read ref file: {e}"),
        }
    }
}

#[test]
fn drop_legacy_refs_warns_loudly_purges_refs_and_creates_stop_writing_them() {
    let fx = Fixture::heavy_repo(10);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();
    let base = tempfile::tempdir().expect("tempdir");
    let store = base.path().join("store");

    assert!(fx
        .wt_with_store(&["create", "one"], &store)
        .status
        .success());
    let dropped = fx.wt_with_store(&["store", "migrate", "--drop-legacy-refs"], &store);
    assert!(
        dropped.status.success(),
        "{}",
        String::from_utf8_lossy(&dropped.stderr)
    );
    let stderr = String::from_utf8_lossy(&dropped.stderr);
    assert!(
        stderr.contains("WARNING") && stderr.contains("pre-cutover"),
        "the one-way cutover must warn loudly: {stderr}"
    );
    assert_eq!(
        fs::read_to_string(store.join("gc-mode")).unwrap().trim(),
        "mark-sweep-no-refs"
    );
    // Every ref file purged...
    assert!(
        fs::read_dir(store.join("refs")).unwrap().next().is_none(),
        "refs/ must be empty after the drop"
    );
    // ...and the surviving worktree's blobs stay put via its mirror.
    let ids = ledger_ids(&fx.repo.parent().unwrap().join("origin-one"));
    age_objects(&store);
    let swept = fx.wt_with_store(&["sweep", "--age", "0s"], &store);
    assert!(swept.status.success());
    for id in &ids {
        assert!(
            object_path(&store, id).is_file(),
            "blob {id} collected although its worktree lives"
        );
    }

    // New creates stop touching refs/ entirely.
    assert!(fx
        .wt_with_store(&["create", "two"], &store)
        .status
        .success());
    assert!(
        fs::read_dir(store.join("refs")).unwrap().next().is_none(),
        "create wrote ref files after --drop-legacy-refs"
    );
}
