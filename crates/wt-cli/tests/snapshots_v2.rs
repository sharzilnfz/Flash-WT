//! v2 diff-based incremental snapshot rebuilds, end to end.
//!
//! Every test runs the real `wt` binary against a real temporary git
//! repository with `WT_SNAPSHOTS=1 AND WT_SNAPSHOTS_V2=1` and an
//! isolated `WT_STORE`. Coverage:
//!
//! - content bump: w2 rebuilds INCREMENTALLY (snapshot-mode=v2,
//!   snapshot-v2-linked well below the file count) and lands a tree
//!   byte-identical to the source
//! - corrupted old-snapshot manifest: v2 selection rejects it and the
//!   create succeeds via a plain full build (snapshot-mode=build)
//! - WT_VERIFY=1 with both gates: exact tree regardless of which mode
//!   served it

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A throwaway git repo whose `heavy/` tree is big enough for the
/// incremental heuristic to bite: 5 generated package dirs (~250
/// files), a symlink, an executable, and an explicit empty dir.
struct V2Fixture {
    repo: PathBuf,
    _base: tempfile::TempDir,
}

const PACKAGES: usize = 5;
const FILES_PER_PACKAGE: usize = 50;

impl V2Fixture {
    fn new() -> V2Fixture {
        let base = tempfile::tempdir().expect("tempdir");
        let repo = base.path().join("origin");
        fs::create_dir_all(&repo).expect("mkdir repo");
        git(&repo, &["init", "--quiet"]);
        git(&repo, &["config", "user.email", "test@example.com"]);
        git(&repo, &["config", "user.name", "Test"]);

        let heavy = repo.join("heavy");
        for p in 0..PACKAGES {
            let dir = heavy.join(format!("pkg{p:02}"));
            fs::create_dir_all(&dir).expect("mkdir pkg");
            for i in 0..FILES_PER_PACKAGE {
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
        fs::write(repo.join(".wtinclude"), "heavy/\n").unwrap();
        fs::write(repo.join("src.txt"), "tracked source\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "--quiet", "-m", "init"]);

        V2Fixture { repo, _base: base }
    }

    fn heavy(&self) -> PathBuf {
        self.repo.join("heavy")
    }

    /// Run `wt` with full environment control.
    fn wt(&self, args: &[&str], env: &[(&str, &str)], worktree_name: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_wt"))
            .args(args)
            .env("WT_STORE", self.store_path())
            .env_remove("WT_HARDLINK")
            .env_remove("WT_NO_HARDLINK")
            .env_remove("WT_GC_GRACE")
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

fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

fn assert_created(out: &Output) {
    assert!(
        out.status.success(),
        "create failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The whole hydrated tree matches the source: every regular file's
/// bytes, symlink targets, exec bits, and the explicit empty dir.
fn assert_tree_matches_source(source: &Path, hydrated_heavy: &Path) {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).expect("read_dir") {
            let p = entry.expect("dir entry").path();
            if p.symlink_metadata().expect("lstat").is_dir() {
                walk(&p, out);
            } else {
                out.push(p);
            }
        }
    }
    let mut src_files = Vec::new();
    walk(source, &mut src_files);
    src_files.sort();
    assert!(!src_files.is_empty());

    for src_path in &src_files {
        let rel = src_path.strip_prefix(source).unwrap();
        let dest_path = hydrated_heavy.join(rel);
        let src_meta = src_path.symlink_metadata().unwrap();
        if src_meta.file_type().is_symlink() {
            let md = dest_path.symlink_metadata().expect("symlink survived");
            assert!(
                md.file_type().is_symlink(),
                "{} must stay a symlink",
                rel.display()
            );
            assert_eq!(
                fs::read_link(&dest_path).unwrap(),
                fs::read_link(src_path).unwrap(),
                "symlink target must match"
            );
        } else {
            let got = fs::read(&dest_path)
                .unwrap_or_else(|e| panic!("cannot read hydrated {}: {e}", rel.display()));
            let want = fs::read(src_path)
                .unwrap_or_else(|e| panic!("cannot read source {}: {e}", rel.display()));
            assert_eq!(got, want, "content mismatch at {}", rel.display());
            if src_meta.mode() & 0o111 != 0 {
                let md = dest_path.metadata().unwrap();
                assert_eq!(
                    md.mode() & 0o111,
                    0o111,
                    "exec bits must survive at {}",
                    rel.display()
                );
            }
        }
    }
    assert!(
        hydrated_heavy.join("empty").is_dir(),
        "empty directory vanished during hydration"
    );
}

/// Count regular files under `dir` (for the linked < total bound).
fn count_files(dir: &Path) -> usize {
    fn walk(dir: &Path, out: &mut usize) {
        for entry in fs::read_dir(dir).expect("read_dir") {
            let p = entry.expect("dir entry").path();
            if p.is_dir() && !p.symlink_metadata().unwrap().is_symlink() {
                walk(&p, out);
            } else if !p.symlink_metadata().unwrap().is_symlink() {
                *out += 1;
            }
        }
    }
    let mut n = 0;
    walk(dir, &mut n);
    n
}

/// Mutate ONE existing file and ADD one new package dir with a few
/// files: the minimal bump the incremental path exists for.
fn bump_source(heavy: &Path) {
    fs::write(
        heavy.join("pkg02/file-010.txt"),
        "package 2 file 10 EDITED\n",
    )
    .unwrap();
    let fresh = heavy.join("pkg05");
    fs::create_dir_all(&fresh).unwrap();
    for i in 0..4 {
        fs::write(
            fresh.join(format!("new-{i}.txt")),
            format!("brand new {i}\n"),
        )
        .unwrap();
    }
}

const BOTH_GATES: &[(&str, &str)] = &[("WT_SNAPSHOTS", "1"), ("WT_SNAPSHOTS_V2", "1")];

#[test]
fn bump_rebuilds_incrementally_and_matches_source_exactly() {
    let fx = V2Fixture::new();

    // Seed generation 1 through the gated path.
    assert_created(&fx.wt(&["create", "one"], BOTH_GATES, "origin-one"));

    // Generation 2: one edited file + one added package dir.
    bump_source(&fx.heavy());
    let out = fx.wt(
        &["create", "two"],
        &[BOTH_GATES, &[("WT_TIMING", "1")]].concat(),
        "origin-two",
    );
    assert_created(&out);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("wt-stage snapshot-mode=v2"),
        "the bump must rebuild incrementally:\n{stderr}"
    );

    let linked = stderr
        .lines()
        .find_map(|l| l.strip_prefix("wt-stage snapshot-v2-linked="))
        .map(|v| v.trim().parse::<usize>().expect("integer linked count"));
    let linked = linked.expect("snapshot-v2-linked line must be present");

    let total_files = count_files(&fx.heavy());
    assert!(
        linked < total_files,
        "incremental rebuild hardlinked {linked} of {total_files} files — \
         that is not incremental"
    );

    // Exact fidelity regardless of how it was served.
    assert_tree_matches_source(&fx.heavy(), &fx.worktree_path("origin-two").join("heavy"));
}

#[test]
fn corrupt_old_manifest_falls_back_to_full_build() {
    let fx = V2Fixture::new();

    assert_created(&fx.wt(&["create", "one"], BOTH_GATES, "origin-one"));

    // Corrupt (truncate into garbage) the only published snapshot's
    // manifest: v2 selection must reject it as unusable.
    let snapshots_dir = fx.store_path().join("snapshots");
    let hash = fs::read_dir(&snapshots_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .find(|n| *n != "tmp" && *n != "index.tsv")
        .expect("published snapshot");
    fs::write(snapshots_dir.join(&hash).join("manifest.tsv"), "garbage\n").unwrap();

    // The bump changes content anyway, so the rebuilt snapshot lands
    // on a FRESH address rather than colliding with the debris.
    bump_source(&fx.heavy());
    let out = fx.wt(
        &["create", "two"],
        &[BOTH_GATES, &[("WT_TIMING", "1")]].concat(),
        "origin-two",
    );
    assert_created(&out);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("wt-stage snapshot-mode=build"),
        "with no usable old snapshot the create must take the plain \
         full build:\n{stderr}"
    );
    assert_tree_matches_source(&fx.heavy(), &fx.worktree_path("origin-two").join("heavy"));
}

#[test]
fn paranoid_verify_with_v2_gates_still_lands_an_exact_tree() {
    let fx = V2Fixture::new();

    assert_created(&fx.wt(&["create", "one"], BOTH_GATES, "origin-one"));

    // Bump content so the second create cannot just hit; run it
    // paranoid WITH both gates. Whatever mode serves it, the landed
    // tree must be exact.
    bump_source(&fx.heavy());
    let out = fx.wt(
        &["create", "two"],
        &[
            ("WT_SNAPSHOTS", "1"),
            ("WT_SNAPSHOTS_V2", "1"),
            ("WT_VERIFY", "1"),
            ("WT_TIMING", "1"),
        ],
        "origin-two",
    );
    assert_created(&out);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("wt-stage snapshot-mode="),
        "mode line must be emitted whenever the snapshot path engaged:\n{stderr}"
    );
    assert_tree_matches_source(&fx.heavy(), &fx.worktree_path("origin-two").join("heavy"));
}
