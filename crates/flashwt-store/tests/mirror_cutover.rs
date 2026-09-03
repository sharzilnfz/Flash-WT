#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::fs;
use std::time::Duration;

use flashwt_store::{
    DiskStore, GcMode, HydrateDest, HydrateOutcome, HydratePinned, HydratePolicy, HydrateReq,
    HydrateSrc, HydrateTree, Ingested, Manifest, PublishOptions, SnapshotEntry, StoreReclaimer,
    hash_lockfile, record_snapshot_publish,
};
use tempfile::TempDir;

#[test]
fn legacy_sweep_does_not_collect_active_pinned_snapshot_member_blobs() {
    let store_dir = TempDir::new().expect("store dir");
    let mut store = DiskStore::open(store_dir.path()).expect("open store");
    assert_eq!(store.gc_mode(), GcMode::Legacy);

    let repo_dir = TempDir::new().expect("repo dir");
    let worktree_dir = TempDir::new().expect("worktree dir");
    let git_dir = TempDir::new().expect("git dir");

    let heavy = repo_dir.path().join("node_modules");
    fs::create_dir_all(heavy.join("pkg")).expect("mkdir");
    let content = b"export const answer = 42;\n";
    fs::write(heavy.join("pkg/index.js"), content).expect("write");
    let blob = store.put(content).expect("put");

    let lock_hash = hash_lockfile(b"lockfile-pinned-v1");
    let entries = vec![SnapshotEntry::file("pkg/index.js", blob, 0o644)];
    let manifest =
        Manifest::new_with_lockfile_and_size(entries, Some(lock_hash), content.len() as u64)
            .expect("manifest");
    let snap_hash = manifest.hash;

    store
        .publish_manifest(
            &manifest,
            PublishOptions::default().lockfile_hash(Some(lock_hash)),
        )
        .expect("publish");

    let repo_key = repo_dir.path().to_string_lossy().into_owned();
    record_snapshot_publish(
        store_dir.path(),
        &repo_key,
        "node_modules/",
        "node_modules",
        &snap_hash,
    )
    .expect("seed ring");

    let req = HydrateReq {
        src: HydrateSrc::PinnedLockfile(HydratePinned {
            repo_root: repo_dir.path(),
            pattern: "node_modules/",
            src_root: repo_dir.path(),
            heavy_rel: "node_modules",
            lockfile_hash: lock_hash,
        }),
        dest: HydrateDest {
            worktree_root: worktree_dir.path(),
            git_dir: git_dir.path(),
            base_branch: None,
            base_commit: None,
        },
        policy: HydratePolicy {
            verify: false,
            snapshots: true,
            v2: false,
            strategy: flashwt_copy::StrategyPolicy::Default,
        },
    };

    let outcome = store.hydrate(req).expect("hydrate");

    #[cfg(target_os = "macos")]
    {
        match outcome {
            HydrateOutcome::Hydrated(receipt) => {
                assert!(receipt.snapshot_hit);
            }
            _ => panic!("expected snapshot hit on macos"),
        }

        assert_eq!(
            store.ref_count(&blob).expect("read ref_count"),
            1,
            "pinned snapshot hydration must record member blob ref in legacy mode"
        );

        let audit = store
            .audit_marks_against_refs(Duration::ZERO)
            .expect("audit");
        assert!(
            audit.is_empty(),
            "mirror marks and refcounts must agree with zero audit discrepancies: {audit:?}"
        );

        let swept = store.sweep(Duration::ZERO).expect("legacy sweep");
        assert_eq!(
            swept.reclaimed, 0,
            "legacy sweep must not collect active snapshot member blobs"
        );
        assert!(
            store.contains(&blob),
            "member blob must remain present in store"
        );
        assert_eq!(
            fs::read(worktree_dir.path().join("node_modules/pkg/index.js"))
                .expect("read hydrated file"),
            content
        );

        let mut reclaimer = StoreReclaimer::new(&mut store);
        let receipt = reclaimer
            .retire_worktree(worktree_dir.path(), git_dir.path())
            .expect("retire worktree");
        assert_eq!(
            receipt.references_released, 1,
            "retiring worktree must release snapshot member blob ref"
        );
        assert_eq!(
            store.ref_count(&blob).expect("read ref_count after retire"),
            0
        );

        let swept_after = store
            .sweep(Duration::ZERO)
            .expect("legacy sweep after retire");
        assert_eq!(
            swept_after.reclaimed, 1,
            "legacy sweep should now collect unreferenced member blob"
        );
        assert!(
            !store.contains(&blob),
            "member blob should now be reclaimed"
        );
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (blob, content, snap_hash, outcome);
    }
}

#[test]
fn legacy_sweep_does_not_collect_active_tree_snapshot_member_blobs() {
    let store_dir = TempDir::new().expect("store dir");
    let mut store = DiskStore::open(store_dir.path()).expect("open store");
    assert_eq!(store.gc_mode(), GcMode::Legacy);

    let repo_dir = TempDir::new().expect("repo dir");
    let worktree_dir = TempDir::new().expect("worktree dir");
    let git_dir = TempDir::new().expect("git dir");

    let c1 = b"first file payload\n";
    let c2 = b"second file payload\n";
    let id1 = store.put(c1).expect("put 1");
    let id2 = store.put(c2).expect("put 2");

    let dirs = vec!["sub".to_string()];
    let mut dir_modes = BTreeMap::new();
    dir_modes.insert("sub".to_string(), 0o755);

    let mut files = BTreeMap::new();
    files.insert("root.js".to_string(), id1);
    files.insert("sub/child.js".to_string(), id2);

    let mut file_sizes = BTreeMap::new();
    file_sizes.insert("root.js".to_string(), c1.len() as u64);
    file_sizes.insert("sub/child.js".to_string(), c2.len() as u64);

    let symlinks = BTreeMap::new();

    let mut modes = BTreeMap::new();
    modes.insert("root.js".to_string(), 0o644);
    modes.insert("sub/child.js".to_string(), 0o644);

    let ingested = Ingested {
        dirs,
        dir_modes,
        files,
        file_sizes,
        symlinks,
        modes,
    };

    let req = HydrateReq {
        src: HydrateSrc::Tree(HydrateTree {
            ingested: &ingested,
            repo_root: repo_dir.path(),
            pattern: "heavy/",
            src_root: repo_dir.path(),
            heavy_rel: "heavy",
            lockfile_hash: None,
        }),
        dest: HydrateDest {
            worktree_root: worktree_dir.path(),
            git_dir: git_dir.path(),
            base_branch: None,
            base_commit: None,
        },
        policy: HydratePolicy {
            verify: false,
            snapshots: true,
            v2: false,
            strategy: flashwt_copy::StrategyPolicy::Default,
        },
    };

    let outcome = store.hydrate(req).expect("hydrate");
    assert!(matches!(outcome, HydrateOutcome::Hydrated(_)));

    assert_eq!(store.ref_count(&id1).expect("ref id1"), 1);
    assert_eq!(store.ref_count(&id2).expect("ref id2"), 1);

    let swept = store.sweep(Duration::ZERO).expect("sweep");
    assert_eq!(
        swept.reclaimed, 0,
        "legacy sweep must not reclaim active tree member blobs"
    );
    assert!(store.contains(&id1));
    assert!(store.contains(&id2));

    let audit = store
        .audit_marks_against_refs(Duration::ZERO)
        .expect("audit");
    assert!(
        audit.is_empty(),
        "audit must find zero discrepancies: {audit:?}"
    );

    let mut reclaimer = StoreReclaimer::new(&mut store);
    let receipt = reclaimer
        .retire_worktree(worktree_dir.path(), git_dir.path())
        .expect("retire");
    assert_eq!(receipt.references_released, 2);
    assert_eq!(store.ref_count(&id1).expect("ref id1"), 0);
    assert_eq!(store.ref_count(&id2).expect("ref id2"), 0);

    let swept_after = store.sweep(Duration::ZERO).expect("sweep after retire");
    assert_eq!(swept_after.reclaimed, 2);
    assert!(!store.contains(&id1));
    assert!(!store.contains(&id2));
}
