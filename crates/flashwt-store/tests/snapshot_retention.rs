#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::fs::FileTimes;
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use flashwt_store::{
    ContentId, DiskStore, Manifest, MarkSwept, PublishOptions, PublishOutcome, SnapshotEntry,
    SnapshotLru,
};

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

fn reference_snapshot(store: &DiskStore, base: &Path, name: &str, hash: &ContentId) {
    let worktree = base.join(name);
    let gitdir = base.join(format!("{name}.gitdir"));
    fs::create_dir_all(&worktree).unwrap();
    fs::create_dir_all(&gitdir).unwrap();
    fs::write(gitdir.join("flashwt-hydrated.tsv"), "").unwrap();
    let no_files: [ContentId; 0] = [];
    store
        .publish_worktree_mirror(
            &worktree,
            &gitdir,
            no_files.iter(),
            std::slice::from_ref(hash),
            None,
            None,
        )
        .unwrap();
}

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

fn stamp_lru(store: &Path, stamps: &[(ContentId, u64)]) {
    SnapshotLru {
        entries: stamps.to_vec(),
    }
    .save_durable(store)
    .unwrap();
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

fn sweep_now(store: &mut DiskStore, cap: usize) -> MarkSwept {
    store
        .sweep_mark_sweep_with_budget(Duration::from_secs(0), cap, None)
        .unwrap()
}

fn sweep_budget(store: &mut DiskStore, cap: usize, max_bytes: Option<u64>) -> MarkSwept {
    store
        .sweep_mark_sweep_with_budget(Duration::from_secs(0), cap, max_bytes)
        .unwrap()
}

#[test]
fn cap_evicts_least_recently_used_and_keeps_most_recent() {
    let base = tempfile::tempdir().unwrap();
    let mut store = DiskStore::open(base.path().join("store")).unwrap();

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

    let swept = sweep_now(&mut store, 2);
    assert_eq!(
        swept.snapshot_dirs_removed, 3,
        "cap evictions count toward dirs removed"
    );
    assert_eq!(swept.snapshot_cap_evicted, 3);
    assert_eq!(surviving_snapshots(store.root()).len(), 2);

    let mut expected: Vec<String> = hashes[3..].iter().map(|h| h.to_string()).collect();
    expected.sort();
    assert_eq!(surviving_snapshots(store.root()), expected);

    assert!(store.root().join("snapshots").join("lru.tsv").is_file());
}

#[test]
fn anti_thrashing_protects_young_snapshots_even_when_budget_is_zero() {
    let base = tempfile::tempdir().unwrap();
    let mut store = DiskStore::open(base.path().join("store")).unwrap();

    let _hashes: Vec<ContentId> = ["a", "b", "c"]
        .iter()
        .map(|t| publish_snapshot(&mut store, t.as_bytes()))
        .collect();

    let swept = store
        .sweep_mark_sweep_with_budget(Duration::from_secs(3600), 0, Some(0))
        .unwrap();
    assert_eq!(swept.snapshot_cap_evicted, 0);
    assert_eq!(surviving_snapshots(store.root()).len(), 3);
}

#[test]
fn byte_budget_evicts_oldest_unreferenced_snapshots_using_manifest_sizes() {
    let base = tempfile::tempdir().unwrap();
    let mut store = DiskStore::open(base.path().join("store")).unwrap();

    let hashes: Vec<ContentId> = (1..=4)
        .map(|i| {
            let data = vec![b'x'; i * 100];
            publish_snapshot(&mut store, &data)
        })
        .collect();

    for (i, h) in hashes.iter().enumerate() {
        let manifest = store.find_snapshot(h).unwrap();
        assert_eq!(manifest.total_size, ((i + 1) * 100) as u64);
    }

    let stamps: Vec<(ContentId, u64)> = hashes
        .iter()
        .enumerate()
        .map(|(i, h)| (*h, 100 * (i as u64 + 1)))
        .collect();
    stamp_lru(store.root(), &stamps);

    let swept = sweep_budget(&mut store, 10, Some(700));
    assert_eq!(swept.snapshot_cap_evicted, 2);
    assert_eq!(surviving_snapshots(store.root()).len(), 2);

    let mut expected: Vec<String> = vec![hashes[2].to_string(), hashes[3].to_string()];
    expected.sort();
    assert_eq!(surviving_snapshots(store.root()), expected);
}

#[test]
fn dual_budget_enforces_both_count_cap_and_byte_limit() {
    let base = tempfile::tempdir().unwrap();
    let mut store = DiskStore::open(base.path().join("store")).unwrap();

    let hashes: Vec<ContentId> = (0..4)
        .map(|i| {
            let mut data = vec![b'y'; 200];
            data[0] = i as u8;
            publish_snapshot(&mut store, &data)
        })
        .collect();

    let stamps: Vec<(ContentId, u64)> = hashes
        .iter()
        .enumerate()
        .map(|(i, h)| (*h, 100 * (i as u64 + 1)))
        .collect();
    stamp_lru(store.root(), &stamps);

    let swept = sweep_budget(&mut store, 3, Some(500));
    assert_eq!(swept.snapshot_cap_evicted, 2);
    assert_eq!(surviving_snapshots(store.root()).len(), 2);
}

#[test]
fn referenced_snapshots_survive_any_cap() {
    let base = tempfile::tempdir().unwrap();
    let mut store = DiskStore::open(base.path().join("store")).unwrap();

    let hashes: Vec<ContentId> = ["a", "b", "c", "d"]
        .iter()
        .map(|t| publish_snapshot(&mut store, t.as_bytes()))
        .collect();

    reference_snapshot(&store, base.path(), "worktree-a", &hashes[0]);
    reference_snapshot(&store, base.path(), "worktree-b", &hashes[1]);

    stamp_lru(
        store.root(),
        &hashes
            .iter()
            .enumerate()
            .map(|(i, h)| (*h, 100 * (i as u64 + 1)))
            .collect::<Vec<_>>(),
    );
    age_all_snapshots(store.root());

    let swept = sweep_now(&mut store, 0);
    assert_eq!(swept.snapshot_dirs_removed, 2);
    assert_eq!(swept.snapshot_cap_evicted, 2);
    let mut expected: Vec<String> = hashes[..2].iter().map(|h| h.to_string()).collect();
    expected.sort();
    assert_eq!(surviving_snapshots(store.root()), expected);
}

#[test]
fn unreferenced_surplus_is_capped_but_referenced_survive() {
    let base = tempfile::tempdir().unwrap();
    let mut store = DiskStore::open(base.path().join("store")).unwrap();

    let hashes: Vec<ContentId> = ["a", "b", "c"]
        .iter()
        .map(|t| publish_snapshot(&mut store, t.as_bytes()))
        .collect();
    reference_snapshot(&store, base.path(), "worktree-a", &hashes[0]);

    stamp_lru(
        store.root(),
        &hashes
            .iter()
            .enumerate()
            .map(|(i, h)| (*h, 100 * (i as u64 + 1)))
            .collect::<Vec<_>>(),
    );

    let swept = sweep_now(&mut store, 0);
    assert_eq!(swept.snapshot_dirs_removed, 2);
    assert_eq!(swept.snapshot_cap_evicted, 2);
    assert!(store.snapshot_path(&hashes[0]).is_dir());
    assert!(!store.snapshot_path(&hashes[1]).exists());
    assert!(!store.snapshot_path(&hashes[2]).exists());
}

#[test]
fn missing_lru_stamps_fall_back_to_publish_mtime_order() {
    let base = tempfile::tempdir().unwrap();
    let mut store = DiskStore::open(base.path().join("store")).unwrap();

    let hashes: Vec<ContentId> = ["a", "b"]
        .iter()
        .map(|t| publish_snapshot(&mut store, t.as_bytes()))
        .collect();

    stamp_lru(store.root(), &[(hashes[1], u64::MAX)]);

    let swept = sweep_now(&mut store, 1);
    assert_eq!(swept.snapshot_dirs_removed, 1);
    assert_eq!(swept.snapshot_cap_evicted, 1);
    assert!(!store.snapshot_path(&hashes[0]).exists());
    assert!(store.snapshot_path(&hashes[1]).is_dir());
}
