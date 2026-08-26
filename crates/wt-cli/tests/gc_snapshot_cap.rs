// Tests assert with unwrap/expect by design: a panic IS the failure
// signal under test, so the workspace restriction lints stay off here.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! `WT_SNAPSHOT_CAP`: the LRU retention cap for unreferenced
//! snapshots (product-handoff §7.4), asserted through the CLI seam
//! like tests/gc_mirror.rs. The store is built with the real library
//! (publishing works on every platform; only snapshot HYDRATION is
//! macOS-only), then `wt sweep` runs in mark-sweep mode with the env
//! knob pointed at it.

mod common;

use std::fs;
use std::fs::FileTimes;
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use common::Fixture;
use wt_store::{ContentId, DiskStore, Manifest, PublishOutcome, SnapshotEntry, SnapshotLru};

/// Publish one distinct, valid unreferenced snapshot carrying `tag`.
fn publish_snapshot(store: &mut DiskStore, tag: &[u8]) -> ContentId {
    use wt_store::Store as _;

    let blob = store.put(tag).unwrap();
    let entries = vec![SnapshotEntry::file("f.bin", blob, 0o644)];
    let manifest = Manifest::new(entries.clone()).unwrap();
    assert_eq!(
        store.publish_snapshot(entries, false).unwrap(),
        PublishOutcome::Published
    );
    manifest.hash
}

fn age_dir(path: &Path) {
    let f = fs::OpenOptions::new().read(true).open(path).unwrap();
    f.set_times(FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(1000)))
        .unwrap();
}

/// Hex-named snapshot directories still on disk, sorted.
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

/// A mark-sweep store holding `count` aged-out unreferenced
/// snapshots with strictly increasing last-use stamps.
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
    .save(store.root())
    .unwrap();
    for entry in fs::read_dir(store.root().join("snapshots")).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            age_dir(&path);
        }
    }
    fs::write(base.path().join("store").join("gc-mode"), "mark-sweep\n").unwrap();
    (base, hashes)
}

#[test]
fn without_the_env_the_generous_default_keeps_every_snapshot() {
    let fx = Fixture::heavy_repo(1);
    let (base, hashes) = capped_store(4);
    let store = base.path().join("store");

    let swept = fx.wt_with_store(&["sweep", "--age", "0s"], &store);
    assert!(
        swept.status.success(),
        "{}",
        String::from_utf8_lossy(&swept.stderr)
    );
    // Default cap 64 >> 4: nothing evicted by the cap.
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

    let swept = fx.wt_with_store_env(
        &["sweep", "--age", "0s"],
        &store,
        &[("WT_SNAPSHOT_CAP", "2")],
    );
    assert!(
        swept.status.success(),
        "{}",
        String::from_utf8_lossy(&swept.stderr)
    );
    let stdout = String::from_utf8_lossy(&swept.stdout);
    assert!(stdout.contains("cap evicted 2"), "stdout: {stdout}");

    // Exactly the two most-recently-used survive.
    let mut expected: Vec<String> = hashes[2..].iter().map(|h| h.to_string()).collect();
    expected.sort();
    assert_eq!(surviving_snapshots(&store), expected);

    // The sidecar survives its own sweep.
    assert!(store.join("snapshots").join("lru.tsv").is_file());
}

#[test]
fn invalid_cap_value_fails_loudly_like_other_wt_knobs() {
    let fx = Fixture::heavy_repo(1);
    let (base, _hashes) = capped_store(2);
    let store = base.path().join("store");

    let swept = fx.wt_with_store_env(
        &["sweep", "--age", "0s"],
        &store,
        &[("WT_SNAPSHOT_CAP", "banana")],
    );
    assert!(!swept.status.success(), "garbage cap must fail the sweep");
    let stderr = String::from_utf8_lossy(&swept.stderr);
    assert!(
        stderr.contains("WT_SNAPSHOT_CAP"),
        "error must name the knob: {stderr}"
    );
}
