// Tests assert with unwrap/expect by design: a panic IS the failure
// signal under test, so the workspace restriction lints stay off here.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Tests for human-centric primary verbs: `wt new` and `wt isolate` (ticket 03).

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

#[test]
fn wt_new_creates_worktree_with_structured_receipt() {
    let fx = TestFixture::new();
    let out = fx.wt(&["new", "feat-alpha"]);
    assert!(
        out.status.success(),
        "wt new failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("✓ Created worktree"),
        "expected ✓ Created worktree in {stdout}"
    );
    assert!(
        stdout.contains("feat-alpha"),
        "expected branch name in {stdout}"
    );
    assert!(
        stdout.contains("Next: cd"),
        "expected actionable next step hint in {stdout}"
    );

    let wt_path = fx.repo.parent().unwrap().join("origin-feat-alpha");
    assert!(wt_path.exists(), "worktree directory must exist");
    assert!(
        wt_path.join("node_modules/pkg/index.js").exists(),
        "hydrated file must exist"
    );
}

#[test]
fn wt_new_json_emits_valid_ndjson_envelope() {
    let fx = TestFixture::new();
    let out = fx.wt(&["new", "feat-json", "--json"]);
    assert!(out.status.success(), "wt new --json failed");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let val: serde_json::Value = serde_json::from_str(stdout.trim()).expect("valid JSON envelope");
    assert_eq!(val["command"], "create");
    assert_eq!(val["status"], "ok");
    assert_eq!(val["data"]["branch"], "feat-json");
    assert_eq!(val["data"]["files_hydrated"], 1);
}

#[test]
fn wt_isolate_aliases_scratch_worktree_execution() {
    let fx = TestFixture::new();
    let out = fx.wt(&["isolate", "--run", "echo isolate-ok", "--json"]);
    assert!(out.status.success(), "wt isolate failed");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let val: serde_json::Value = serde_json::from_str(stdout.trim()).expect("valid JSON envelope");
    assert_eq!(val["command"], "scratch");
    assert_eq!(val["status"], "ok");
    assert_eq!(val["data"]["command"], "echo isolate-ok");
    assert_eq!(val["data"]["cleaned_up"], true);
}
