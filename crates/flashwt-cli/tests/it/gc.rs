use std::fs;
use std::fs::FileTimes;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::common::{Fixture, list_files};
use flashwt_store::{
    ContentId, DiskStore, Manifest, PublishOptions, PublishOutcome, SnapshotEntry, SnapshotLru,
    WorktreeLease, current_process_start_time, lease_path, publish_lease, read_leases,
};

fn content_files(store: &Path) -> Vec<PathBuf> {
    list_files(store)
        .into_iter()
        .filter(|p| {
            let name = p.file_name();
            name != Some(std::ffi::OsStr::new("ingest-cache.tsv"))
                && name != Some(std::ffi::OsStr::new("verified.tsv"))
                && name != Some(std::ffi::OsStr::new("gc-mode"))
                && !p.starts_with(store.join("worktrees"))
                && !p.starts_with(store.join("snapshots"))
        })
        .collect()
}

fn mirror_files(store: &Path) -> Vec<PathBuf> {
    list_files(&store.join("worktrees"))
}

fn git_dir_of(worktree_root: &Path) -> PathBuf {
    let pointer = fs::read_to_string(worktree_root.join(".git")).expect("worktree .git pointer");
    PathBuf::from(
        pointer
            .trim()
            .strip_prefix("gitdir: ")
            .expect(".git points at a gitdir"),
    )
}

fn ledger_ids(worktree_root: &Path) -> Vec<String> {
    let ledger = fs::read_to_string(git_dir_of(worktree_root).join("flashwt-hydrated.tsv"))
        .expect("hydration ledger exists");
    ledger
        .lines()
        .filter(|l| !l.is_empty())
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

fn object_path(store: &Path, id: &str) -> PathBuf {
    store.join("objects").join(&id[..2]).join(&id[2..])
}

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
    let out = Fixture::heavy_repo(1).flashwt(&["store", "migrate", "--help"]);
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("--activate-mark-sweep"));
    assert!(text.contains("--drop-legacy-refs"));
}

#[test]
fn live_worktree_blobs_survive_mark_sweep_past_any_age() {
    let fx = Fixture::heavy_repo(20);
    fs::write(fx.repo.join(".flashwtinclude"), "heavy/\n").unwrap();
    let base = tempfile::tempdir().expect("tempdir");
    let store = base.path().join("store");

    let created = fx.flashwt_with_store(&["create", "one"], &store);
    assert!(
        created.status.success(),
        "create failed: {}",
        String::from_utf8_lossy(&created.stderr)
    );
    let activated = fx.flashwt_with_store(&["store", "migrate", "--activate-mark-sweep"], &store);
    assert!(activated.status.success());

    age_objects(&store);
    let ids = ledger_ids(&fx.repo.parent().unwrap().join("origin-one"));
    assert!(!ids.is_empty());
    assert_eq!(mirror_files(&store).len(), 1, "one create wrote one mirror");

    let swept = fx.flashwt_with_store(&["sweep", "--age", "0s"], &store);
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

    let again = fx.flashwt_with_store(&["create", "two"], &store);
    assert!(
        again.status.success(),
        "post-sweep create failed: {}",
        String::from_utf8_lossy(&again.stderr)
    );
}

#[test]
fn rm_rf_worktree_without_prune_becomes_collectable_after_grace() {
    let fx = Fixture::heavy_repo(10);
    fs::write(fx.repo.join(".flashwtinclude"), "heavy/\n").unwrap();
    let base = tempfile::tempdir().expect("tempdir");
    let store = base.path().join("store");

    let created = fx.flashwt_with_store(&["create", "doomed"], &store);
    assert!(created.status.success());
    let worktree_root = fx.repo.parent().unwrap().join("origin-doomed");
    let ids = ledger_ids(&worktree_root);

    fs::remove_dir_all(&worktree_root).expect("rm -rf the worktree");

    let activated = fx.flashwt_with_store(&["store", "migrate", "--activate-mark-sweep"], &store);
    assert!(activated.status.success());

    age_objects(&store);
    let swept = fx.flashwt_with_store(&["sweep", "--age", "1h"], &store);
    assert!(swept.status.success());
    for id in &ids {
        assert!(
            object_path(&store, id).is_file(),
            "{id} collected inside grace"
        );
    }

    for mirror in mirror_files(&store) {
        age_file(&mirror);
    }
    let swept = fx.flashwt_with_store(&["sweep", "--age", "0s"], &store);
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

#[test]
fn torn_final_line_is_ignored_and_live_content_survives() {
    let fx = Fixture::heavy_repo(10);
    fs::write(fx.repo.join(".flashwtinclude"), "heavy/\n").unwrap();
    let base = tempfile::tempdir().expect("tempdir");
    let store = base.path().join("store");

    assert!(
        fx.flashwt_with_store(&["create", "one"], &store)
            .status
            .success()
    );
    let ids = ledger_ids(&fx.repo.parent().unwrap().join("origin-one"));

    let mirror = &mirror_files(&store)[0];
    let mut text = fs::read_to_string(mirror).expect("read mirror");
    assert!(text.ends_with('\n'));
    text.push_str("file\tdeadbeef");
    fs::write(mirror, text).expect("tear the mirror");

    assert!(
        fx.flashwt_with_store(&["store", "migrate", "--activate-mark-sweep"], &store)
            .status
            .success()
    );
    age_objects(&store);

    let swept = fx.flashwt_with_store(&["sweep", "--age", "0s"], &store);
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
    fs::write(fx.repo.join(".flashwtinclude"), "heavy/\n").unwrap();
    let base = tempfile::tempdir().expect("tempdir");
    let store = base.path().join("store");

    assert!(
        fx.flashwt_with_store(&["create", "one"], &store)
            .status
            .success()
    );
    let ids = ledger_ids(&fx.repo.parent().unwrap().join("origin-one"));

    let mut mirrors = mirror_files(&store);
    let mirror = mirrors.remove(0);
    fs::write(&mirror, "not a mirror at all\n").expect("corrupt mirror");

    assert!(
        fx.flashwt_with_store(&["store", "migrate", "--activate-mark-sweep"], &store)
            .status
            .success()
    );
    age_objects(&store);

    let swept = fx.flashwt_with_store(&["sweep", "--age", "1h"], &store);
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
    assert_eq!(mirror_files(&store).len(), 1);
}

#[test]
fn missing_mirror_still_allows_remove_and_leaves_no_mirror_behind() {
    let fx = Fixture::heavy_repo(10);
    fs::write(fx.repo.join(".flashwtinclude"), "heavy/\n").unwrap();
    let base = tempfile::tempdir().expect("tempdir");
    let store = base.path().join("store");

    assert!(
        fx.flashwt_with_store(&["create", "one"], &store)
            .status
            .success()
    );
    let worktree_root = fx.repo.parent().unwrap().join("origin-one");
    let refs_before = ledger_ids(&worktree_root);

    fs::remove_file(&mirror_files(&store)[0]).expect("delete mirror");

    let removed = fx.flashwt_with_store(&["remove", "one"], &store);
    assert!(
        removed.status.success(),
        "remove without mirror failed: {}",
        String::from_utf8_lossy(&removed.stderr)
    );
    assert!(!worktree_root.exists());
    assert!(
        mirror_files(&store).is_empty(),
        "no mirror may survive remove"
    );

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
    fs::write(fx.repo.join(".flashwtinclude"), "heavy/\n").unwrap();
    let base = tempfile::tempdir().expect("tempdir");
    let store = base.path().join("store");

    assert!(
        fx.flashwt_with_store(&["create", "one"], &store)
            .status
            .success()
    );
    assert!(
        fx.flashwt_with_store(&["store", "migrate", "--activate-mark-sweep"], &store)
            .status
            .success()
    );

    let worktree_root = fx.repo.parent().unwrap().join("origin-one");
    let ids = ledger_ids(&worktree_root);
    fs::remove_file(object_path(&store, &ids[0])).expect("simulate partial sweep");

    let swept = fx.flashwt_with_store(&["sweep", "--age", "1h"], &store);
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

    let again = fx.flashwt_with_store(&["create", "two"], &store);
    assert!(
        again.status.success(),
        "{}",
        String::from_utf8_lossy(&again.stderr)
    );
}

#[test]
fn timing_env_emits_flashwt_stage_lines_to_stderr_and_stays_silent_without_it() {
    let fx = Fixture::heavy_repo(10);
    fs::write(fx.repo.join(".flashwtinclude"), "heavy/\n").unwrap();
    let base = tempfile::tempdir().expect("tempdir");
    let store = base.path().join("store");

    let quiet = fx.flashwt_with_store(&["create", "quiet"], &store);
    assert!(quiet.status.success());
    assert!(
        !String::from_utf8_lossy(&quiet.stderr).contains("flashwt-stage"),
        "no stage lines without FLASHWT_TIMING"
    );

    let timed = Command::new(env!("CARGO_BIN_EXE_flashwt"))
        .args(["create", "loud"])
        .env("FLASHWT_STORE", &store)
        .env("FLASHWT_TIMING", "1")
        .current_dir(&fx.repo)
        .output()
        .expect("run flashwt binary");
    assert!(timed.status.success());
    let stderr = String::from_utf8_lossy(&timed.stderr);
    let stages: Vec<&str> = stderr
        .lines()
        .filter(|l| l.starts_with("flashwt-stage "))
        .collect();
    let names: Vec<&str> = stages
        .iter()
        .filter_map(|l| l.split_once('=').map(|(n, _)| n))
        .collect();
    let ms: Vec<Option<u64>> = stages
        .iter()
        .filter(|l| !l.starts_with("flashwt-stage snapshot-mode="))
        .map(|l| l.split('=').nth(1).and_then(|v| v.parse().ok()))
        .collect();
    assert!(
        ms.iter().all(|v| v.is_some()),
        "stage values must be integer milliseconds:\n{stderr}"
    );
    let legacy = [
        "flashwt-stage ingest",
        "flashwt-stage references",
        "flashwt-stage materialize",
        "flashwt-stage total",
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

#[test]
fn legacy_sweep_audit_reports_zero_disagreements_on_normal_fixtures() {
    let fx = Fixture::heavy_repo(30);
    fs::write(fx.repo.join(".flashwtinclude"), "heavy/\n").unwrap();
    let base = tempfile::tempdir().expect("tempdir");
    let store = base.path().join("store");

    for name in ["one", "two"] {
        assert!(
            fx.flashwt_with_store(&["create", name], &store)
                .status
                .success()
        );
    }
    let removed = fx.flashwt_with_store(&["remove", "one"], &store);
    assert!(removed.status.success());

    let swept = fx.flashwt_with_store(&["sweep", "--age", "0s"], &store);
    assert!(swept.status.success());
    let stderr = String::from_utf8_lossy(&swept.stderr);
    assert!(
        !stderr.contains("flashwt-gc-audit"),
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
    fs::write(fx.repo.join(".flashwtinclude"), "heavy/\n").unwrap();
    let base = tempfile::tempdir().expect("tempdir");
    let store = base.path().join("store");

    assert!(
        fx.flashwt_with_store(&["store", "migrate", "--activate-mark-sweep"], &store)
            .status
            .success()
    );
    assert_eq!(
        fs::read_to_string(store.join("gc-mode")).unwrap().trim(),
        "mark-sweep"
    );

    assert!(
        fx.flashwt_with_store(&["create", "one"], &store)
            .status
            .success()
    );
    let ids = ledger_ids(&fx.repo.parent().unwrap().join("origin-one"));
    for id in &ids {
        assert!(
            store.join("refs").join(id).is_file(),
            "activation must keep maintaining legacy refs (downgrade safety)"
        );
    }

    let removed = fx.flashwt_with_store(&["remove", "one"], &store);
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
    fs::write(fx.repo.join(".flashwtinclude"), "heavy/\n").unwrap();
    let base = tempfile::tempdir().expect("tempdir");
    let store = base.path().join("store");

    assert!(
        fx.flashwt_with_store(&["create", "one"], &store)
            .status
            .success()
    );
    let dropped = fx.flashwt_with_store(&["store", "migrate", "--drop-legacy-refs"], &store);
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
    assert!(
        fs::read_dir(store.join("refs")).unwrap().next().is_none(),
        "refs/ must be empty after the drop"
    );
    let ids = ledger_ids(&fx.repo.parent().unwrap().join("origin-one"));
    age_objects(&store);
    let swept = fx.flashwt_with_store(&["sweep", "--age", "0s"], &store);
    assert!(swept.status.success());
    for id in &ids {
        assert!(
            object_path(&store, id).is_file(),
            "blob {id} collected although its worktree lives"
        );
    }

    assert!(
        fx.flashwt_with_store(&["create", "two"], &store)
            .status
            .success()
    );
    assert!(
        fs::read_dir(store.join("refs")).unwrap().next().is_none(),
        "create wrote ref files after --drop-legacy-refs"
    );
}

fn publish_snapshot(store: &mut DiskStore, tag: &[u8]) -> ContentId {
    let blob = store.put(tag).unwrap();
    let entries = vec![SnapshotEntry::file("f.bin", blob, 0o644)];
    let manifest = Manifest::new(entries.clone()).unwrap();
    assert_eq!(
        store
            .publish_snapshot(entries, PublishOptions::default())
            .unwrap()
            .outcome,
        PublishOutcome::Published
    );
    manifest.hash
}

fn surviving_snapshots(store: &Path) -> Vec<String> {
    let mut names = Vec::new();
    for entry in fs::read_dir(store.join("snapshots")).unwrap() {
        let name = entry.unwrap().file_name().to_string_lossy().into_owned();
        if ContentId::from_hex(&name).is_some() {
            names.push(name);
        }
    }
    names.sort();
    names
}

fn capped_store(count: usize) -> (tempfile::TempDir, Vec<ContentId>) {
    let base = tempfile::tempdir().unwrap();
    let mut store = DiskStore::open(base.path().join("store")).unwrap();
    let hashes: Vec<ContentId> = (0..count)
        .map(|i| publish_snapshot(&mut store, format!("snapshot {i}").as_bytes()))
        .collect();
    SnapshotLru {
        entries: hashes
            .iter()
            .enumerate()
            .map(|(i, h)| (*h, 100 * (i as u64 + 1)))
            .collect(),
    }
    .save_durable(store.root())
    .unwrap();
    fs::write(base.path().join("store").join("gc-mode"), "mark-sweep\n").unwrap();
    (base, hashes)
}

#[test]
fn without_the_env_the_generous_default_keeps_every_snapshot() {
    let fx = Fixture::heavy_repo(1);
    let (base, hashes) = capped_store(4);
    let store = base.path().join("store");

    let swept = fx.flashwt_with_store(&["sweep", "--age", "1h"], &store);
    assert!(
        swept.status.success(),
        "{}",
        String::from_utf8_lossy(&swept.stderr)
    );
    assert_eq!(surviving_snapshots(&store).len(), hashes.len());
    assert!(
        String::from_utf8_lossy(&swept.stdout).contains("cap evicted 0"),
        "default cap must not evict:\n{}",
        String::from_utf8_lossy(&swept.stdout)
    );
}

#[test]
fn env_override_caps_unreferenced_snapshots_keeping_mru() {
    let fx = Fixture::heavy_repo(1);
    let (base, hashes) = capped_store(4);
    let store = base.path().join("store");

    let swept = fx.flashwt_with_store_env(
        &["sweep", "--age", "0s"],
        &store,
        &[("FLASHWT_SNAPSHOT_CAP", "2")],
    );
    assert!(
        swept.status.success(),
        "{}",
        String::from_utf8_lossy(&swept.stderr)
    );
    let stdout = String::from_utf8_lossy(&swept.stdout);
    assert!(stdout.contains("cap evicted 2"), "stdout: {stdout}");

    let mut expected: Vec<String> = hashes[2..].iter().map(|h| h.to_string()).collect();
    expected.sort();
    assert_eq!(surviving_snapshots(&store), expected);
    assert!(store.join("snapshots").join("lru.tsv").is_file());
}

#[test]
fn anti_thrashing_grace_window_protects_young_snapshots_from_budget_eviction() {
    let fx = Fixture::heavy_repo(1);
    let (base, hashes) = capped_store(4);
    let store = base.path().join("store");

    let swept = fx.flashwt_with_store_env(
        &["sweep", "--age", "1h"],
        &store,
        &[
            ("FLASHWT_SNAPSHOT_CAP", "1"),
            ("FLASHWT_MAX_SNAPSHOT_BYTES", "10B"),
        ],
    );
    assert!(
        swept.status.success(),
        "{}",
        String::from_utf8_lossy(&swept.stderr)
    );
    let stdout = String::from_utf8_lossy(&swept.stdout);
    assert!(
        stdout.contains("cap evicted 0"),
        "anti-thrashing must protect young snapshots: {stdout}"
    );
    assert_eq!(surviving_snapshots(&store).len(), hashes.len());
}

#[test]
fn byte_budget_env_evicts_oldest_unreferenced_snapshots_once_threshold_exceeded() {
    let fx = Fixture::heavy_repo(1);
    let base = tempfile::tempdir().unwrap();
    let mut store = DiskStore::open(base.path().join("store")).unwrap();

    let payload = vec![b'a'; 1000];
    let hashes: Vec<ContentId> = (0..4)
        .map(|i| {
            let mut p = payload.clone();
            p[0] = i as u8;
            publish_snapshot(&mut store, &p)
        })
        .collect();

    SnapshotLru {
        entries: hashes
            .iter()
            .enumerate()
            .map(|(i, h)| (*h, 100 * (i as u64 + 1)))
            .collect(),
    }
    .save_durable(store.root())
    .unwrap();
    fs::write(base.path().join("store").join("gc-mode"), "mark-sweep\n").unwrap();

    let store_path = base.path().join("store");

    let swept = fx.flashwt_with_store_env(
        &["sweep", "--age", "0s"],
        &store_path,
        &[("FLASHWT_MAX_SNAPSHOT_BYTES", "2500B")],
    );
    assert!(
        swept.status.success(),
        "{}",
        String::from_utf8_lossy(&swept.stderr)
    );
    let stdout = String::from_utf8_lossy(&swept.stdout);
    assert!(stdout.contains("cap evicted 2"), "stdout: {stdout}");

    let mut expected: Vec<String> = hashes[2..].iter().map(|h| h.to_string()).collect();
    expected.sort();
    assert_eq!(surviving_snapshots(&store_path), expected);
}

#[test]
fn dual_budget_count_and_bytes_operate_alongside() {
    let fx = Fixture::heavy_repo(1);
    let base = tempfile::tempdir().unwrap();
    let mut store = DiskStore::open(base.path().join("store")).unwrap();

    let payload = vec![b'b'; 500];
    let hashes: Vec<ContentId> = (0..4)
        .map(|i| {
            let mut p = payload.clone();
            p[0] = i as u8;
            publish_snapshot(&mut store, &p)
        })
        .collect();

    SnapshotLru {
        entries: hashes
            .iter()
            .enumerate()
            .map(|(i, h)| (*h, 100 * (i as u64 + 1)))
            .collect(),
    }
    .save_durable(store.root())
    .unwrap();
    fs::write(base.path().join("store").join("gc-mode"), "mark-sweep\n").unwrap();

    let store_path = base.path().join("store");

    let swept = fx.flashwt_with_store_env(
        &["sweep", "--age", "0s"],
        &store_path,
        &[
            ("FLASHWT_SNAPSHOT_CAP", "3"),
            ("FLASHWT_MAX_SNAPSHOT_BYTES", "1200B"),
        ],
    );
    assert!(
        swept.status.success(),
        "{}",
        String::from_utf8_lossy(&swept.stderr)
    );
    let stdout = String::from_utf8_lossy(&swept.stdout);
    assert!(stdout.contains("cap evicted 2"), "stdout: {stdout}");

    let mut expected: Vec<String> = hashes[2..].iter().map(|h| h.to_string()).collect();
    expected.sort();
    assert_eq!(surviving_snapshots(&store_path), expected);
}

#[test]
fn invalid_cap_value_fails_loudly_like_other_flashwt_knobs() {
    let fx = Fixture::heavy_repo(1);
    let (base, _hashes) = capped_store(2);
    let store = base.path().join("store");

    let swept = fx.flashwt_with_store_env(
        &["sweep", "--age", "0s"],
        &store,
        &[("FLASHWT_SNAPSHOT_CAP", "banana")],
    );
    assert!(!swept.status.success());
    let stderr = String::from_utf8_lossy(&swept.stderr);
    assert!(stderr.contains("FLASHWT_SNAPSHOT_CAP"));
}

#[test]
fn invalid_max_snapshot_bytes_value_fails_loudly() {
    let fx = Fixture::heavy_repo(1);
    let (base, _hashes) = capped_store(2);
    let store = base.path().join("store");

    let swept = fx.flashwt_with_store_env(
        &["sweep", "--age", "0s"],
        &store,
        &[("FLASHWT_MAX_SNAPSHOT_BYTES", "invalid_size_str")],
    );
    assert!(!swept.status.success());
    let stderr = String::from_utf8_lossy(&swept.stderr);
    assert!(stderr.contains("FLASHWT_MAX_SNAPSHOT_BYTES"));
}

const LEASE_HEAVY_FILES: usize = 20;

#[test]
fn dead_process_lease_is_reaped_by_sweep() {
    let store_dir = tempfile::tempdir().unwrap();
    let fx = Fixture::heavy_repo(LEASE_HEAVY_FILES);
    fs::write(fx.repo.join(".flashwtinclude"), "heavy/\n").unwrap();

    let out = fx.flashwt_with_store(&["scratch", "--json"], store_dir.path());
    assert!(out.status.success());
    let json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).unwrap();
    let branch = json["data"]["branch"].as_str().unwrap().to_string();
    let lease_id = json["data"]["lease_id"].as_str().unwrap().to_string();
    let worktree_path = PathBuf::from(json["data"]["worktree_path"].as_str().unwrap());
    let lease_file = PathBuf::from(json["data"]["lease_file"].as_str().unwrap());

    assert!(worktree_path.exists());
    assert!(lease_file.exists());

    let git_dir = fx.repo.join(".git").join("worktrees").join(&branch);
    let dead_lease = WorktreeLease::new(
        &lease_id,
        worktree_path.clone(),
        git_dir.clone(),
        999_999_999,
        12345,
        1900000000,
    );
    publish_lease(store_dir.path(), &dead_lease).unwrap();

    let sweep_out = fx.flashwt_with_store(&["sweep", "--age", "0s", "--json"], store_dir.path());
    assert!(sweep_out.status.success());
    let sweep_json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&sweep_out.stdout).trim()).unwrap();
    assert_eq!(sweep_json["command"], "sweep");
    assert_eq!(sweep_json["status"], "ok");
    assert_eq!(sweep_json["data"]["leases_examined"], 1);
    assert_eq!(sweep_json["data"]["leases_reclaimed"], 1);
    assert!(
        sweep_json["data"]["lease_bytes_reclaimed"]
            .as_u64()
            .unwrap()
            > 0
    );

    assert!(!lease_file.exists());
    assert!(!worktree_path.exists());
    assert!(!git_dir.exists());

    let branch_check = Command::new("git")
        .args(["rev-parse", "--verify", &branch])
        .current_dir(&fx.repo)
        .output()
        .unwrap();
    assert!(!branch_check.status.success());
    assert_eq!(read_leases(store_dir.path()).len(), 0);
}

#[test]
fn shifted_start_time_pid_reuse_is_reaped() {
    let store_dir = tempfile::tempdir().unwrap();
    let fx = Fixture::heavy_repo(LEASE_HEAVY_FILES);
    fs::write(fx.repo.join(".flashwtinclude"), "heavy/\n").unwrap();

    let out = fx.flashwt_with_store(&["scratch", "--json"], store_dir.path());
    assert!(out.status.success());
    let json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).unwrap();
    let branch = json["data"]["branch"].as_str().unwrap().to_string();
    let lease_id = json["data"]["lease_id"].as_str().unwrap().to_string();
    let worktree_path = PathBuf::from(json["data"]["worktree_path"].as_str().unwrap());

    let git_dir = fx.repo.join(".git").join("worktrees").join(&branch);
    let my_pid = std::process::id();
    let reused_pid_lease = WorktreeLease::new(
        &lease_id,
        worktree_path.clone(),
        git_dir.clone(),
        my_pid,
        1,
        1900000000,
    );
    publish_lease(store_dir.path(), &reused_pid_lease).unwrap();

    let sweep_out = fx.flashwt_with_store(&["sweep", "--age", "0s", "--json"], store_dir.path());
    assert!(sweep_out.status.success());
    let sweep_json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&sweep_out.stdout).trim()).unwrap();
    assert_eq!(sweep_json["data"]["leases_reclaimed"], 1);

    assert!(!worktree_path.exists());
    assert_eq!(read_leases(store_dir.path()).len(), 0);
}

#[test]
fn expired_ttl_lease_is_reaped_even_with_live_pid() {
    let store_dir = tempfile::tempdir().unwrap();
    let fx = Fixture::heavy_repo(LEASE_HEAVY_FILES);
    fs::write(fx.repo.join(".flashwtinclude"), "heavy/\n").unwrap();

    let out = fx.flashwt_with_store(&["scratch", "--json"], store_dir.path());
    assert!(out.status.success());
    let json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).unwrap();
    let branch = json["data"]["branch"].as_str().unwrap().to_string();
    let lease_id = json["data"]["lease_id"].as_str().unwrap().to_string();
    let worktree_path = PathBuf::from(json["data"]["worktree_path"].as_str().unwrap());

    let now_secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let git_dir = fx.repo.join(".git").join("worktrees").join(&branch);
    let expired_lease = WorktreeLease::new(
        &lease_id,
        worktree_path.clone(),
        git_dir.clone(),
        std::process::id(),
        current_process_start_time(),
        now_secs.saturating_sub(60),
    );
    publish_lease(store_dir.path(), &expired_lease).unwrap();

    let sweep_out = fx.flashwt_with_store(&["sweep", "--age", "0s", "--json"], store_dir.path());
    assert!(sweep_out.status.success());
    let sweep_json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&sweep_out.stdout).trim()).unwrap();
    assert_eq!(sweep_json["data"]["leases_reclaimed"], 1);

    assert!(!worktree_path.exists());
    assert_eq!(read_leases(store_dir.path()).len(), 0);
}

#[test]
fn active_lease_is_protected_from_sweep() {
    let store_dir = tempfile::tempdir().unwrap();
    let fx = Fixture::heavy_repo(LEASE_HEAVY_FILES);
    fs::write(fx.repo.join(".flashwtinclude"), "heavy/\n").unwrap();

    let out = fx.flashwt_with_store(&["scratch", "--json"], store_dir.path());
    assert!(out.status.success());
    let json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).unwrap();
    let branch = json["data"]["branch"].as_str().unwrap().to_string();
    let worktree_path = PathBuf::from(json["data"]["worktree_path"].as_str().unwrap());

    let sweep_out = fx.flashwt_with_store(&["sweep", "--age", "0s", "--json"], store_dir.path());
    assert!(sweep_out.status.success());
    let sweep_json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&sweep_out.stdout).trim()).unwrap();
    assert_eq!(sweep_json["data"]["leases_examined"], 1);
    assert_eq!(sweep_json["data"]["leases_reclaimed"], 0);
    assert_eq!(sweep_json["data"]["lease_bytes_reclaimed"], 0);

    assert!(worktree_path.exists());
    assert_eq!(read_leases(store_dir.path()).len(), 1);

    let _ = fx.flashwt_with_store(&["remove", &branch], store_dir.path());
}

#[test]
fn sweep_reclaims_unreferenced_blobs_of_reaped_lease() {
    let store_dir = tempfile::tempdir().unwrap();
    let fx = Fixture::heavy_repo(LEASE_HEAVY_FILES);
    fs::write(fx.repo.join(".flashwtinclude"), "heavy/\n").unwrap();

    let _ = fx.flashwt_with_store(
        &["store", "migrate", "--activate-mark-sweep"],
        store_dir.path(),
    );

    let out = fx.flashwt_with_store(&["scratch", "--json"], store_dir.path());
    assert!(out.status.success());
    let json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).unwrap();
    let branch = json["data"]["branch"].as_str().unwrap().to_string();
    let lease_id = json["data"]["lease_id"].as_str().unwrap().to_string();
    let worktree_path = PathBuf::from(json["data"]["worktree_path"].as_str().unwrap());
    let git_dir = fx.repo.join(".git").join("worktrees").join(&branch);

    let dead_lease = WorktreeLease::new(
        &lease_id,
        worktree_path.clone(),
        git_dir.clone(),
        999_999_999,
        0,
        1900000000,
    );
    publish_lease(store_dir.path(), &dead_lease).unwrap();

    let sweep_out = fx.flashwt_with_store(&["sweep", "--age", "0s", "--json"], store_dir.path());
    assert!(sweep_out.status.success());
    let sweep_json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&sweep_out.stdout).trim()).unwrap();

    assert_eq!(sweep_json["data"]["leases_reclaimed"], 1);
    assert_eq!(sweep_json["data"]["mirrors_removed"], 1);
    assert!(sweep_json["data"]["reclaimed"].as_u64().unwrap() >= LEASE_HEAVY_FILES as u64);
}

#[test]
fn concurrent_lease_sweeping_stress() {
    let store_dir = tempfile::tempdir().unwrap();
    let store_path = Arc::new(store_dir.path().to_path_buf());

    let mut handles = Vec::new();
    for i in 0..6 {
        let store_p = Arc::clone(&store_path);
        handles.push(std::thread::spawn(move || {
            let fx = Fixture::heavy_repo(10);
            fs::write(fx.repo.join(".flashwtinclude"), "heavy/\n").unwrap();
            let name = format!("concurrent-dead-{i}");
            let out = fx.flashwt_with_store(&["scratch", &name, "--json"], &store_p);
            assert!(out.status.success());

            let lease_p = lease_path(&store_p, &name);
            let text = fs::read_to_string(&lease_p).unwrap();
            let mut lease = WorktreeLease::parse(&name, &text).unwrap();
            lease.pid = 999_999_000 + i;
            publish_lease(&store_p, &lease).unwrap();

            let sweep = fx.flashwt_with_store(&["sweep", "--age", "0s", "--json"], &store_p);
            assert!(
                sweep.status.success(),
                "sweep failed: stdout={}, stderr={}",
                String::from_utf8_lossy(&sweep.stdout),
                String::from_utf8_lossy(&sweep.stderr)
            );
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    let fx_final = Fixture::heavy_repo(5);
    let final_sweep =
        fx_final.flashwt_with_store(&["sweep", "--age", "0s", "--json"], store_path.as_path());
    assert!(final_sweep.status.success());
    assert_eq!(read_leases(store_path.as_path()).len(), 0);
}
