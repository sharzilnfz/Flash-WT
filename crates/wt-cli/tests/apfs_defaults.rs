// Tests assert with unwrap/expect by design: a panic IS the failure
// signal under test, so the workspace restriction lints stay off here.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Tests for APFS snapshot defaults and opt-out mechanics (ticket 04).

#![cfg(target_os = "macos")]

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

        let heavy = repo.join("heavy");
        fs::create_dir_all(heavy.join("pkg/nested")).expect("mkdir");
        fs::write(heavy.join("pkg/nested/file.txt"), "hello snapshot\n").unwrap();
        fs::write(repo.join(".gitignore"), "heavy/\n").unwrap();
        fs::write(repo.join(".wtinclude"), "heavy/\n").unwrap();
        fs::write(repo.join("src.txt"), "main branch\n").unwrap();

        git(&repo, &["add", "."]);
        git(&repo, &["commit", "--quiet", "-m", "init"]);

        Self { repo, _base: base }
    }

    fn store_path(&self) -> PathBuf {
        self.repo.parent().unwrap().join("isolated-store")
    }

    fn wt(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_wt"))
            .args(args)
            .env("WT_STORE", self.store_path())
            .envs(env.iter().copied())
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
fn apfs_defaults_enable_snapshots_without_explicit_env() {
    let fx = TestFixture::new();
    // Run wt create without WT_SNAPSHOTS set
    let out = fx.wt(&["create", "snap-auto"], &[]);
    assert!(
        out.status.success(),
        "wt create failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("via snapshot"),
        "macOS APFS must default snapshots to enabled: {stdout}"
    );
}

#[test]
fn apfs_opt_out_disables_snapshots_via_env() {
    let fx = TestFixture::new();
    // Run wt create with WT_SNAPSHOTS=0 and WT_SNAPSHOTS_V2=0
    let out = fx.wt(
        &["create", "ladder-forced"],
        &[("WT_SNAPSHOTS", "0"), ("WT_SNAPSHOTS_V2", "0")],
    );
    assert!(out.status.success());

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("via snapshot"),
        "explicit WT_SNAPSHOTS=0 must opt out: {stdout}"
    );
}
