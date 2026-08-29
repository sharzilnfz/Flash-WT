// Tests assert with unwrap/expect by design: a panic IS the failure
// signal under test, so the workspace restriction lints stay off here.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Tests for `wt clean` unified removal and batch cleanup (ticket 03 & ticket 05).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct TestFixture {
    repo: PathBuf,
    _base: tempfile::TempDir,
}

impl TestFixture {
    fn new() -> Self {
        let base = tempfile::tempdir().expect("tempdir");
        let repo = base.path().join("origin");
        fs::create_dir_all(&repo).expect("mkdir repo");

        git(&repo, &["init", "--quiet"]);
        git(&repo, &["config", "user.email", "test@example.com"]);
        git(&repo, &["config", "user.name", "Test"]);

        let nm = repo.join("node_modules");
        fs::create_dir_all(nm.join("pkg")).expect("mkdir pkg");
        fs::write(nm.join("pkg/index.js"), "module.exports = 42;\n").unwrap();
        fs::write(repo.join(".gitignore"), "node_modules/\n").unwrap();
        fs::write(repo.join(".wtinclude"), "node_modules/\n").unwrap();
        fs::write(repo.join("src.txt"), "hello world\n").unwrap();

        git(&repo, &["add", "."]);
        git(&repo, &["commit", "--quiet", "-m", "init"]);

        Self { repo, _base: base }
    }

    fn store_path(&self) -> PathBuf {
        self.repo.parent().unwrap().join("isolated-store")
    }

    fn wt(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_wt"))
            .args(args)
            .env("WT_STORE", self.store_path())
            .current_dir(&self.repo)
            .output()
            .expect("run wt binary")
    }
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

fn git_out(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn wt_clean_unregisters_raw_git_worktree_and_spares_sibling() {
    let fx = TestFixture::new();

    // Synthetic multi-worktree fixture: linked worktrees created with
    // raw git rather than `wt new`, so nothing exists in the store yet.
    let first = fx.repo.parent().unwrap().join("origin-raw-a");
    let second = fx.repo.parent().unwrap().join("origin-raw-b");
    git(&fx.repo, &["branch", "raw-a"]);
    git(&fx.repo, &["branch", "raw-b"]);
    git(
        &fx.repo,
        &[
            "worktree",
            "add",
            "--quiet",
            first.to_str().unwrap(),
            "raw-a",
        ],
    );
    git(
        &fx.repo,
        &[
            "worktree",
            "add",
            "--quiet",
            second.to_str().unwrap(),
            "raw-b",
        ],
    );
    let canonical_first = std::fs::canonicalize(&first).unwrap();
    assert!(first.exists() && second.exists());

    let out = fx.wt(&["clean", "raw-a", "--json"]);
    assert!(
        out.status.success(),
        "wt clean failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let val: serde_json::Value = serde_json::from_str(stdout.trim()).expect("valid JSON envelope");
    assert_eq!(
        val["data"]["removed_worktrees"][0],
        canonical_first.to_string_lossy().as_ref()
    );
    assert!(
        !first.exists(),
        "cleaned worktree directory must be removed"
    );

    let registered = git_out(&fx.repo, &["worktree", "list", "--porcelain"]);
    assert!(
        !registered.contains("origin-raw-a"),
        "git must forget the removed worktree: {registered}"
    );
    assert!(
        registered.contains("origin-raw-b"),
        "untouched sibling must stay registered: {registered}"
    );
    assert!(second.exists(), "untouched sibling directory must survive");
}

#[test]
fn wt_clean_single_worktree_removes_and_sweeps_with_receipt() {
    let fx = TestFixture::new();
    let out = fx.wt(&["new", "feature-clean"]);
    assert!(out.status.success());

    let wt_path = fx.repo.parent().unwrap().join("origin-feature-clean");
    assert!(wt_path.exists());

    let out_clean = fx.wt(&["clean", "feature-clean"]);
    assert!(
        out_clean.status.success(),
        "wt clean failed: {}",
        String::from_utf8_lossy(&out_clean.stderr)
    );

    let stdout = String::from_utf8_lossy(&out_clean.stdout);
    assert!(
        stdout.contains("✓ Removed worktree"),
        "receipt must contain ✓ Removed worktree: {stdout}"
    );
    assert!(
        stdout.contains("feature-clean"),
        "receipt must contain branch name: {stdout}"
    );
    assert!(!wt_path.exists(), "worktree directory must be removed");
}

#[test]
fn wt_clean_single_json_emits_valid_envelope() {
    let fx = TestFixture::new();
    let out = fx.wt(&["new", "feature-clean-json"]);
    assert!(out.status.success());

    let out_clean = fx.wt(&["clean", "feature-clean-json", "--json"]);
    assert!(out_clean.status.success());

    let stdout = String::from_utf8_lossy(&out_clean.stdout);
    let val: serde_json::Value = serde_json::from_str(stdout.trim()).expect("valid JSON envelope");
    assert_eq!(val["command"], "clean");
    assert_eq!(val["status"], "ok");
    assert_eq!(val["data"]["branches_removed"][0], "feature-clean-json");
    assert_eq!(val["data"]["mirrors_removed"], 1);
}

#[test]
fn wt_clean_batch_removes_merged_branches_non_interactively() {
    let fx = TestFixture::new();

    // Create two worktrees: one merged into HEAD, one unmerged
    let out1 = fx.wt(&["new", "merged-feat"]);
    assert!(out1.status.success());

    let out2 = fx.wt(&["new", "unmerged-feat"]);
    assert!(out2.status.success());

    let unmerged_wt = fx.repo.parent().unwrap().join("origin-unmerged-feat");
    fs::write(unmerged_wt.join("src.txt"), "divergent work").unwrap();
    git(&unmerged_wt, &["add", "."]);
    git(&unmerged_wt, &["commit", "--quiet", "-m", "unmerged work"]);

    // Running wt clean without arguments should clean merged-feat and spare unmerged-feat
    let out_batch = fx.wt(&["clean", "--json"]);
    assert!(out_batch.status.success());

    let stdout = String::from_utf8_lossy(&out_batch.stdout);
    let val: serde_json::Value = serde_json::from_str(stdout.trim()).expect("valid JSON envelope");
    assert_eq!(val["command"], "clean");

    let removed = val["data"]["branches_removed"].as_array().unwrap();
    let removed_names: Vec<&str> = removed.iter().map(|v| v.as_str().unwrap()).collect();
    assert!(
        removed_names.contains(&"merged-feat"),
        "merged-feat must be cleaned"
    );
    assert!(
        !removed_names.contains(&"unmerged-feat"),
        "unmerged-feat must not be cleaned without --force/--all"
    );
}

#[test]
fn wt_clean_all_removes_all_worktrees() {
    let fx = TestFixture::new();

    let out1 = fx.wt(&["new", "w1"]);
    assert!(out1.status.success());
    let out2 = fx.wt(&["new", "w2"]);
    assert!(out2.status.success());

    let out_all = fx.wt(&["clean", "--all", "--json"]);
    assert!(out_all.status.success());

    let stdout = String::from_utf8_lossy(&out_all.stdout);
    let val: serde_json::Value = serde_json::from_str(stdout.trim()).expect("valid JSON envelope");
    let removed = val["data"]["branches_removed"].as_array().unwrap();
    assert_eq!(removed.len(), 2);
}
