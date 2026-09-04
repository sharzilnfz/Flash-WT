#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use flashwt_store::{
    DiskStore, HydratePolicy, StoreReclaimer, SweepPolicy, WorkspaceHydrateReq,
    ZERO_SAVINGS_NO_FILES_HYDRATED, ZERO_SAVINGS_NO_MATCHING_DIRS,
};
use tempfile::TempDir;

#[test]
fn fallback_materialization_creates_files_dirs_and_metadata() {
    let store_dir = TempDir::new().expect("store dir");
    let mut store = DiskStore::open(store_dir.path()).expect("open store");

    let repo_dir = TempDir::new().expect("repo dir");
    let worktree_dir = TempDir::new().expect("worktree dir");
    let git_dir = TempDir::new().expect("git dir");

    let c1 = b"console.log('hello from index.js');\n";
    let c2 = b"#!/usr/bin/env node\nconsole.log('binary tool');\n";
    let c3 = b"{\"name\": \"pkg\", \"version\": \"1.0.0\"}\n";

    let heavy = repo_dir.path().join("node_modules");
    fs::create_dir_all(heavy.join("bin")).expect("mkdir bin");
    fs::create_dir_all(heavy.join("nested/sub")).expect("mkdir nested/sub");
    fs::write(heavy.join("index.js"), c1).expect("write index.js");
    fs::write(heavy.join("bin/cli"), c2).expect("write bin/cli");
    fs::write(heavy.join("nested/sub/package.json"), c3).expect("write pkg.json");

    #[cfg(unix)]
    {
        fs::set_permissions(&heavy.join("bin"), fs::Permissions::from_mode(0o755)).expect("chmod bin");
        fs::set_permissions(&heavy.join("nested/sub"), fs::Permissions::from_mode(0o755)).expect("chmod nested/sub");
        fs::set_permissions(&heavy.join("bin/cli"), fs::Permissions::from_mode(0o755)).expect("chmod bin/cli");
        std::os::unix::fs::symlink("cli", heavy.join("bin/symlink-cli")).expect("symlink");
    }

    let patterns = vec!["node_modules/".to_string()];
    let req = WorkspaceHydrateReq {
        repo_root: repo_dir.path(),
        worktree_root: worktree_dir.path(),
        git_dir: git_dir.path(),
        patterns: &patterns,
        base_branch: Some("main"),
        base_commit: Some("abc1234"),
        policy: HydratePolicy {
            verify: true,
            snapshots: false,
            v2: false,
            strategy: flashwt_copy::StrategyPolicy::Default,
        },
    };

    let receipt = store.hydrate_workspace(req).expect("hydrate");
    assert_eq!(receipt.files_total, 3);
    assert!(!receipt.snapshot_hit);

    let heavy_root = worktree_dir.path().join("node_modules");
    assert_eq!(
        fs::read(heavy_root.join("index.js")).expect("read index.js"),
        c1
    );
    assert_eq!(
        fs::read(heavy_root.join("bin/cli")).expect("read bin/cli"),
        c2
    );
    assert_eq!(
        fs::read(heavy_root.join("nested/sub/package.json")).expect("read package.json"),
        c3
    );

    #[cfg(unix)]
    {
        let sym_target = fs::read_link(heavy_root.join("bin/symlink-cli")).expect("readlink");
        assert_eq!(sym_target, Path::new("cli"));

        let meta_cli = fs::metadata(heavy_root.join("bin/cli")).expect("meta cli");
        assert_eq!(meta_cli.permissions().mode() & 0o111, 0o111);
    }

    let id1 = store.put(c1).expect("put 1");
    let id2 = store.put(c2).expect("put 2");
    let id3 = store.put(c3).expect("put 3");

    let sidecar_path = git_dir.path().join("flashwt-hydrated.tsv");
    assert!(sidecar_path.exists(), "sidecar must exist");
    let sidecar_content = fs::read_to_string(&sidecar_path).expect("read sidecar");
    assert!(
        sidecar_content.contains(&format!("index.js\tblob\t{id1}")),
        "sidecar must list index.js"
    );
    assert!(
        sidecar_content.contains(&format!("bin/cli\tblob\t{id2}")),
        "sidecar must list bin/cli"
    );
    assert!(
        sidecar_content.contains(&format!("nested/sub/package.json\tblob\t{id3}")),
        "sidecar must list nested/sub/package.json"
    );

    let mirror_opt = store
        .read_worktree_mirror(worktree_dir.path(), git_dir.path())
        .expect("read mirror");
    let mirror = mirror_opt.expect("mirror should be published");
    assert_eq!(mirror.base_branch.as_deref(), Some("main"));
    assert_eq!(mirror.base_commit.as_deref(), Some("abc1234"));
    assert!(mirror.files.contains(&id1));
    assert!(mirror.files.contains(&id2));
    assert!(mirror.files.contains(&id3));
}

#[test]
fn fallback_materialization_with_custom_heavy_rel() {
    let store_dir = TempDir::new().expect("store dir");
    let mut store = DiskStore::open(store_dir.path()).expect("open store");

    let repo_dir = TempDir::new().expect("repo dir");
    let worktree_dir = TempDir::new().expect("worktree dir");
    let git_dir = TempDir::new().expect("git dir");

    let c1 = b"custom heavy payload\n";
    let custom = repo_dir.path().join("assets");
    fs::create_dir_all(&custom).expect("mkdir assets");
    fs::write(custom.join("file.txt"), c1).expect("write file");

    let patterns = vec!["assets/".to_string()];
    let req = WorkspaceHydrateReq {
        repo_root: repo_dir.path(),
        worktree_root: worktree_dir.path(),
        git_dir: git_dir.path(),
        patterns: &patterns,
        base_branch: None,
        base_commit: None,
        policy: HydratePolicy {
            verify: false,
            snapshots: false,
            v2: false,
            strategy: flashwt_copy::StrategyPolicy::ForceByteCopy,
        },
    };

    let receipt = store.hydrate_workspace(req).expect("hydrate");
    assert_eq!(receipt.files_total, 1);
    assert_eq!(receipt.strategy, "byte-copy");
    assert_eq!(
        fs::read(worktree_dir.path().join("assets/file.txt")).expect("read file"),
        c1
    );
}

#[test]
fn workspace_hydration_zero_matching_directories_publishes_empty_mirror_and_records_metadata() {
    let store_dir = TempDir::new().expect("store dir");
    let mut store = DiskStore::open(store_dir.path()).expect("open store");

    let repo_dir = TempDir::new().expect("repo dir");
    fs::create_dir_all(repo_dir.path().join("src")).expect("create src");
    fs::write(repo_dir.path().join("src/main.rs"), b"fn main() {}\n").expect("write main");

    let worktree_dir = TempDir::new().expect("worktree dir");
    let git_dir = TempDir::new().expect("git dir");

    let patterns = vec!["node_modules/".to_string(), "target/".to_string()];
    let req = WorkspaceHydrateReq {
        repo_root: repo_dir.path(),
        worktree_root: worktree_dir.path(),
        git_dir: git_dir.path(),
        patterns: &patterns,
        base_branch: Some("feature/zero-baseline"),
        base_commit: Some("a1b2c3d4e5f67890"),
        policy: HydratePolicy::default(),
    };

    let receipt = store.hydrate_workspace(req).expect("hydrate workspace");

    assert_eq!(receipt.files_total, 0);
    assert_eq!(receipt.files_copied, 0);
    assert_eq!(receipt.bytes_copied, 0);
    assert_eq!(receipt.bytes_shared, 0);
    assert_eq!(receipt.strategy, "none");
    assert!(!receipt.snapshot_hit);
    assert!(
        receipt
            .diagnostics
            .iter()
            .any(|d| d == ZERO_SAVINGS_NO_MATCHING_DIRS)
    );

    let mirror = store
        .read_worktree_mirror(worktree_dir.path(), git_dir.path())
        .expect("read mirror")
        .expect("mirror exists");

    assert_eq!(
        mirror.base_branch.as_deref(),
        Some("feature/zero-baseline")
    );
    assert_eq!(
        mirror.base_commit.as_deref(),
        Some("a1b2c3d4e5f67890")
    );
    assert!(mirror.files.is_empty());
    assert!(mirror.snapshots.is_empty());

    let mirror_file = flashwt_store::mirror_path(
        store_dir.path(),
        &fs::canonicalize(worktree_dir.path()).expect("canon worktree"),
        &fs::canonicalize(git_dir.path()).expect("canon gitdir"),
    );
    assert!(mirror_file.exists());
}

#[test]
fn workspace_hydration_empty_heavy_tree_publishes_empty_mirror_and_reports_zero_savings() {
    let store_dir = TempDir::new().expect("store dir");
    let mut store = DiskStore::open(store_dir.path()).expect("open store");

    let repo_dir = TempDir::new().expect("repo dir");
    fs::create_dir_all(repo_dir.path().join("node_modules")).expect("create node_modules");

    let worktree_dir = TempDir::new().expect("worktree dir");
    let git_dir = TempDir::new().expect("git dir");

    let patterns = vec!["node_modules/".to_string()];
    let req = WorkspaceHydrateReq {
        repo_root: repo_dir.path(),
        worktree_root: worktree_dir.path(),
        git_dir: git_dir.path(),
        patterns: &patterns,
        base_branch: None,
        base_commit: None,
        policy: HydratePolicy::default(),
    };

    let receipt = store.hydrate_workspace(req).expect("hydrate workspace");

    assert_eq!(receipt.files_total, 0);
    assert_eq!(receipt.files_copied, 0);
    assert_eq!(receipt.bytes_copied, 0);
    assert_eq!(receipt.bytes_shared, 0);
    assert_eq!(receipt.strategy, "none");
    assert!(!receipt.snapshot_hit);
    assert!(
        receipt
            .diagnostics
            .iter()
            .any(|d| d == ZERO_SAVINGS_NO_FILES_HYDRATED)
    );

    let mirror = store
        .read_worktree_mirror(worktree_dir.path(), git_dir.path())
        .expect("read mirror")
        .expect("mirror exists");

    assert_eq!(mirror.base_branch, None);
    assert_eq!(mirror.base_commit, None);
    assert!(mirror.files.is_empty());
    assert!(mirror.snapshots.is_empty());
}

#[test]
fn workspace_hydration_empty_mirror_preserves_blobs_during_garbage_collection() {
    let store_dir = TempDir::new().expect("store dir");
    let mut store = DiskStore::open(store_dir.path()).expect("open store");

    let blob_id = store.put(b"protected store payload").expect("put blob");
    store.add_ref(&blob_id).expect("add ref");
    assert_eq!(store.ref_count(&blob_id).expect("ref count"), 1);

    let repo_dir = TempDir::new().expect("repo dir");
    let worktree_dir = TempDir::new().expect("worktree dir");
    let git_dir = TempDir::new().expect("git dir");

    let patterns = vec!["node_modules/".to_string()];
    let req = WorkspaceHydrateReq {
        repo_root: repo_dir.path(),
        worktree_root: worktree_dir.path(),
        git_dir: git_dir.path(),
        patterns: &patterns,
        base_branch: Some("main"),
        base_commit: Some("commit01"),
        policy: HydratePolicy::default(),
    };

    let receipt = store.hydrate_workspace(req).expect("hydrate workspace");
    assert_eq!(receipt.files_total, 0);

    let mut reclaimer = StoreReclaimer::new(&mut store);
    let policy = SweepPolicy {
        grace: std::time::Duration::ZERO,
        snapshot_cap: 64,
        max_snapshot_bytes: None,
        dry_run: false,
    };

    let summary = reclaimer.sweep_objects(&policy).expect("sweep objects");
    assert_eq!(summary.reclaimed_blobs, 0);

    assert!(store.get(&blob_id).is_ok());
    assert_eq!(store.ref_count(&blob_id).expect("ref count"), 1);

    let mirror = store
        .read_worktree_mirror(worktree_dir.path(), git_dir.path())
        .expect("read mirror")
        .expect("mirror exists");
    assert!(mirror.files.is_empty());
}

#[test]
fn workspace_hydration_pinned_lockfile_snapshot_hit_materializes_and_records_blobs_in_mirror() {
    let store_dir = TempDir::new().expect("store dir");
    let mut store = DiskStore::open(store_dir.path()).expect("open store");

    let repo_dir = TempDir::new().expect("repo dir");
    let worktree_dir = TempDir::new().expect("worktree dir");
    let git_dir = TempDir::new().expect("git dir");

    let heavy = repo_dir.path().join("node_modules");
    fs::create_dir_all(heavy.join("pkg")).expect("mkdir");
    let content = b"console.log('hi');\n";
    fs::write(heavy.join("pkg/index.js"), content).expect("write");
    let blob = store.put(content).expect("put");

    let lockfile_bytes = b"{\n  \"name\": \"test-app\",\n  \"version\": \"1.0.0\",\n  \"lockfileVersion\": 3,\n  \"packages\": {}\n}\n";
    fs::write(repo_dir.path().join("package-lock.json"), lockfile_bytes).expect("write lockfile");
    let lock_hash = flashwt_store::hash_lockfile(lockfile_bytes);
    let entries = vec![flashwt_store::SnapshotEntry::file(
        "pkg/index.js",
        blob,
        0o644,
    )];
    let manifest = flashwt_store::Manifest::new_with_lockfile_and_size(
        entries,
        Some(lock_hash),
        content.len() as u64,
    )
    .expect("manifest");
    let hash = manifest.hash;
    store
        .publish_manifest(
            &manifest,
            flashwt_store::PublishOptions::default().lockfile_hash(Some(lock_hash)),
        )
        .expect("publish");

    let repo_key = repo_dir.path().to_string_lossy().into_owned();
    flashwt_store::record_snapshot_publish(
        store_dir.path(),
        &repo_key,
        "node_modules/",
        "node_modules",
        &hash,
    )
    .expect("seed ring");

    let patterns = vec!["node_modules/".to_string()];
    let req = WorkspaceHydrateReq {
        repo_root: repo_dir.path(),
        worktree_root: worktree_dir.path(),
        git_dir: git_dir.path(),
        patterns: &patterns,
        base_branch: Some("feature/snap-hit"),
        base_commit: Some("commit-snap-hit"),
        policy: HydratePolicy::default(),
    };

    #[cfg(target_os = "macos")]
    {
        let receipt = store.hydrate_workspace(req).expect("hydrate workspace");
        assert!(receipt.snapshot_hit);
        assert_eq!(receipt.files_total, 1);
        assert_eq!(receipt.files_copied, 0);
        assert_eq!(receipt.bytes_shared, content.len() as u64);
        assert_eq!(receipt.bytes_copied, 0);
        assert_eq!(receipt.strategy, "snapshot-hit");

        assert_eq!(
            fs::read(worktree_dir.path().join("node_modules/pkg/index.js")).expect("read"),
            content
        );

        let mirror = store
            .read_worktree_mirror(worktree_dir.path(), git_dir.path())
            .expect("read mirror")
            .expect("mirror exists");
        assert!(mirror.files.is_empty());
        assert!(mirror.snapshots.contains(&hash));
        assert_eq!(mirror.base_branch.as_deref(), Some("feature/snap-hit"));
        assert_eq!(mirror.base_commit.as_deref(), Some("commit-snap-hit"));

        let ledger =
            fs::read_to_string(git_dir.path().join("flashwt-hydrated.tsv")).expect("read ledger");
        assert_eq!(ledger, format!("-\tsnapshot\t{hash}\n"));
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (req, hash, blob);
    }
}

#[test]
fn workspace_hydration_unpinned_lockfile_falls_back_to_tree_ingest_and_materializes() {
    let store_dir = TempDir::new().expect("store dir");
    let mut store = DiskStore::open(store_dir.path()).expect("open store");

    let repo_dir = TempDir::new().expect("repo dir");
    let worktree_dir = TempDir::new().expect("worktree dir");
    let git_dir = TempDir::new().expect("git dir");

    let heavy = repo_dir.path().join("node_modules");
    fs::create_dir_all(heavy.join("pkg/bin")).expect("mkdir");

    let file_content = b"console.log('hello world');\n";
    let file_path = heavy.join("pkg/index.js");
    fs::write(&file_path, file_content).expect("write index.js");

    let bin_content = b"#!/bin/sh\necho 42\n";
    let bin_path = heavy.join("pkg/bin/run");
    fs::write(&bin_path, bin_content).expect("write run");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&bin_path, fs::Permissions::from_mode(0o755)).expect("chmod");
    }

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("../index.js", heavy.join("pkg/bin/link-index.js")).expect("symlink");
    }

    let unpinned_lockfile = b"{\n  \"dependencies\": {\n    \"local-dep\": \"file:../local-dep\"\n  }\n}\n";
    fs::write(repo_dir.path().join("package-lock.json"), unpinned_lockfile).expect("write unpinned");

    let patterns = vec!["node_modules/".to_string()];
    let req = WorkspaceHydrateReq {
        repo_root: repo_dir.path(),
        worktree_root: worktree_dir.path(),
        git_dir: git_dir.path(),
        patterns: &patterns,
        base_branch: Some("feature/fallback"),
        base_commit: Some("commit-fallback-01"),
        policy: HydratePolicy {
            verify: false,
            snapshots: false,
            v2: false,
            strategy: flashwt_copy::StrategyPolicy::Default,
        },
    };

    let receipt = store.hydrate_workspace(req).expect("hydrate workspace");
    assert!(!receipt.snapshot_hit);
    assert_eq!(receipt.files_total, 2);

    let dest_index = worktree_dir.path().join("node_modules/pkg/index.js");
    assert_eq!(fs::read(&dest_index).expect("read dest index"), file_content);

    let dest_bin = worktree_dir.path().join("node_modules/pkg/bin/run");
    assert_eq!(fs::read(&dest_bin).expect("read dest bin"), bin_content);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::symlink_metadata(&dest_bin).expect("stat dest bin").permissions().mode();
        assert_eq!(mode & 0o111, 0o111);

        let dest_link = worktree_dir.path().join("node_modules/pkg/bin/link-index.js");
        let link_target = fs::read_link(&dest_link).expect("read symlink");
        assert_eq!(link_target, Path::new("../index.js"));
    }

    let mirror = store
        .read_worktree_mirror(worktree_dir.path(), git_dir.path())
        .expect("read mirror")
        .expect("mirror exists");
    assert_eq!(mirror.files.len(), 2);
    assert_eq!(mirror.base_branch.as_deref(), Some("feature/fallback"));
    assert_eq!(mirror.base_commit.as_deref(), Some("commit-fallback-01"));

    let ledger = fs::read_to_string(git_dir.path().join("flashwt-hydrated.tsv")).expect("read ledger");
    assert!(ledger.contains("pkg/index.js\tblob\t"));
    assert!(ledger.contains("pkg/bin/run\tblob\t"));
}

#[test]
fn workspace_hydration_steps_down_to_byte_copies_when_forced_policy() {
    let store_dir = TempDir::new().expect("store dir");
    let mut store = DiskStore::open(store_dir.path()).expect("open store");

    let repo_dir = TempDir::new().expect("repo dir");
    let worktree_dir = TempDir::new().expect("worktree dir");
    let git_dir = TempDir::new().expect("git dir");

    let heavy = repo_dir.path().join("node_modules");
    fs::create_dir_all(heavy.join("sub")).expect("mkdir");
    let content = b"export const answer = 42;\n";
    fs::write(heavy.join("sub/lib.js"), content).expect("write lib.js");

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
            snapshots: false,
            v2: false,
            strategy: flashwt_copy::StrategyPolicy::ForceByteCopy,
        },
    };

    let receipt = store.hydrate_workspace(req).expect("hydrate workspace");
    assert_eq!(receipt.strategy, "byte-copy");
    assert_eq!(receipt.files_total, 1);
    assert_eq!(receipt.files_copied, 1);
    assert_eq!(receipt.bytes_copied, content.len() as u64);
    assert_eq!(receipt.bytes_shared, 0);

    assert_eq!(
        fs::read(worktree_dir.path().join("node_modules/sub/lib.js")).expect("read dest"),
        content
    );
}

#[test]
fn workspace_hydration_multiple_matching_directories_hydrates_all() {
    let store_dir = TempDir::new().expect("store dir");
    let mut store = DiskStore::open(store_dir.path()).expect("open store");

    let repo_dir = TempDir::new().expect("repo dir");
    let worktree_dir = TempDir::new().expect("worktree dir");
    let git_dir = TempDir::new().expect("git dir");

    let dir1 = repo_dir.path().join("node_modules");
    fs::create_dir_all(dir1.join("pkg1")).expect("mkdir dir1");
    let content1 = b"module.exports = 'one';\n";
    fs::write(dir1.join("pkg1/index.js"), content1).expect("write dir1");

    let dir2 = repo_dir.path().join("packages/app/node_modules");
    fs::create_dir_all(dir2.join("pkg2")).expect("mkdir dir2");
    let content2 = b"module.exports = 'two';\n";
    fs::write(dir2.join("pkg2/index.js"), content2).expect("write dir2");

    let patterns = vec!["node_modules/".to_string(), "packages/**/node_modules/".to_string()];
    let req = WorkspaceHydrateReq {
        repo_root: repo_dir.path(),
        worktree_root: worktree_dir.path(),
        git_dir: git_dir.path(),
        patterns: &patterns,
        base_branch: Some("multi"),
        base_commit: Some("commit-multi"),
        policy: HydratePolicy {
            verify: false,
            snapshots: false,
            v2: false,
            strategy: flashwt_copy::StrategyPolicy::ForceByteCopy,
        },
    };

    let receipt = store.hydrate_workspace(req).expect("hydrate workspace");
    assert_eq!(receipt.files_total, 2);
    assert_eq!(receipt.files_copied, 2);
    assert_eq!(receipt.bytes_copied, (content1.len() + content2.len()) as u64);

    assert_eq!(
        fs::read(worktree_dir.path().join("node_modules/pkg1/index.js")).expect("read dest1"),
        content1
    );
    assert_eq!(
        fs::read(worktree_dir.path().join("packages/app/node_modules/pkg2/index.js")).expect("read dest2"),
        content2
    );

    let mirror = store
        .read_worktree_mirror(worktree_dir.path(), git_dir.path())
        .expect("read mirror")
        .expect("mirror exists");
    assert_eq!(mirror.files.len(), 2);
}

#[test]
fn workspace_hydration_mixed_snapshot_hit_and_ingest_fallback_aggregates_into_one_mirror() {
    let store_dir = TempDir::new().expect("store dir");
    let mut store = DiskStore::open(store_dir.path()).expect("open store");

    let repo_dir = TempDir::new().expect("repo dir");
    let worktree_dir = TempDir::new().expect("worktree dir");
    let git_dir = TempDir::new().expect("git dir");

    let snap_heavy = repo_dir.path().join("node_modules");
    fs::create_dir_all(snap_heavy.join("pkg")).expect("mkdir snap");
    let snap_content = b"console.log('snap');\n";
    fs::write(snap_heavy.join("pkg/index.js"), snap_content).expect("write snap");
    let snap_blob = store.put(snap_content).expect("put");

    let lockfile_bytes = b"{\n  \"name\": \"app\",\n  \"version\": \"1.0.0\",\n  \"lockfileVersion\": 3,\n  \"packages\": {}\n}\n";
    fs::write(repo_dir.path().join("package-lock.json"), lockfile_bytes).expect("write lockfile");
    let lock_hash = flashwt_store::hash_lockfile(lockfile_bytes);
    let entries = vec![flashwt_store::SnapshotEntry::file(
        "pkg/index.js",
        snap_blob,
        0o644,
    )];
    let manifest = flashwt_store::Manifest::new_with_lockfile_and_size(
        entries,
        Some(lock_hash),
        snap_content.len() as u64,
    )
    .expect("manifest");
    let snap_hash = manifest.hash;
    store
        .publish_manifest(
            &manifest,
            flashwt_store::PublishOptions::default().lockfile_hash(Some(lock_hash)),
        )
        .expect("publish");

    let repo_key = repo_dir.path().to_string_lossy().into_owned();
    flashwt_store::record_snapshot_publish(
        store_dir.path(),
        &repo_key,
        "node_modules/",
        "node_modules",
        &snap_hash,
    )
    .expect("seed ring");

    let fallback_heavy = repo_dir.path().join("dist");
    fs::create_dir_all(fallback_heavy.join("bundle")).expect("mkdir fallback");
    let fallback_content = b"var bundle = true;\n";
    fs::write(fallback_heavy.join("bundle/app.js"), fallback_content).expect("write fallback");

    let patterns = vec!["node_modules/".to_string(), "dist/".to_string()];
    let req = WorkspaceHydrateReq {
        repo_root: repo_dir.path(),
        worktree_root: worktree_dir.path(),
        git_dir: git_dir.path(),
        patterns: &patterns,
        base_branch: Some("feature/mixed"),
        base_commit: Some("commit-mixed-01"),
        policy: HydratePolicy::default(),
    };

    #[cfg(target_os = "macos")]
    {
        let receipt = store.hydrate_workspace(req).expect("hydrate workspace");
        assert_eq!(receipt.files_total, 2);
        assert_eq!(receipt.strategy, "mixed");
        assert!(!receipt.snapshot_hit);

        assert_eq!(
            fs::read(worktree_dir.path().join("node_modules/pkg/index.js")).expect("read snap dest"),
            snap_content
        );
        assert_eq!(
            fs::read(worktree_dir.path().join("dist/bundle/app.js")).expect("read fallback dest"),
            fallback_content
        );

        let mirror = store
            .read_worktree_mirror(worktree_dir.path(), git_dir.path())
            .expect("read mirror")
            .expect("mirror exists");
        assert!(mirror.snapshots.contains(&snap_hash));
        assert_eq!(mirror.snapshots.len(), 2);
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (req, snap_hash, snap_blob);
    }
}

#[test]
fn workspace_hydration_verification_flag_bypasses_snapshot_fast_path() {
    let store_dir = TempDir::new().expect("store dir");
    let mut store = DiskStore::open(store_dir.path()).expect("open store");

    let repo_dir = TempDir::new().expect("repo dir");
    let worktree_dir = TempDir::new().expect("worktree dir");
    let git_dir = TempDir::new().expect("git dir");

    let heavy = repo_dir.path().join("node_modules");
    fs::create_dir_all(heavy.join("pkg")).expect("mkdir");
    let content = b"console.log('verify');\n";
    fs::write(heavy.join("pkg/index.js"), content).expect("write");
    let blob = store.put(content).expect("put");

    let lockfile_bytes = b"{\n  \"name\": \"app\",\n  \"version\": \"1.0.0\",\n  \"lockfileVersion\": 3,\n  \"packages\": {}\n}\n";
    fs::write(repo_dir.path().join("package-lock.json"), lockfile_bytes).expect("write lockfile");
    let lock_hash = flashwt_store::hash_lockfile(lockfile_bytes);
    let entries = vec![flashwt_store::SnapshotEntry::file(
        "pkg/index.js",
        blob,
        0o644,
    )];
    let manifest = flashwt_store::Manifest::new_with_lockfile_and_size(
        entries,
        Some(lock_hash),
        content.len() as u64,
    )
    .expect("manifest");
    let hash = manifest.hash;
    store
        .publish_manifest(
            &manifest,
            flashwt_store::PublishOptions::default().lockfile_hash(Some(lock_hash)),
        )
        .expect("publish");

    let repo_key = repo_dir.path().to_string_lossy().into_owned();
    flashwt_store::record_snapshot_publish(
        store_dir.path(),
        &repo_key,
        "node_modules/",
        "node_modules",
        &hash,
    )
    .expect("seed ring");

    let patterns = vec!["node_modules/".to_string()];
    let req = WorkspaceHydrateReq {
        repo_root: repo_dir.path(),
        worktree_root: worktree_dir.path(),
        git_dir: git_dir.path(),
        patterns: &patterns,
        base_branch: None,
        base_commit: None,
        policy: HydratePolicy {
            verify: true,
            snapshots: true,
            v2: true,
            strategy: flashwt_copy::StrategyPolicy::Default,
        },
    };

    let receipt = store.hydrate_workspace(req).expect("hydrate workspace");
    assert!(!receipt.snapshot_hit);
    assert_ne!(receipt.strategy, "snapshot-hit");
    assert_eq!(receipt.files_total, 1);
    assert_eq!(
        fs::read(worktree_dir.path().join("node_modules/pkg/index.js")).expect("read"),
        content
    );
}
