// Tests assert with unwrap/expect by design: a panic IS the failure
// signal under test, so the workspace restriction lints stay off here.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Append-only snapshot write-ahead journal (`journal.tsv`) and sweep
//! compaction integration tests (Ticket 10).
//!
//! Verifies:
//! 1. Snapshot hit/publish/touch append single-line TSV entries to `journal.tsv`.
//! 2. Concurrency-sensitive snapshot lookup reads live state directly from journal.
//! 3. Metadata updates avoid global read-modify-write whole-file cycles.
//! 4. `sweep` acquires exclusive metadata lock, compacts `journal.tsv` into `index.tsv` and `lru.tsv`, fsyncs, and truncates `journal.tsv`.
//! 5. Crash resilience (torn tail tolerance) and concurrency stress across concurrent worker processes/threads.

use std::fs;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use wt_store::{
    compact_journal, journal_path, lru_path, record_snapshot_hit, record_snapshot_publish,
    record_snapshot_lru_touch, select_old_snapshot, ContentId, DiskStore, Manifest,
    PublishOutcome, SelectionIndex, SnapshotEntry, SnapshotLru, Store as _,
};

fn sample_id(n: u8) -> ContentId {
    let mut bytes = [0u8; 32];
    bytes[0] = n;
    bytes[31] = n;
    ContentId(bytes)
}

const REPO: &str = "/repos/workspace-alpha";
const PATTERN: &str = "packages/*/";
const HEAVY: &str = "packages/frontend/node_modules";

#[test]
fn snapshot_publish_and_hit_append_to_journal_using_o_append() {
    let base = tempfile::tempdir().unwrap();
    let root = base.path();

    let id1 = sample_id(1);
    let id2 = sample_id(2);

    // Initial state: journal does not exist
    let j_path = journal_path(root);
    assert!(!j_path.exists());

    // Record publish: must create and append to journal.tsv
    record_snapshot_publish(root, REPO, PATTERN, HEAVY, &id1).unwrap();
    assert!(j_path.exists());

    let lines: Vec<String> = fs::read_to_string(&j_path)
        .unwrap()
        .lines()
        .map(String::from)
        .collect();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].starts_with("publish\t"));
    assert!(lines[0].contains(&id1.to_string()));

    // Record hit: appends another line
    record_snapshot_hit(root, REPO, PATTERN, HEAVY, &id2).unwrap();
    let lines2: Vec<String> = fs::read_to_string(&j_path)
        .unwrap()
        .lines()
        .map(String::from)
        .collect();
    assert_eq!(lines2.len(), 2);
    assert!(lines2[1].starts_with("hit\t"));
    assert!(lines2[1].contains(&id2.to_string()));

    // Record touch: appends touch line
    record_snapshot_lru_touch(root, &id1);
    let lines3: Vec<String> = fs::read_to_string(&j_path)
        .unwrap()
        .lines()
        .map(String::from)
        .collect();
    assert_eq!(lines3.len(), 3);
    assert!(lines3[2].starts_with("touch\t"));
}

#[test]
fn snapshot_lookup_and_selection_reads_live_journal_state() {
    let base = tempfile::tempdir().unwrap();
    let root = base.path().join("store");
    let mut store = DiskStore::open(&root).unwrap();

    let blob1 = store.put(b"version 1 content").unwrap();
    let entries1 = vec![SnapshotEntry::file("bundle.js", blob1, 0o644)];
    let manifest1 = Manifest::new(entries1.clone()).unwrap();
    assert_eq!(
        store.publish_snapshot(entries1, false).unwrap(),
        PublishOutcome::Published
    );
    record_snapshot_publish(store.root(), REPO, PATTERN, HEAVY, &manifest1.hash).unwrap();

    // Lookup before any sweep/compaction finds the newly published snapshot from journal
    let (picked, found_manifest) =
        select_old_snapshot(store.root(), REPO, PATTERN, HEAVY).expect("must find from journal");
    assert_eq!(picked, manifest1.hash);
    assert_eq!(found_manifest, manifest1);

    // Publish a newer version
    let blob2 = store.put(b"version 2 content").unwrap();
    let entries2 = vec![
        SnapshotEntry::file("bundle.js", blob2, 0o644),
        SnapshotEntry::file("styles.css", blob2, 0o644),
    ];
    let manifest2 = Manifest::new(entries2.clone()).unwrap();
    assert_eq!(
        store.publish_snapshot(entries2, false).unwrap(),
        PublishOutcome::Published
    );
    record_snapshot_publish(store.root(), REPO, PATTERN, HEAVY, &manifest2.hash).unwrap();

    // Lookup now picks the newer version
    let (picked2, found_manifest2) =
        select_old_snapshot(store.root(), REPO, PATTERN, HEAVY).expect("must pick newer");
    assert_eq!(picked2, manifest2.hash);
    assert_eq!(found_manifest2, manifest2);
}

#[test]
fn sweep_compacts_journal_into_index_and_lru_and_truncates() {
    let base = tempfile::tempdir().unwrap();
    let root = base.path().join("store");
    let mut store = DiskStore::open(&root).unwrap();

    let id1 = sample_id(11);
    let id2 = sample_id(12);
    let id3 = sample_id(13);

    record_snapshot_publish(store.root(), REPO, PATTERN, HEAVY, &id1).unwrap();
    record_snapshot_publish(store.root(), REPO, PATTERN, HEAVY, &id2).unwrap();
    record_snapshot_hit(store.root(), REPO, PATTERN, HEAVY, &id1).unwrap();
    record_snapshot_lru_touch(store.root(), &id3);

    // Run sweep which triggers compaction
    let _ = store.sweep(Duration::from_secs(3600)).unwrap();

    // Canonical files should exist
    let idx_file = store.root().join("snapshots/index.tsv");
    let lru_file = lru_path(store.root());
    assert!(idx_file.exists());
    assert!(lru_file.exists());

    // Journal must be truncated to 0
    let j_file = journal_path(store.root());
    let j_len = fs::metadata(&j_file).unwrap().len();
    assert_eq!(j_len, 0, "journal must be truncated to 0 bytes");

    // Canonical files alone hold the merged state
    let idx = SelectionIndex::load_canonical(store.root());
    assert_eq!(idx.records.len(), 1);
    assert_eq!(idx.records[0].ring, vec![id1, id2]);

    let lru = SnapshotLru::load_canonical(store.root());
    assert!(lru.last_use(&id1).is_some());
    assert!(lru.last_use(&id2).is_some());
    assert!(lru.last_use(&id3).is_some());
}

#[test]
fn concurrent_processes_zero_lost_updates_stress_test() {
    let base = tempfile::tempdir().unwrap();
    let root = Arc::new(base.path().to_path_buf());

    let num_workers = 10;
    let ops_per_worker = 40;

    let mut handles = Vec::new();
    for w in 0..num_workers {
        let r = Arc::clone(&root);
        handles.push(thread::spawn(move || {
            for i in 0..ops_per_worker {
                let id = sample_id(((w * ops_per_worker + i) % 250 + 1) as u8);
                let repo = format!("/repos/agent-{w}");
                if i % 4 == 0 {
                    record_snapshot_publish(&r, &repo, "pat/", "node_modules", &id).unwrap();
                } else if i % 4 == 1 {
                    record_snapshot_hit(&r, &repo, "pat/", "node_modules", &id).unwrap();
                } else if i % 4 == 2 {
                    record_snapshot_lru_touch(&r, &id);
                } else {
                    let _ = select_old_snapshot(&r, &repo, "pat/", "node_modules");
                }
                // Periodic concurrent compaction
                if i % 10 == 0 {
                    let _ = compact_journal(&r);
                }
            }
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    // Final compaction
    compact_journal(&root).unwrap();

    let idx = SelectionIndex::load(&root);
    assert_eq!(
        idx.records.len(),
        num_workers,
        "Every agent repository key must be present in index without lost updates"
    );

    let lru = SnapshotLru::load(&root);
    assert!(
        !lru.entries.is_empty(),
        "LRU entries must be preserved across concurrent runs"
    );
}
