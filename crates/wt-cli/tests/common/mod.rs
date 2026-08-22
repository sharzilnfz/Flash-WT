//! Shared fixture builders for the end-to-end rig.
//!
//! Every test runs the real `wt` binary against a real temporary git
//! repository filled with a fake-heavy directory tree, then asserts
//! only on the CLI's output and files on disk.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A throwaway git repository plus its generated heavy directory.
pub struct Fixture {
    pub repo: PathBuf,
    _base: tempfile::TempDir,
}

impl Fixture {
    /// Create a temp git repo with one initial commit and `files`
    /// small files spread across nested directories under
    /// `<repo>/heavy/` — the shape hydration has to move cheaply.
    pub fn heavy_repo(files: usize) -> Fixture {
        let base = tempfile::tempdir().expect("tempdir");
        let repo = base.path().join("origin");
        fs::create_dir_all(&repo).expect("mkdir repo");
        git(&repo, &["init", "--quiet"]);
        git(&repo, &["config", "user.email", "test@example.com"]);
        git(&repo, &["config", "user.name", "Test"]);

        let heavy = repo.join("heavy");
        let content = |i: usize| format!("fake-heavy file {i} of {files}\n");
        for i in 0..files {
            let dir = heavy.join(format!("pkg{:02}/nested", i % 20));
            fs::create_dir_all(&dir).expect("mkdir heavy");
            fs::write(dir.join(format!("file-{i}.txt")), content(i)).expect("write file");
        }
        // Keep the heavy directory out of git: it stands in for
        // node_modules-style untracked bulk.
        fs::write(repo.join(".gitignore"), "heavy/\n").expect("gitignore");
        fs::write(repo.join("src.txt"), "tracked source\n").expect("src");

        git(&repo, &["add", "."]);
        git(&repo, &["commit", "--quiet", "-m", "init"]);

        Fixture { repo, _base: base }
    }

    /// Run `wt <args>` inside the fixture repository.
    #[allow(dead_code)] // each suite compiles this module; not all use both runners
    pub fn wt(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_wt"))
            .args(args)
            .current_dir(&self.repo)
            .output()
            .expect("run wt binary")
    }

    /// Run `wt <args>` with `WT_STORE` pointed at an isolated store,
    /// so tests never touch the developer's machine-wide store.
    #[allow(dead_code)] // each suite compiles this module; not all use both runners
    pub fn wt_with_store(&self, args: &[&str], store: &Path) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_wt"))
            .args(args)
            .env("WT_STORE", store)
            .current_dir(&self.repo)
            .output()
            .expect("run wt binary")
    }
}

/// Recursively collect every regular-file path under `dir`, sorted,
/// so tests can compare two trees by path list.
#[allow(dead_code)] // not every suite that compiles this module uses it
pub fn list_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in fs::read_dir(&d).expect("read_dir") {
            let p = entry.expect("dir entry").path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}
