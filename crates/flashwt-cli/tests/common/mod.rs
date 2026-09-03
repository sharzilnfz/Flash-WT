#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub struct Fixture {
    pub repo: PathBuf,
    pub _base: tempfile::TempDir,
}

impl Fixture {
    pub fn init() -> Fixture {
        let base = tempfile::tempdir().expect("tempdir");
        let repo = base.path().join("origin");
        fs::create_dir_all(&repo).expect("mkdir repo");
        git(&repo, &["init", "--quiet"]);
        git(&repo, &["config", "user.email", "test@example.com"]);
        git(&repo, &["config", "user.name", "Test"]);

        fs::write(repo.join("src.txt"), "tracked source\n").expect("write src");
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "--quiet", "-m", "init"]);

        Fixture { repo, _base: base }
    }

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

        fs::write(repo.join(".gitignore"), "heavy/\n").expect("gitignore");
        fs::write(repo.join("src.txt"), "tracked source\n").expect("src");

        git(&repo, &["add", "."]);
        git(&repo, &["commit", "--quiet", "-m", "init"]);

        Fixture { repo, _base: base }
    }

    pub fn node_modules_repo() -> Fixture {
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
        fs::write(repo.join(".flashwtinclude"), "node_modules/\n").unwrap();
        fs::write(repo.join("src.txt"), "hello world\n").unwrap();

        git(&repo, &["add", "."]);
        git(&repo, &["commit", "--quiet", "-m", "init"]);

        Fixture { repo, _base: base }
    }

    pub fn rich_repo() -> Fixture {
        let base = tempfile::tempdir().expect("tempdir");
        let repo = base.path().join("origin");
        fs::create_dir_all(&repo).expect("mkdir repo");
        git(&repo, &["init", "--quiet"]);
        git(&repo, &["config", "user.email", "test@example.com"]);
        git(&repo, &["config", "user.name", "Test"]);

        let heavy = repo.join("heavy");
        fs::create_dir_all(heavy.join("pkg00/nested")).expect("mkdir");
        fs::create_dir_all(heavy.join("deep/a/b")).expect("mkdir");
        fs::write(heavy.join("pkg00/nested/file-0.txt"), "file zero\n").unwrap();
        fs::write(heavy.join("deep/a/b/c.txt"), "deep c\n").unwrap();
        let exec = heavy.join("exec.sh");
        fs::write(&exec, "#!/bin/sh\necho hi\n").unwrap();
        set_mode(&exec, 0o755);
        fs::create_dir(heavy.join("empty")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("../exec.sh", heavy.join("bin-link")).unwrap();

        fs::write(repo.join(".gitignore"), "heavy/\n").unwrap();
        fs::write(repo.join(".flashwtinclude"), "heavy/\n").unwrap();
        fs::write(repo.join("src.txt"), "tracked source\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "--quiet", "-m", "init"]);

        Fixture { repo, _base: base }
    }

    pub fn v2_repo(packages: usize, files_per_package: usize) -> Fixture {
        let base = tempfile::tempdir().expect("tempdir");
        let repo = base.path().join("origin");
        fs::create_dir_all(&repo).expect("mkdir repo");
        git(&repo, &["init", "--quiet"]);
        git(&repo, &["config", "user.email", "test@example.com"]);
        git(&repo, &["config", "user.name", "Test"]);

        let heavy = repo.join("heavy");
        for p in 0..packages {
            let dir = heavy.join(format!("pkg{p:02}"));
            fs::create_dir_all(&dir).expect("mkdir pkg");
            for i in 0..files_per_package {
                fs::write(
                    dir.join(format!("file-{i:03}.txt")),
                    format!("package {p} file {i}\n"),
                )
                .expect("write file");
            }
        }
        let exec = heavy.join("exec.sh");
        fs::write(&exec, "#!/bin/sh\necho hi\n").unwrap();
        set_mode(&exec, 0o755);
        fs::create_dir(heavy.join("empty")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("../exec.sh", heavy.join("bin-link")).unwrap();

        fs::write(repo.join(".gitignore"), "heavy/\n").unwrap();
        fs::write(repo.join(".flashwtinclude"), "heavy/\n").unwrap();
        fs::write(repo.join("src.txt"), "tracked source\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "--quiet", "-m", "init"]);

        Fixture { repo, _base: base }
    }

    pub fn lockfile_repo(lockfile_content: &str) -> Fixture {
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
        fs::write(repo.join(".flashwtinclude"), "node_modules/\n").unwrap();
        fs::write(repo.join("package.json"), "{\"name\":\"root\"}\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "--quiet", "-m", "init"]);

        Fixture { repo, _base: base }
    }

    pub fn store_path(&self) -> PathBuf {
        self.repo.parent().unwrap().join("isolated-store")
    }

    pub fn worktree_path(&self, name: &str) -> PathBuf {
        self.repo.parent().unwrap().join(name)
    }

    pub fn git(&self, args: &[&str]) {
        git(&self.repo, args);
    }

    pub fn git_out(&self, args: &[&str]) -> String {
        git_out(&self.repo, args)
    }

    pub fn flashwt_cli(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_flashwt"))
            .args(args)
            .env("FLASHWT_STORE", self.store_path())
            .env("FLASHWT_TEST_NO_SYNC", "1")
            .env("FLASHWT_NO_TINY_BYPASS", "1")
            .current_dir(&self.repo)
            .output()
            .expect("run flashwt binary")
    }

    pub fn flashwt(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_flashwt"))
            .args(args)
            .env("FLASHWT_STORE", self.store_path())
            .env("FLASHWT_TEST_NO_SYNC", "1")
            .env("FLASHWT_NO_TINY_BYPASS", "1")
            .current_dir(&self.repo)
            .output()
            .expect("run flashwt binary")
    }

    pub fn flashwt_hydrate(&self, args: &[&str]) -> Output {
        self.flashwt(args)
    }

    pub fn flashwt_with_store(&self, args: &[&str], store: &Path) -> Output {
        Command::new(env!("CARGO_BIN_EXE_flashwt"))
            .args(args)
            .env("FLASHWT_STORE", store)
            .env("FLASHWT_TEST_NO_SYNC", "1")
            .env("FLASHWT_NO_TINY_BYPASS", "1")
            .current_dir(&self.repo)
            .output()
            .expect("run flashwt binary")
    }

    pub fn flashwt_with_store_env(&self, args: &[&str], store: &Path, env: &[(&str, &str)]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_flashwt"))
            .args(args)
            .env("FLASHWT_STORE", store)
            .env("FLASHWT_TEST_NO_SYNC", "1")
            .env("FLASHWT_NO_TINY_BYPASS", "1")
            .envs(env.iter().copied())
            .current_dir(&self.repo)
            .output()
            .expect("run flashwt binary")
    }

    pub fn flashwt_env(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_flashwt"))
            .args(args)
            .env("FLASHWT_STORE", self.store_path())
            .env("FLASHWT_TEST_NO_SYNC", "1")
            .env("FLASHWT_NO_TINY_BYPASS", "1")
            .envs(env.iter().copied())
            .current_dir(&self.repo)
            .output()
            .expect("run flashwt binary")
    }
}

pub fn list_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if !dir.exists() {
        return out;
    }
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        if let Ok(entries) = fs::read_dir(&d) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    out.push(p);
                }
            }
        }
    }
    out.sort();
    out
}

pub fn store_footprint(store: &Path) -> u64 {
    let mut total = 0u64;
    for file in list_files(store) {
        if let Ok(meta) = fs::metadata(&file) {
            total += meta.len();
        }
    }
    total
}

pub fn assert_hydrated_files(worktree: &Path, expected: usize) {
    let count = list_files(worktree)
        .into_iter()
        .filter(|p| !p.components().any(|c| c.as_os_str() == ".git"))
        .count();
    assert_eq!(
        count,
        expected,
        "hydrated file count mismatch in {}",
        worktree.display()
    );
}

pub fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

pub fn git_out(dir: &Path, args: &[&str]) -> String {
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

#[cfg(unix)]
pub fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("set_mode");
}

#[cfg(not(unix))]
pub fn set_mode(_path: &Path, _mode: u32) {}
