// Tests assert with unwrap/expect by design: a panic IS the failure
// signal under test, so the workspace restriction lints stay off here.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! LRU retention cap for unreferenced snapshots (product-handoff
//! §7.4), asserted against [`wt_store::DiskStore::sweep_mark_sweep`]
//! directly: real published snapshots under `<store>/snapshots/`,
//! last-use stamps in the `lru.tsv` sidecar, and directory mtimes
//! backdated past the grace window. The safety rules under test:
//! only unreferenced AND aged-out snapshots are eligible, referenced
//! ones never count against the cap, and inside-grace directories
//! are never touched however tight the cap.

use std::fs;
use std::fs::FileTimes;
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use wt_store::{
    ContentId, DiskStore, Manifest, MarkSwept, PublishOutcome, SnapshotEntry, SnapshotLru,
};

/// Publish one distinct, valid snapshot carrying `tag`; returns its
/// manifest hash (the directory name under `snapshots/`).
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

/// Make one live worktree mirror whose snapshot record names `hash`.
/// The fake roots exist (so validation passes) and the mirror is
/// fresh, so marks apply inside the grace window.
fn reference_snapshot(store: &DiskStore, base: &Path, name: &str, hash: &ContentId) {
    let worktree = base.join(name);
    let gitdir = base.join(format!("{name}.gitdir"));
    fs::create_dir_all(&worktree).unwrap();
    fs::create_dir_all(&gitdir).unwrap();
    let no_files: [ContentId; 0] = [];
    store
        .publish_worktree_mirror(
            &worktree,
            &gitdir,
            no_files.iter(),
            std::slice::from_ref(hash),
        )
        .unwrap();
}

/// Backdate a snapshot directory's mtime far past any grace window.
fn age_dir(path: &Path) {
    let f = fs::OpenOptions::new().read(true).open(path).unwrap();
    f.set_times(FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(1000)))
        .unwrap();
}

fn age_all_snapshots(store: &Path) {
    for entry in fs::read_dir(store.join("snapshots")).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            age_dir(&path);
        }
    }
}

/// Write explicit last-use stamps into the sidecar.
fn stamp_lru(store: &Path, stamps: &[(ContentId, u64)]) {
    SnapshotLru {
        entries: stamps.to_vec(),
    }
    .save(store)
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

/// One-hour sweep: long enough that backdated (epoch+1000s)
/// directories are past the cutoff while freshly published ones are
/// safely inside the grace window.
fn sweep(store: &mut DiskStore, cap: usize) -> MarkSwept {
    store
        .sweep_mark_sweep(Duration::from_secs(3600), cap)
        .unwrap()
}

#[test]
fn cap_evicts_least_recently_used_and_keeps_most_recent() {
    let base = tempfile::tempdir().unwrap();
    let mut store = DiskStore::open(base.path().join("store")).unwrap();

    // Five unreferenced snapshots, last uses 100..500.
    let hashes: Vec<ContentId> = ["a", "b", "c", "d", "e"]
        .iter()
        .map(|t| publish_snapshot(&mut store, t.as_bytes()))
        .collect();
    let stamps: Vec<(ContentId, u64)> = hashes
        .iter()
        .enumerate()
        .map(|(i, h)| (*h, 100 * (i as u64 + 1)))
        .collect();
    stamp_lru(store.root(), &stamps);
    age_all_snapshots(store.root());

    let swept = sweep(&mut store, 2);
    assert_eq!(surviving_snapshots(store.root()).len(), 2);
    assert_eq!(swept.snapshot_cap_evicted, 3);

    // Exactly the two most-recently-used survive.
    let mut expected: Vec<String> = hashes[3..].iter().map(|h| h.to_string()).collect();
    expected.sort();
    assert_eq!(surviving_snapshots(store.root()), expected);

    // The sidecar itself must never become sweep debris.
    assert!(store.root().join("snapshots").join("lru.tsv").is_file());
}

#[test]
fn referenced_snapshots_survive_any_cap() {
    let base = tempfile::tempdir().unwrap();
    let mut store = DiskStore::open(base.path().join("store")).unwrap();

    let hashes: Vec<ContentId> = ["a", "b", "c", "d"]
        .iter()
        .map(|t| publish_snapshot(&mut store, t.as_bytes()))
        .collect();

    // The two OLDEST snapshots are referenced by a live worktree.
    reference_snapshot(&store, base.path(), "wt-a", &hashes[0]);
    reference_snapshot(&store, base.path(), "wt-b", &hashes[1]);

    let stamps: Vec<(ContentId, u64)> = hashes
        .iter()
        .enumerate()
        .map(|(i, h)| (*h, 100 * (i as u64 + 1)))
        .collect();
    stamp_lru(store.root(), &stamps);
    age_all_snapshots(store.root());

    // Cap zero: every unreferenced aged-out snapshot must go, yet
    // the referenced pair survives untouched.
    let swept = sweep(&mut store, 0);
    assert_eq!(swept.snapshot_cap_evicted, 2);
    let mut expected: Vec<String> = hashes[..2].iter().map(|h| h.to_string()).collect();
    expected.sort();
    assert_eq!(surviving_snapshots(store.root()), expected);
}

#[test]
fn snapshots_inside_grace_are_never_capped_or_collected() {
    let base = tempfile::tempdir().unwrap();
    let mut store = DiskStore::open(base.path().join("store")).unwrap();

    let hashes: Vec<ContentId> = ["a", "b", "c"]
        .iter()
        .map(|t| publish_snapshot(&mut store, t.as_bytes()))
        .collect();
    stamp_lru(
        store.root(),
        &hashes
            .iter()
            .enumerate()
            .map(|(i, h)| (*h, 100 * (i as u64 + 1)))
            .collect::<Vec<_>>(),
    );

    // Only the first snapshot ages out; the others stay inside the
    // grace window (fresh mtimes).
    age_dir(&store.snapshot_path(&hashes[0]));

    // Cap zero cannot touch the young pair: the grace rule dominates
    // the cap, always.
    let swept = sweep(&mut store, 0);
    assert_eq!(swept.snapshot_dirs_removed, 1);
    assert_eq!(surviving_snapshots(store.root()).len(), 2);
    assert!(store.snapshot_path(&hashes[1]).is_dir());
    assert!(store.snapshot_path(&hashes[2]).is_dir());
    assert!(!store.snapshot_path(&hashes[0]).exists());
}

#[test]
fn missing_lru_stamps_fall_back_to_publish_mtime_order() {
    let base = tempfile::tempdir().unwrap();
    let mut store = DiskStore::open(base.path().join("store")).unwrap();

    let hashes: Vec<ContentId> = ["a", "b"]
        .iter()
        .map(|t| publish_snapshot(&mut store, t.as_bytes()))
        .collect();
    // No sidecar at all: both candidates fall back to their
    // directory mtimes. Stamp only the NEWER one explicitly with a
    // very recent time; the unstamped one's backdated mtime counts
    // as ancient, so it must be the cap's victim.
    stamp_lru(store.root(), &[(hashes[1], u64::MAX)]);
    age_all_snapshots(store.root());

    let swept = sweep(&mut store, 1);
    assert_eq!(swept.snapshot_cap_evicted, 1);
    assert!(!store.snapshot_path(&hashes[0]).exists());
    assert!(store.snapshot_path(&hashes[1]).is_dir());
}
