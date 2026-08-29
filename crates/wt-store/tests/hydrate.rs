//! Tests for deep hydration engine in wt-store.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use tempfile::TempDir;
use wt_store::{DiskStore, HydrationRequest, Store};

#[test]
fn fallback_materialization_creates_files_dirs_and_metadata() {
    let store_dir = TempDir::new().expect("store dir");
    let mut store = DiskStore::open(store_dir.path()).expect("open store");

    let repo_dir = TempDir::new().expect("repo dir");
    let wt_dir = TempDir::new().expect("wt dir");
    let git_dir = TempDir::new().expect("git dir");

    // Ingest some blobs
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

    let req = HydrationRequest {
        worktree_root: wt_dir.path(),
        git_dir: git_dir.path(),
        dirs: &dirs,
        dir_modes: &dir_modes,
        files: &files,
        file_sizes: &file_sizes,
        symlinks: &symlinks,
        modes: &modes,
        repo_root: repo_dir.path(),
        pattern: "node_modules",
        src_root: repo_dir.path(),
        heavy_rel: "node_modules",
        lockfile_hash: None,
        base_branch: Some("main"),
        base_commit: Some("abc1234"),
        verify: true,
        snapshots_enabled: false,
        v2_enabled: false,
        strategy_policy: wt_copy::StrategyPolicy::Default,
    };

    let receipt = store.hydrate(req).expect("hydrate");

    assert_eq!(receipt.files_total, 3);
    assert!(!receipt.snapshot_hit);

    let heavy_root = wt_dir.path().join("node_modules");

    // Verify files created with exact contents
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

    // Verify symlink
    #[cfg(unix)]
    {
        let sym_target = fs::read_link(heavy_root.join("bin/symlink-cli")).expect("readlink");
        assert_eq!(sym_target, Path::new("cli"));
    }

    // Verify permissions
    #[cfg(unix)]
    {
        let meta_cli = fs::metadata(heavy_root.join("bin/cli")).expect("meta cli");
        assert_eq!(meta_cli.permissions().mode() & 0o111, 0o111);
    }

    // Verify sidecar wt-hydrated.tsv
    let sidecar_path = git_dir.path().join("wt-hydrated.tsv");
    assert!(sidecar_path.exists(), "sidecar must exist");
    let sidecar_content = fs::read_to_string(&sidecar_path).expect("read sidecar");
    assert!(
        sidecar_content.contains(&format!("index.js\t{id1}")),
        "sidecar must list index.js"
    );
    assert!(
        sidecar_content.contains(&format!("bin/cli\t{id2}")),
        "sidecar must list bin/cli"
    );
    assert!(
        sidecar_content.contains(&format!("nested/sub/package.json\t{id3}")),
        "sidecar must list nested/sub/package.json"
    );

    // Verify published mirror
    let mirror_opt = store
        .read_worktree_mirror(wt_dir.path(), git_dir.path())
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
    let wt_dir = TempDir::new().expect("wt dir");
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

    let req = HydrationRequest {
        worktree_root: wt_dir.path(),
        git_dir: git_dir.path(),
        dirs: &dirs,
        dir_modes: &dir_modes,
        files: &files,
        file_sizes: &file_sizes,
        symlinks: &symlinks,
        modes: &modes,
        repo_root: repo_dir.path(),
        pattern: "",
        src_root: repo_dir.path(),
        heavy_rel: "",
        lockfile_hash: None,
        base_branch: None,
        base_commit: None,
        verify: false,
        snapshots_enabled: false,
        v2_enabled: false,
        strategy_policy: wt_copy::StrategyPolicy::ForceByteCopy,
    };

    let receipt = store.hydrate(req).expect("hydrate");
    assert_eq!(receipt.files_total, 1);
    assert_eq!(receipt.strategy, "byte-copy");
    assert_eq!(
        fs::read(wt_dir.path().join("file.txt")).expect("read file"),
        c1
    );
}
