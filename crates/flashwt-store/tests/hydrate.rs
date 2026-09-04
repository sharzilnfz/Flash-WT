#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use flashwt_store::{
    DiskStore, HydrateDest, HydrateOutcome, HydratePinned, HydratePolicy, HydrateReq, HydrateSrc,
    HydrateTree, Ingested, StoreReclaimer, SweepPolicy, WorkspaceHydrateReq,
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

    let id1 = store.put(c1).expect("put 1");
    let id2 = store.put(c2).expect("put 2");
    let id3 = store.put(c3).expect("put 3");

    let dirs = vec!["bin".to_string(), "nested/sub".to_string()];
    let mut dir_modes = BTreeMap::new();
    dir_modes.insert("bin".to_string(), 0o755);
    dir_modes.insert("nested/sub".to_string(), 0o755);

    let mut files = BTreeMap::new();
    files.insert("index.js".to_string(), id1);
    files.insert("bin/cli".to_string(), id2);
    files.insert("nested/sub/package.json".to_string(), id3);

    let mut file_sizes = BTreeMap::new();
    file_sizes.insert("index.js".to_string(), c1.len() as u64);
    file_sizes.insert("bin/cli".to_string(), c2.len() as u64);
    file_sizes.insert("nested/sub/package.json".to_string(), c3.len() as u64);

    let mut symlinks = BTreeMap::new();
    symlinks.insert("bin/symlink-cli".to_string(), "cli".to_string());

    let mut modes = BTreeMap::new();
    modes.insert("index.js".to_string(), 0o644);
    modes.insert("bin/cli".to_string(), 0o755);
    modes.insert("nested/sub/package.json".to_string(), 0o644);

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
            pattern: "node_modules",
            src_root: repo_dir.path(),
            heavy_rel: "node_modules",
            lockfile_hash: None,
        }),
        dest: HydrateDest {
            worktree_root: worktree_dir.path(),
            git_dir: git_dir.path(),
            base_branch: Some("main"),
            base_commit: Some("abc1234"),
        },
        policy: HydratePolicy {
            verify: true,
            snapshots: false,
            v2: false,
            strategy: flashwt_copy::StrategyPolicy::Default,
        },
    };

    let receipt = match store.hydrate(req).expect("hydrate") {
        HydrateOutcome::Hydrated(r) => r,
        HydrateOutcome::NeedIngest { .. } | HydrateOutcome::Failed(_) => {
            panic!("tree hydration always resolves")
        }
    };

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
    }

    #[cfg(unix)]
    {
        let meta_cli = fs::metadata(heavy_root.join("bin/cli")).expect("meta cli");
        assert_eq!(meta_cli.permissions().mode() & 0o111, 0o111);
    }

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
fn fallback_materialization_with_empty_heavy_rel() {
    let store_dir = TempDir::new().expect("store dir");
    let mut store = DiskStore::open(store_dir.path()).expect("open store");

    let repo_dir = TempDir::new().expect("repo dir");
    let worktree_dir = TempDir::new().expect("worktree dir");
    let git_dir = TempDir::new().expect("git dir");

    let c1 = b"top-level file\n";
    let id1 = store.put(c1).expect("put");

    let dirs = vec![];
    let dir_modes = BTreeMap::new();
    let mut files = BTreeMap::new();
    files.insert("file.txt".to_string(), id1);
    let mut file_sizes = BTreeMap::new();
    file_sizes.insert("file.txt".to_string(), c1.len() as u64);
    let symlinks = BTreeMap::new();
    let mut modes = BTreeMap::new();
    modes.insert("file.txt".to_string(), 0o644);

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
            pattern: "",
            src_root: repo_dir.path(),
            heavy_rel: "",
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
            snapshots: false,
            v2: false,
            strategy: flashwt_copy::StrategyPolicy::ForceByteCopy,
        },
    };

    let receipt = match store.hydrate(req).expect("hydrate") {
        HydrateOutcome::Hydrated(r) => r,
        HydrateOutcome::NeedIngest { .. } | HydrateOutcome::Failed(_) => {
            panic!("tree hydration always resolves")
        }
    };
    assert_eq!(receipt.files_total, 1);
    assert_eq!(receipt.strategy, "byte-copy");
    assert_eq!(
        fs::read(worktree_dir.path().join("file.txt")).expect("read file"),
        c1
    );
}

#[test]
fn pinned_lockfile_hit_hydrates_without_ingest() {
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

    let lock_hash = flashwt_store::hash_lockfile(b"pinned-lockfile-bytes");
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
        let receipt = match outcome {
            HydrateOutcome::Hydrated(r) => r,
            _ => panic!("expected a pinned lockfile hit, got {outcome:?}"),
        };
        assert!(receipt.snapshot_hit);
        assert_eq!(receipt.files_total, 1);
        let ledger =
            fs::read_to_string(git_dir.path().join("flashwt-hydrated.tsv")).expect("read ledger");
        assert_eq!(ledger, format!("-\tsnapshot\t{hash}\n"));
        let mirror = store
            .read_worktree_mirror(worktree_dir.path(), git_dir.path())
            .expect("read mirror")
            .expect("mirror published");
        assert!(mirror.snapshots.contains(&hash));
        assert_eq!(
            fs::read(worktree_dir.path().join("node_modules/pkg/index.js")).expect("read file"),
            content
        );
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = hash;
        assert!(
            matches!(outcome, HydrateOutcome::NeedIngest { .. }),
            "fast path is macOS-only"
        );
    }
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
