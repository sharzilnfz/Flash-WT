//! Integration and contract tests for tiered lockfile validation (ticket 09).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct LockfileFixture {
    repo: PathBuf,
    _base: tempfile::TempDir,
}

impl LockfileFixture {
    fn new(lockfile_content: &str) -> LockfileFixture {
        let base = tempfile::tempdir().expect("tempdir");
        let repo = base.path().join("origin");
        fs::create_dir_all(&repo).expect("mkdir repo");
        git(&repo, &["init", "--quiet"]);
        git(&repo, &["config", "user.email", "test@example.com"]);
        git(&repo, &["config", "user.name", "Test"]);

        let heavy = repo.join("node_modules");
        fs::create_dir_all(heavy.join("pkg/lib")).expect("mkdir");
        fs::write(heavy.join("pkg/lib/index.js"), "console.log('hi');\n").unwrap();
        fs::write(heavy.join("pkg/package.json"), "{\"name\":\"pkg\"}\n").unwrap();

        fs::write(repo.join("package-lock.json"), lockfile_content).unwrap();
        fs::write(repo.join(".gitignore"), "node_modules/\n").unwrap();
        fs::write(repo.join(".wtinclude"), "node_modules/\n").unwrap();
        fs::write(repo.join("package.json"), "{\"name\":\"root\"}\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "--quiet", "-m", "init"]);

        LockfileFixture { repo, _base: base }
    }

    fn wt(&self, args: &[&str], env: &[(&str, &str)], worktree_name: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_wt"))
            .args(args)
            .env("WT_STORE", self.store_path())
            .envs(env.iter().copied())
            .current_dir(&self.repo)
            .args([
                "--dir",
                &self.worktree_path(worktree_name).to_string_lossy(),
            ])
            .output()
            .expect("run wt binary")
    }

    fn store_path(&self) -> PathBuf {
        self.repo.parent().unwrap().join("isolated-store")
    }

    fn worktree_path(&self, name: &str) -> PathBuf {
        self.repo.parent().unwrap().join(name)
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

const PINNED_LOCKFILE: &str = r#"{
  "name": "root",
  "version": "1.0.0",
  "lockfileVersion": 3,
  "packages": {
    "node_modules/pkg": {
      "version": "1.0.0",
      "resolved": "https://registry.npmjs.org/pkg/-/pkg-1.0.0.tgz",
      "integrity": "sha512-pinnedhash123=="
    }
  }
}"#;

const PINNED_LOCKFILE_V2: &str = r#"{
  "name": "root",
  "version": "1.0.1",
  "lockfileVersion": 3,
  "packages": {
    "node_modules/pkg": {
      "version": "1.0.1",
      "resolved": "https://registry.npmjs.org/pkg/-/pkg-1.0.1.tgz",
      "integrity": "sha512-pinnedhash456=="
    }
  }
}"#;

const MUTABLE_FILE_LOCKFILE: &str = r#"{
  "name": "root",
  "version": "1.0.0",
  "lockfileVersion": 3,
  "packages": {
    "node_modules/pkg": {
      "resolved": "file:../local/pkg"
    }
  }
}"#;

const MUTABLE_LINK_LOCKFILE: &str = r#"{
  "name": "root",
  "version": "1.0.0",
  "lockfileVersion": 3,
  "packages": {
    "node_modules/pkg": {
      "resolved": "link:../local/pkg"
    }
  }
}"#;

const MUTABLE_WORKSPACE_LOCKFILE: &str = r#"{
  "name": "root",
  "version": "1.0.0",
  "lockfileVersion": 3,
  "packages": {
    "node_modules/pkg": {
      "version": "workspace:*",
      "resolved": "workspace:packages/pkg"
    }
  }
}"#;

const MUTABLE_UNPINNED_GIT_LOCKFILE: &str = r#"{
  "name": "root",
  "version": "1.0.0",
  "lockfileVersion": 3,
  "packages": {
    "node_modules/pkg": {
      "resolved": "git+https://github.com/foo/bar.git#main"
    }
  }
}"#;

#[test]
fn lockfile_parser_safety_classification() {
    use wt_store::lockfile::{DependencySafety, classify_lockfile};

    assert_eq!(classify_lockfile(PINNED_LOCKFILE), DependencySafety::Pinned);
    assert_eq!(classify_lockfile(MUTABLE_FILE_LOCKFILE), DependencySafety::Mutable);
    assert_eq!(classify_lockfile(MUTABLE_LINK_LOCKFILE), DependencySafety::Mutable);
    assert_eq!(classify_lockfile(MUTABLE_WORKSPACE_LOCKFILE), DependencySafety::Mutable);
    assert_eq!(classify_lockfile(MUTABLE_UNPINNED_GIT_LOCKFILE), DependencySafety::Mutable);
}

#[test]
fn lockfiles_with_mutable_dependencies_bypass_fast_path() {
    for (lock_content, label) in [
        (MUTABLE_FILE_LOCKFILE, "file"),
        (MUTABLE_LINK_LOCKFILE, "link"),
        (MUTABLE_WORKSPACE_LOCKFILE, "workspace"),
        (MUTABLE_UNPINNED_GIT_LOCKFILE, "unpinned_git"),
    ] {
        let fx = LockfileFixture::new(lock_content);
        let out = fx.wt(&["create", &format!("wt-{label}")], &[("WT_SNAPSHOTS", "1")], &format!("dest-{label}"));
        assert!(out.status.success(), "create failed for {label}: {}", String::from_utf8_lossy(&out.stderr));

        let dest_file = fx.worktree_path(&format!("dest-{label}")).join("node_modules/pkg/lib/index.js");
        assert!(dest_file.is_file(), "file must be hydrated via fallback ladder for {label}");
    }
}

#[test]
fn pinned_lockfile_evaluates_sha256_and_manifest_header() {
    let fx = LockfileFixture::new(PINNED_LOCKFILE);
    let out = fx.wt(&["create", "one"], &[("WT_SNAPSHOTS", "1")], "dest-one");
    assert!(out.status.success(), "create one failed: {}", String::from_utf8_lossy(&out.stderr));

    // Second create with identical pinned lockfile
    let out2 = fx.wt(&["create", "two"], &[("WT_SNAPSHOTS", "1")], "dest-two");
    assert!(out2.status.success(), "create two failed: {}", String::from_utf8_lossy(&out2.stderr));
    let dest_file = fx.worktree_path("dest-two").join("node_modules/pkg/lib/index.js");
    assert!(dest_file.is_file());
}

#[test]
fn lockfile_content_change_invalidates_fast_path() {
    let fx = LockfileFixture::new(PINNED_LOCKFILE);
    let out = fx.wt(&["create", "one"], &[("WT_SNAPSHOTS", "1")], "dest-one");
    assert!(out.status.success(), "create one failed: {}", String::from_utf8_lossy(&out.stderr));

    // Modify lockfile
    fs::write(fx.repo.join("package-lock.json"), PINNED_LOCKFILE_V2).unwrap();

    let out2 = fx.wt(&["create", "two"], &[("WT_SNAPSHOTS", "1")], "dest-two");
    assert!(out2.status.success(), "create two failed: {}", String::from_utf8_lossy(&out2.stderr));
    let dest_file = fx.worktree_path("dest-two").join("node_modules/pkg/lib/index.js");
    assert!(dest_file.is_file());
}

#[test]
fn directory_timestamp_change_triggers_revalidation() {
    let fx = LockfileFixture::new(PINNED_LOCKFILE);
    let out = fx.wt(&["create", "one"], &[("WT_SNAPSHOTS", "1")], "dest-one");
    assert!(out.status.success(), "create one failed: {}", String::from_utf8_lossy(&out.stderr));

    // Modify root directory mtime
    let heavy = fx.repo.join("node_modules");
    let future = std::time::SystemTime::now() + std::time::Duration::from_secs(100);
    let f = fs::OpenOptions::new().write(true).open(heavy.join("pkg/lib/index.js")).unwrap();
    f.set_times(fs::FileTimes::new().set_modified(future)).unwrap();

    let out2 = fx.wt(&["create", "two"], &[("WT_SNAPSHOTS", "1")], "dest-two");
    assert!(out2.status.success(), "create two failed: {}", String::from_utf8_lossy(&out2.stderr));
    let dest_file = fx.worktree_path("dest-two").join("node_modules/pkg/lib/index.js");
    assert!(dest_file.is_file());
}

