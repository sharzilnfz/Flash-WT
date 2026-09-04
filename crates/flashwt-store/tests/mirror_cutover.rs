#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::time::Duration;

use flashwt_store::{
    DiskStore, GcMode, HydratePolicy, Manifest, PublishOptions, SnapshotEntry, StoreReclaimer,
    WorkspaceHydrateReq, hash_lockfile, record_snapshot_publish,
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

    fs::write(
        repo_dir.path().join("package-lock.json"),
        b"lockfile-pinned-v1",
    )
    .expect("write lock");

    let patterns = vec!["node_modules/".to_string()];
    let req = WorkspaceHydrateReq {
        repo_root: repo_dir.path(),
        worktree_root: worktree_dir.path(),
        git_dir: git_dir.path(),
        patterns: &patterns,
        base_branch: None,
        base_commit: None,
        policy: HydratePolicy {
            verify: false,
            snapshots: true,
            v2: false,
            strategy: flashwt_copy::StrategyPolicy::Default,
        },
    };

    let receipt = store.hydrate_workspace(req).expect("hydrate");

    #[cfg(target_os = "macos")]
    {
        assert!(receipt.snapshot_hit);

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

    let heavy = repo_dir.path().join("heavy");
    fs::create_dir_all(heavy.join("sub")).expect("mkdir heavy/sub");
    let c1 = b"first file payload\n";
    let c2 = b"second file payload\n";
    fs::write(heavy.join("root.js"), c1).expect("write root");
    fs::write(heavy.join("sub/child.js"), c2).expect("write child");

    let id1 = store.put(c1).expect("put 1");
    let id2 = store.put(c2).expect("put 2");

    let patterns = vec!["heavy/".to_string()];
    let req = WorkspaceHydrateReq {
        repo_root: repo_dir.path(),
        worktree_root: worktree_dir.path(),
        git_dir: git_dir.path(),
        patterns: &patterns,
        base_branch: None,
        base_commit: None,
        policy: HydratePolicy {
            verify: false,
            snapshots: true,
            v2: false,
            strategy: flashwt_copy::StrategyPolicy::Default,
        },
    };

    let receipt = store.hydrate_workspace(req).expect("hydrate");
    assert_eq!(receipt.files_total, 2);

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
