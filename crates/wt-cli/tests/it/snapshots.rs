//! Integration tests for whole-directory snapshots, incremental v2 snapshot rebuilds,
//! lockfile fastpath, and APFS defaults.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread;
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use crate::common::{Fixture, list_files};
use wt_store::{Manifest, PublishOptions, SnapshotEntry};

// =========================================================================
// APFS Defaults & Opt-Out Mechanics (from apfs_defaults.rs)
// =========================================================================

#[cfg(target_os = "macos")]
#[test]
fn apfs_defaults_enable_snapshots_without_explicit_env() {
    let fx = Fixture::heavy_repo(10);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();
    let out = fx.wt_env(&["create", "snap-auto"], &[]);
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

#[cfg(target_os = "macos")]
#[test]
fn apfs_opt_out_disables_snapshots_via_env() {
    let fx = Fixture::heavy_repo(10);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();
    let out = fx.wt_env(
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

// =========================================================================
// Tiered Lockfile Validation Fast-Path (from lockfile_fastpath.rs)
// =========================================================================

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
    assert_eq!(
        classify_lockfile(MUTABLE_FILE_LOCKFILE),
        DependencySafety::Mutable
    );
    assert_eq!(
        classify_lockfile(MUTABLE_LINK_LOCKFILE),
        DependencySafety::Mutable
    );
    assert_eq!(
        classify_lockfile(MUTABLE_WORKSPACE_LOCKFILE),
        DependencySafety::Mutable
    );
    assert_eq!(
        classify_lockfile(MUTABLE_UNPINNED_GIT_LOCKFILE),
        DependencySafety::Mutable
    );
}

#[test]
fn lockfiles_with_mutable_dependencies_bypass_fast_path() {
    for (lock_content, label) in [
        (MUTABLE_FILE_LOCKFILE, "file"),
        (MUTABLE_LINK_LOCKFILE, "link"),
        (MUTABLE_WORKSPACE_LOCKFILE, "workspace"),
        (MUTABLE_UNPINNED_GIT_LOCKFILE, "unpinned_git"),
    ] {
        let fx = Fixture::lockfile_repo(lock_content);
        let out = fx.wt_env(
            &[
                "create",
                &format!("wt-{label}"),
                "--dir",
                &fx.worktree_path(&format!("dest-{label}")).to_string_lossy(),
            ],
            &[("WT_SNAPSHOTS", "1")],
        );
        assert!(
            out.status.success(),
            "create failed for {label}: {}",
            String::from_utf8_lossy(&out.stderr)
        );

        let dest_file = fx
            .worktree_path(&format!("dest-{label}"))
            .join("node_modules/pkg/lib/index.js");
        assert!(
            dest_file.is_file(),
            "file must be hydrated via fallback ladder for {label}"
        );
    }
}

#[test]
fn pinned_lockfile_evaluates_sha256_and_manifest_header() {
    let fx = Fixture::lockfile_repo(PINNED_LOCKFILE);
    let out = fx.wt_env(
        &[
            "create",
            "one",
            "--dir",
            &fx.worktree_path("dest-one").to_string_lossy(),
        ],
        &[("WT_SNAPSHOTS", "1")],
    );
    assert!(
        out.status.success(),
        "create one failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let out2 = fx.wt_env(
        &[
            "create",
            "two",
            "--dir",
            &fx.worktree_path("dest-two").to_string_lossy(),
        ],
        &[("WT_SNAPSHOTS", "1")],
    );
    assert!(
        out2.status.success(),
        "create two failed: {}",
        String::from_utf8_lossy(&out2.stderr)
    );
    let dest_file = fx
        .worktree_path("dest-two")
        .join("node_modules/pkg/lib/index.js");
    assert!(dest_file.is_file());
}

#[test]
fn lockfile_content_change_invalidates_fast_path() {
    let fx = Fixture::lockfile_repo(PINNED_LOCKFILE);
    let out = fx.wt_env(
        &[
            "create",
            "one",
            "--dir",
            &fx.worktree_path("dest-one").to_string_lossy(),
        ],
        &[("WT_SNAPSHOTS", "1")],
    );
    assert!(
        out.status.success(),
        "create one failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    fs::write(fx.repo.join("package-lock.json"), PINNED_LOCKFILE_V2).unwrap();

    let out2 = fx.wt_env(
        &[
            "create",
            "two",
            "--dir",
            &fx.worktree_path("dest-two").to_string_lossy(),
        ],
        &[("WT_SNAPSHOTS", "1")],
    );
    assert!(
        out2.status.success(),
        "create two failed: {}",
        String::from_utf8_lossy(&out2.stderr)
    );
    let dest_file = fx
        .worktree_path("dest-two")
        .join("node_modules/pkg/lib/index.js");
    assert!(dest_file.is_file());
}

#[test]
fn directory_timestamp_change_triggers_revalidation() {
    let fx = Fixture::lockfile_repo(PINNED_LOCKFILE);
    let out = fx.wt_env(
        &[
            "create",
            "one",
            "--dir",
            &fx.worktree_path("dest-one").to_string_lossy(),
        ],
        &[("WT_SNAPSHOTS", "1")],
    );
    assert!(
        out.status.success(),
        "create one failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let heavy = fx.repo.join("node_modules");
    let future = std::time::SystemTime::now() + std::time::Duration::from_secs(100);
    let f = fs::OpenOptions::new()
        .write(true)
        .open(heavy.join("pkg/lib/index.js"))
        .unwrap();
    f.set_times(fs::FileTimes::new().set_modified(future))
        .unwrap();

    let out2 = fx.wt_env(
        &[
            "create",
            "two",
            "--dir",
            &fx.worktree_path("dest-two").to_string_lossy(),
        ],
        &[("WT_SNAPSHOTS", "1")],
    );
    assert!(
        out2.status.success(),
        "create two failed: {}",
        String::from_utf8_lossy(&out2.stderr)
    );
    let dest_file = fx
        .worktree_path("dest-two")
        .join("node_modules/pkg/lib/index.js");
    assert!(dest_file.is_file());
}

// =========================================================================
// Whole-Directory Snapshot Hydration (from snapshots.rs)
// =========================================================================

#[cfg(target_os = "macos")]
fn assert_created(out: &Output) {
    assert!(
        out.status.success(),
        "create failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[cfg(target_os = "macos")]
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

#[cfg(target_os = "macos")]
const SNAPSHOTS_ON: &[(&str, &str)] = &[("WT_SNAPSHOTS", "1")];
#[cfg(target_os = "macos")]
const SNAPSHOTS_OFF: &[(&str, &str)] = &[("WT_SNAPSHOTS", "0"), ("WT_SNAPSHOTS_V2", "0")];

#[cfg(target_os = "macos")]
#[test]
fn timing_lines_attribute_snapshot_lookup_build_and_clone() {
    let fx = Fixture::rich_repo();
    let env: &[(&str, &str)] = &[("WT_SNAPSHOTS", "1"), ("WT_TIMING", "1")];

    let out = fx.wt_env(
        &[
            "create",
            "one",
            "--dir",
            &fx.worktree_path("origin-one").to_string_lossy(),
        ],
        env,
    );
    assert_created(&out);
    let stderr = String::from_utf8_lossy(&out.stderr);
    for line in [
        "wt-stage snapshot=",
        "wt-stage snapshot-lookup=",
        "wt-stage snapshot-clonefile=",
        "wt-stage snapshot-build-verify=",
        "wt-stage snapshot-build-link-train=",
        "wt-stage snapshot-build-publish=",
    ] {
        assert!(
            stderr.contains(line),
            "cold run missing `{line}`:\n{stderr}"
        );
    }

    let out = fx.wt_env(
        &[
            "create",
            "two",
            "--dir",
            &fx.worktree_path("origin-two").to_string_lossy(),
        ],
        env,
    );
    assert_created(&out);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("wt-stage snapshot-lookup="),
        "warm run must report snapshot-lookup:\n{stderr}"
    );
    assert!(
        stderr.contains("wt-stage snapshot-clonefile="),
        "warm run must report snapshot-clonefile:\n{stderr}"
    );
    assert!(
        !stderr.contains("snapshot-build"),
        "a warm hit builds nothing, so no build phases may be reported:\n{stderr}"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn miss_builds_publishes_and_second_create_hits_with_private_files() {
    let fx = Fixture::rich_repo();
    let source = fx.repo.join("heavy");

    let out = fx.wt_env(
        &[
            "create",
            "one",
            "--dir",
            &fx.worktree_path("origin-one").to_string_lossy(),
        ],
        SNAPSHOTS_ON,
    );
    assert_created(&out);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("via snapshot"),
        "first create should report the snapshot path: {stdout}"
    );

    let snapshots_dir = fx.store_path().join("snapshots");
    let published: Vec<String> = fs::read_dir(&snapshots_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| wt_store::ContentId::from_hex(n).is_some())
        .collect();
    assert_eq!(published.len(), 1, "exactly one snapshot expected");

    let mirrors = wt_store::read_mirrors(&fx.store_path());
    assert_eq!(mirrors.len(), 1);
    let mirror = mirrors[0].mirror.as_ref().expect("valid mirror");
    assert_eq!(mirror.snapshots.len(), 1, "one snapshot record");
    assert!(
        mirror.files.is_empty(),
        "snapshot hydration writes no per-file blob records"
    );

    let snap_dir = snapshots_dir.join(&published[0]);
    let born = fs::metadata(&snap_dir).unwrap().created().unwrap();
    let out = fx.wt_env(
        &[
            "create",
            "two",
            "--dir",
            &fx.worktree_path("origin-two").to_string_lossy(),
        ],
        &[("WT_SNAPSHOTS", "1"), ("WT_TIMING", "1")],
    );
    assert_created(&out);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("via snapshot"),
        "second create should take the fast path: {stdout}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("wt-stage snapshot="),
        "timing should show the snapshot stage: {stderr}"
    );
    assert_eq!(
        fs::metadata(&snap_dir).unwrap().created().unwrap(),
        born,
        "a hit must reuse the published snapshot, never rebuild it"
    );

    for name in ["origin-one", "origin-two"] {
        let heavy_root = fx.worktree_path(name).join("heavy");
        assert_tree_matches_source(&source, &heavy_root);

        let plain = heavy_root.join("pkg00/nested/file-0.txt");
        let meta = fs::metadata(&plain).unwrap();
        assert_eq!(meta.nlink(), 1, "{} must own a private inode", name);
        assert_ne!(meta.mode() & 0o777, 0o444);
        assert_eq!(
            meta.mode() & 0o200,
            0o200,
            "{name} file must be owner-writable"
        );

        let exec = heavy_root.join("exec.sh");
        let meta = fs::metadata(&exec).unwrap();
        assert_eq!(meta.nlink(), 1);
        assert_eq!(
            meta.mode() & 0o111,
            0o111,
            "exec bits must survive the clone"
        );
        assert_eq!(meta.mode() & 0o200, 0o200);
    }
    let a = fs::metadata(fx.worktree_path("origin-one").join("heavy/exec.sh"))
        .unwrap()
        .ino();
    let b = fs::metadata(fx.worktree_path("origin-two").join("heavy/exec.sh"))
        .unwrap()
        .ino();
    assert_ne!(a, b, "cloned trees must not share inodes with each other");
}

#[cfg(target_os = "macos")]
#[test]
fn gate_off_changes_nothing_at_all() {
    let fx = Fixture::rich_repo();
    let out = fx.wt_env(
        &[
            "create",
            "one",
            "--dir",
            &fx.worktree_path("origin-one").to_string_lossy(),
        ],
        SNAPSHOTS_OFF,
    );
    assert_created(&out);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("via snapshot"),
        "gate off must keep the per-file ladder: {stdout}"
    );
    assert!(
        !fx.store_path().join("snapshots").exists(),
        "gate off must not create a snapshots directory"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn wt_verify_bypasses_hits_and_hashes_every_blob() {
    let fx = Fixture::rich_repo();

    assert_created(&fx.wt_env(
        &[
            "create",
            "one",
            "--dir",
            &fx.worktree_path("origin-one").to_string_lossy(),
        ],
        SNAPSHOTS_ON,
    ));

    let mut tampered = None;
    let objects = fx.store_path().join("objects");
    for shard in fs::read_dir(&objects).unwrap() {
        for blob in fs::read_dir(shard.unwrap().path()).unwrap() {
            let path = blob.unwrap().path();
            let meta = fs::metadata(&path).unwrap();
            if meta.len() == b"file zero\n".len() as u64 {
                let mtime = meta.modified().unwrap();
                fs::write(&path, b"FILE ZERO").unwrap();
                let f = fs::OpenOptions::new().write(true).open(&path).unwrap();
                f.set_times(std::fs::FileTimes::new().set_modified(mtime))
                    .unwrap();
                tampered = Some(path);
            }
        }
    }
    assert!(tampered.is_some(), "fixture must contain the target blob");

    let out = fx.wt_env(
        &[
            "create",
            "two",
            "--dir",
            &fx.worktree_path("origin-two").to_string_lossy(),
        ],
        SNAPSHOTS_ON,
    );
    assert_created(&out);

    let out = fx.wt_env(
        &[
            "create",
            "three",
            "--dir",
            &fx.worktree_path("origin-three").to_string_lossy(),
        ],
        &[("WT_SNAPSHOTS", "1"), ("WT_VERIFY", "1")],
    );
    assert!(
        !out.status.success(),
        "paranoid create must fail loudly on a corrupt blob"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("hash verification"),
        "failure must name the corruption: {stderr}"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn concurrent_identical_creates_one_publish_wins_loser_consumes_winner() {
    let fx = Fixture::rich_repo();

    let repo = fx.repo.clone();
    let store = fx.store_path();
    let one = fx.worktree_path("race-one");
    let two = fx.worktree_path("race-two");
    let (out_a, out_b) = thread::scope(|s| {
        let a = s.spawn({
            let repo = repo.clone();
            let store = store.clone();
            let one = one.clone();
            move || {
                Command::new(env!("CARGO_BIN_EXE_flashwt"))
                    .args(["create", "race-one"])
                    .arg("--dir")
                    .arg(&one)
                    .env("WT_STORE", &store)
                    .env("WT_SNAPSHOTS", "1")
                    .current_dir(&repo)
                    .output()
                    .expect("run wt binary")
            }
        });
        let b = s.spawn(move || {
            Command::new(env!("CARGO_BIN_EXE_flashwt"))
                .args(["create", "race-two"])
                .arg("--dir")
                .arg(&two)
                .env("WT_STORE", &store)
                .env("WT_SNAPSHOTS", "1")
                .current_dir(&repo)
                .output()
                .expect("run wt binary")
        });
        (a.join().unwrap(), b.join().unwrap())
    });

    assert_created(&out_a);
    assert_created(&out_b);

    let snapshots_dir = fx.store_path().join("snapshots");
    let published: Vec<String> = fs::read_dir(&snapshots_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| wt_store::ContentId::from_hex(n).is_some())
        .collect();
    assert_eq!(
        published.len(),
        1,
        "identical content must converge on one snapshot"
    );

    assert_tree_matches_source(
        &fx.repo.join("heavy"),
        &fx.worktree_path("race-one").join("heavy"),
    );
    assert_tree_matches_source(
        &fx.repo.join("heavy"),
        &fx.worktree_path("race-two").join("heavy"),
    );
}

#[cfg(target_os = "macos")]
fn bump_source_wide(heavy: &Path) {
    for pkg in 0..4 {
        for f in 0..9 {
            let p = heavy.join(format!("pkg0{pkg}/file-{f:03}.txt"));
            fs::write(&p, format!("package {pkg} file {f} WIDE EDIT\n")).unwrap();
        }
    }
}

#[cfg(target_os = "macos")]
#[test]
fn incremental_rebuild_guard_narrow_diff_surfaces_hit_in_json_and_timing() {
    let fx = Fixture::v2_repo(5, 50);

    assert_created(&fx.wt_env(
        &[
            "create",
            "one",
            "--dir",
            &fx.worktree_path("origin-one").to_string_lossy(),
        ],
        BOTH_GATES,
    ));

    bump_source(&fx.repo.join("heavy"));

    // Verify JSON output on first rebuild of this new manifest
    let out_json = fx.wt_env(
        &[
            "create",
            "two-json",
            "--json",
            "--dir",
            &fx.worktree_path("origin-two-json").to_string_lossy(),
        ],
        BOTH_GATES,
    );
    assert!(out_json.status.success());
    let json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&out_json.stdout).trim()).expect("parse JSON");
    assert_eq!(json["status"], "ok");
    assert_eq!(json["data"]["cache_hit"], true);
    assert_eq!(json["data"]["incremental_decision"], "hit");
    assert!(json["data"]["incremental_fallback_reason"].is_null());

    // Bump again to create a fresh narrow diff (file 1 in pkg02)
    fs::write(
        fx.repo.join("heavy/pkg02/file-001.txt"),
        "package 2 file 1 FRESH NARROW EDIT\n",
    )
    .unwrap();

    // Verify WT_TIMING=1 emission on fresh incremental rebuild
    let out_timing = fx.wt_env(
        &[
            "create",
            "two-timing",
            "--dir",
            &fx.worktree_path("origin-two-timing").to_string_lossy(),
        ],
        &[BOTH_GATES, &[("WT_TIMING", "1")]].concat(),
    );
    assert!(out_timing.status.success());
    let stderr = String::from_utf8_lossy(&out_timing.stderr);
    assert!(
        stderr.contains("wt-stage snapshot-mode=v2"),
        "narrow diff must rebuild incrementally in timing: {stderr}"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn incremental_rebuild_guard_wide_diff_surfaces_fallback_in_json_and_timing() {
    let fx = Fixture::v2_repo(5, 50);

    assert_created(&fx.wt_env(
        &[
            "create",
            "one",
            "--dir",
            &fx.worktree_path("origin-one").to_string_lossy(),
        ],
        BOTH_GATES,
    ));

    bump_source_wide(&fx.repo.join("heavy"));

    // Verify JSON output on first rebuild of this wide diff
    let out_json = fx.wt_env(
        &[
            "create",
            "two-json",
            "--json",
            "--dir",
            &fx.worktree_path("origin-two-json").to_string_lossy(),
        ],
        BOTH_GATES,
    );
    assert!(out_json.status.success());
    let json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&out_json.stdout).trim()).expect("parse JSON");
    assert_eq!(json["status"], "ok");
    assert_eq!(json["data"]["cache_hit"], false);
    assert_eq!(json["data"]["incremental_decision"], "diff_too_wide");
    let reason = json["data"]["incremental_fallback_reason"]
        .as_str()
        .expect("fallback reason present");
    assert!(
        reason.contains("exceeds maximum threshold"),
        "expected fallback reason to mention threshold: {reason}"
    );

    // Bump wide again to create a fresh wide diff
    for pkg in 0..4 {
        for f in 10..19 {
            let p = fx.repo.join("heavy").join(format!("pkg0{pkg}/file-{f:03}.txt"));
            fs::write(&p, format!("package {pkg} file {f} FRESH WIDE EDIT\n")).unwrap();
        }
    }

    // Verify WT_TIMING=1 emission on fresh wide diff fallback
    let out_timing = fx.wt_env(
        &[
            "create",
            "two-timing",
            "--dir",
            &fx.worktree_path("origin-two-timing").to_string_lossy(),
        ],
        &[BOTH_GATES, &[("WT_TIMING", "1")]].concat(),
    );
    assert!(out_timing.status.success());
    let stderr = String::from_utf8_lossy(&out_timing.stderr);
    assert!(
        stderr.contains("wt-stage snapshot-mode=build"),
        "wide diff must fall back to full build in timing: {stderr}"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn incremental_rebuild_guard_lockfile_miss_surfaces_fallback_in_json_and_timing() {
    let fx = Fixture::v2_repo(5, 50);

    // Initial lockfile
    fs::write(fx.repo.join("package-lock.json"), PINNED_LOCKFILE).unwrap();

    assert_created(&fx.wt_env(
        &[
            "create",
            "one",
            "--dir",
            &fx.worktree_path("origin-one").to_string_lossy(),
        ],
        BOTH_GATES,
    ));

    // Narrow bump to heavy, but lockfile changed to V2!
    bump_source(&fx.repo.join("heavy"));
    fs::write(fx.repo.join("package-lock.json"), PINNED_LOCKFILE_V2).unwrap();

    // Verify JSON output on first rebuild with lockfile miss
    let out_json = fx.wt_env(
        &[
            "create",
            "two-json",
            "--json",
            "--dir",
            &fx.worktree_path("origin-two-json").to_string_lossy(),
        ],
        BOTH_GATES,
    );
    assert!(out_json.status.success());
    let json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&out_json.stdout).trim()).expect("parse JSON");
    assert_eq!(json["status"], "ok");
    assert_eq!(json["data"]["cache_hit"], false);
    assert_eq!(json["data"]["incremental_decision"], "lockfile_miss");
    let reason = json["data"]["incremental_fallback_reason"]
        .as_str()
        .expect("fallback reason present");
    assert!(
        reason.contains("lockfile hash mismatch"),
        "expected fallback reason to mention lockfile hash mismatch: {reason}"
    );

    // Bump again with a fresh lockfile change (version 1.0.2)
    fs::write(
        fx.repo.join("heavy/pkg02/file-001.txt"),
        "package 2 file 1 FRESH LOCKFILE EDIT\n",
    )
    .unwrap();
    let lockfile_v3 = PINNED_LOCKFILE_V2.replace("1.0.1", "1.0.2");
    fs::write(fx.repo.join("package-lock.json"), lockfile_v3).unwrap();

    // Verify WT_TIMING=1 emission on fresh lockfile miss fallback
    let out_timing = fx.wt_env(
        &[
            "create",
            "two-timing",
            "--dir",
            &fx.worktree_path("origin-two-timing").to_string_lossy(),
        ],
        &[BOTH_GATES, &[("WT_TIMING", "1")]].concat(),
    );
    assert!(out_timing.status.success());
    let stderr = String::from_utf8_lossy(&out_timing.stderr);
    assert!(
        stderr.contains("wt-stage snapshot-mode=build"),
        "lockfile miss must fall back to full build in timing: {stderr}"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn evicted_snapshot_referenced_by_mirror_rebuilds_on_next_create() {
    let fx = Fixture::rich_repo();

    assert_created(&fx.wt_env(
        &[
            "create",
            "one",
            "--dir",
            &fx.worktree_path("origin-one").to_string_lossy(),
        ],
        SNAPSHOTS_ON,
    ));

    let snapshots_dir = fx.store_path().join("snapshots");
    let hash = fs::read_dir(&snapshots_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .find(|n| wt_store::ContentId::from_hex(n).is_some())
        .expect("published snapshot");
    fs::remove_dir_all(snapshots_dir.join(&hash)).unwrap();

    let mirrors = wt_store::read_mirrors(&fx.store_path());
    let mirror = mirrors[0].mirror.as_ref().unwrap();
    assert_eq!(mirror.snapshots.len(), 1);
    let store = wt_store::DiskStore::open(fx.store_path()).unwrap();
    let report = store
        .compute_marks(std::time::SystemTime::now(), Duration::from_secs(900))
        .unwrap();
    assert_eq!(report.unresolved_snapshots, 1);
    assert!(
        report.marked.is_empty(),
        "no blobs may be marked through a missing snapshot"
    );

    let out = fx.wt_env(
        &[
            "create",
            "two",
            "--dir",
            &fx.worktree_path("origin-two").to_string_lossy(),
        ],
        SNAPSHOTS_ON,
    );
    assert_created(&out);
    assert_tree_matches_source(
        &fx.repo.join("heavy"),
        &fx.worktree_path("origin-two").join("heavy"),
    );
    assert!(
        snapshots_dir.join(&hash).exists(),
        "snapshot rebuilt at the same address"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn fifo_fails_loudly_under_gate_and_is_skipped_without_it() {
    let fx = Fixture::rich_repo();
    let fifo = fx.repo.join("heavy/pkg00/fifo");
    let c_path = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).expect("cstr");
    unsafe {
        assert_eq!(libc::mkfifo(c_path.as_ptr(), 0o644), 0, "mkfifo failed");
    }

    let out = fx.wt_env(
        &[
            "create",
            "one",
            "--dir",
            &fx.worktree_path("origin-one").to_string_lossy(),
        ],
        SNAPSHOTS_ON,
    );
    assert!(
        !out.status.success(),
        "fifos must fail loudly under the gate"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not a regular file"),
        "error must explain the rejection: {stderr}"
    );
    assert!(
        !fx.worktree_path("origin-one").join("heavy").exists(),
        "nothing may be placed when ingest rejects the tree"
    );

    let out = fx.wt_env(
        &[
            "create",
            "two",
            "--dir",
            &fx.worktree_path("origin-two").to_string_lossy(),
        ],
        SNAPSHOTS_OFF,
    );
    assert_created(&out);
    assert!(
        fx.worktree_path("origin-two")
            .join("heavy/pkg00/nested/file-0.txt")
            .exists()
    );
}

#[cfg(target_os = "macos")]
#[test]
fn invalid_winner_debris_falls_back_to_per_file_ladder() {
    let fx = Fixture::rich_repo();

    let store = wt_store::DiskStore::open(fx.store_path()).unwrap();
    let mut store = store;
    let f0 = store.put(b"file zero\n").unwrap();
    let deep = store.put(b"deep c\n").unwrap();
    let exec = store.put(b"#!/bin/sh\necho hi\n").unwrap();
    let entries = vec![
        SnapshotEntry::dir("deep"),
        SnapshotEntry::dir("deep/a"),
        SnapshotEntry::dir("deep/a/b"),
        SnapshotEntry::dir("empty"),
        SnapshotEntry::dir("pkg00"),
        SnapshotEntry::dir("pkg00/nested"),
        SnapshotEntry::file("deep/a/b/c.txt", deep, 0o644),
        SnapshotEntry::file("exec.sh", exec, 0o755),
        SnapshotEntry::file("pkg00/nested/file-0.txt", f0, 0o644),
        SnapshotEntry::symlink("bin-link", "../exec.sh"),
    ];
    let hash = Manifest::new(entries).unwrap().hash.to_string();
    let debris = fx.store_path().join("snapshots").join(&hash);
    fs::create_dir_all(&debris).unwrap();
    fs::write(debris.join("manifest.tsv"), "garbage\n").unwrap();
    store.flush().unwrap();

    let out = fx.wt_env(
        &[
            "create",
            "one",
            "--dir",
            &fx.worktree_path("origin-one").to_string_lossy(),
        ],
        SNAPSHOTS_ON,
    );
    assert_created(&out);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("via snapshot"),
        "debris must force the per-file ladder: {stdout}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("falling back"),
        "the fallback must be reported: {stderr}"
    );
    assert_eq!(
        fs::read_to_string(debris.join("manifest.tsv")).unwrap(),
        "garbage\n"
    );

    let heavy = fx.worktree_path("origin-one").join("heavy");
    assert_eq!(
        fs::read_to_string(heavy.join("pkg00/nested/file-0.txt")).unwrap(),
        "file zero\n"
    );
    assert!(heavy.join("empty").is_dir());
}

#[cfg(target_os = "macos")]
#[test]
fn gc_marks_through_valid_manifests_only() {
    let base = tempfile::tempdir().unwrap();
    let mut store = wt_store::DiskStore::open(base.path().join("store")).unwrap();

    let b1 = store.put(b"alpha").unwrap();
    let b2 = store.put(b"beta").unwrap();
    let entries = vec![
        SnapshotEntry::dir("d"),
        SnapshotEntry::file("d/a", b1, 0o644),
        SnapshotEntry::file("d/b", b2, 0o644),
    ];
    let m = Manifest::new(entries).unwrap();
    assert_eq!(
        store
            .publish_snapshot(m.entries.clone(), PublishOptions::default())
            .unwrap()
            .outcome,
        wt_store::PublishOutcome::Published
    );

    let wt_dir = base.path().join("wt");
    fs::create_dir_all(&wt_dir).unwrap();
    let gitdir = base.path().join("wt.git");
    fs::create_dir_all(&gitdir).unwrap();
    fs::write(gitdir.join("wt-hydrated.tsv"), "").unwrap();
    store
        .publish_worktree_mirror(&wt_dir, &gitdir, std::iter::empty(), [&m.hash], None, None)
        .unwrap();

    let now = std::time::SystemTime::now();
    let grace = Duration::from_secs(900);
    let report = store.compute_marks(now, grace).unwrap();
    assert_eq!(
        report.referenced_snapshots,
        [m.hash]
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
    );
    assert!(
        report.marked.contains(&b1) && report.marked.contains(&b2),
        "every file entry's blob must be marked through the manifest"
    );

    fs::remove_dir_all(store.snapshot_path(&m.hash)).unwrap();
    let report = store.compute_marks(now, grace).unwrap();
    assert_eq!(report.unresolved_snapshots, 1);
    assert!(report.marked.is_empty());
    store.flush().unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn ladder_and_snapshot_hydrate_identical_trees() {
    let fx = Fixture::rich_repo();
    assert_created(&fx.wt_env(
        &[
            "create",
            "plain",
            "--dir",
            &fx.worktree_path("origin-plain").to_string_lossy(),
        ],
        &[("WT_SNAPSHOTS", "0"), ("WT_SNAPSHOTS_V2", "0")],
    ));
    assert_created(&fx.wt_env(
        &[
            "create",
            "snap",
            "--dir",
            &fx.worktree_path("origin-snap").to_string_lossy(),
        ],
        SNAPSHOTS_ON,
    ));

    let rel_prefix = |root: &Path| {
        list_files(root)
            .iter()
            .map(|p| p.strip_prefix(root).unwrap().to_path_buf())
            .collect::<Vec<_>>()
    };
    let plain = rel_prefix(&fx.worktree_path("origin-plain").join("heavy"));
    let snap = rel_prefix(&fx.worktree_path("origin-snap").join("heavy"));

    assert_eq!(plain, snap, "ladder and snapshot must place the same paths");
    assert!(
        plain.contains(&PathBuf::from("bin-link")),
        "both paths must carry the symlink"
    );
}

// =========================================================================
// Incremental v2 Snapshot Rebuilds (from snapshots_v2.rs)
// =========================================================================

#[cfg(target_os = "macos")]
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

#[cfg(target_os = "macos")]
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

#[cfg(target_os = "macos")]
const BOTH_GATES: &[(&str, &str)] = &[("WT_SNAPSHOTS", "1"), ("WT_SNAPSHOTS_V2", "1")];

#[cfg(target_os = "macos")]
#[test]
fn bump_rebuilds_incrementally_and_matches_source_exactly() {
    let fx = Fixture::v2_repo(5, 50);

    assert_created(&fx.wt_env(
        &[
            "create",
            "one",
            "--dir",
            &fx.worktree_path("origin-one").to_string_lossy(),
        ],
        BOTH_GATES,
    ));

    bump_source(&fx.repo.join("heavy"));
    let out = fx.wt_env(
        &[
            "create",
            "two",
            "--dir",
            &fx.worktree_path("origin-two").to_string_lossy(),
        ],
        &[BOTH_GATES, &[("WT_TIMING", "1")]].concat(),
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

    let cloned = stderr
        .lines()
        .find_map(|l| l.strip_prefix("wt-stage snapshot-v2-cloned="))
        .map(|v| v.trim().parse::<usize>().expect("integer cloned count"));
    assert_eq!(cloned, Some(1), "the whole old tree must clone as ONE unit");

    let total_files = count_files(&fx.repo.join("heavy"));
    assert!(
        linked < total_files,
        "incremental rebuild hardlinked {linked} of {total_files} files — that is not incremental"
    );

    assert_tree_matches_source(
        &fx.repo.join("heavy"),
        &fx.worktree_path("origin-two").join("heavy"),
    );
}

#[cfg(target_os = "macos")]
#[test]
fn corrupt_old_manifest_falls_back_to_full_build() {
    let fx = Fixture::v2_repo(5, 50);

    assert_created(&fx.wt_env(
        &[
            "create",
            "one",
            "--dir",
            &fx.worktree_path("origin-one").to_string_lossy(),
        ],
        BOTH_GATES,
    ));

    let snapshots_dir = fx.store_path().join("snapshots");
    let hash = fs::read_dir(&snapshots_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .find(|n| wt_store::ContentId::from_hex(n).is_some())
        .expect("published snapshot");
    fs::write(snapshots_dir.join(&hash).join("manifest.tsv"), "garbage\n").unwrap();

    bump_source(&fx.repo.join("heavy"));
    let out = fx.wt_env(
        &[
            "create",
            "two",
            "--dir",
            &fx.worktree_path("origin-two").to_string_lossy(),
        ],
        &[BOTH_GATES, &[("WT_TIMING", "1")]].concat(),
    );
    assert_created(&out);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("wt-stage snapshot-mode=build"),
        "with no usable old snapshot the create must take the plain full build:\n{stderr}"
    );
    assert_tree_matches_source(
        &fx.repo.join("heavy"),
        &fx.worktree_path("origin-two").join("heavy"),
    );
}

#[cfg(target_os = "macos")]
#[test]
fn paranoid_verify_with_v2_gates_still_lands_an_exact_tree() {
    let fx = Fixture::v2_repo(5, 50);

    assert_created(&fx.wt_env(
        &[
            "create",
            "one",
            "--dir",
            &fx.worktree_path("origin-one").to_string_lossy(),
        ],
        BOTH_GATES,
    ));

    bump_source(&fx.repo.join("heavy"));
    let out = fx.wt_env(
        &[
            "create",
            "two",
            "--dir",
            &fx.worktree_path("origin-two").to_string_lossy(),
        ],
        &[
            ("WT_SNAPSHOTS", "1"),
            ("WT_SNAPSHOTS_V2", "1"),
            ("WT_VERIFY", "1"),
            ("WT_TIMING", "1"),
        ],
    );
    assert_created(&out);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("wt-stage snapshot-mode="),
        "mode line must be emitted whenever the snapshot path engaged:\n{stderr}"
    );
    assert_tree_matches_source(
        &fx.repo.join("heavy"),
        &fx.worktree_path("origin-two").join("heavy"),
    );
}

#[cfg(target_os = "macos")]
fn published_hashes(store: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(store.join("snapshots")) else {
        return Vec::new();
    };
    let mut out: Vec<String> = entries
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.len() == 64 && n.bytes().all(|b| b.is_ascii_hexdigit()))
        .collect();
    out.sort();
    out
}

#[cfg(target_os = "macos")]
fn manifest_of(store: &Path, hash: &str) -> wt_store::Manifest {
    let id = wt_store::ContentId::from_hex(hash).expect("hex hash");
    let text =
        fs::read_to_string(wt_store::snapshot_path(store, &id).join("manifest.tsv")).unwrap();
    wt_store::Manifest::parse(&text).expect("valid manifest")
}

#[cfg(target_os = "macos")]
fn blob_ids(manifest: &wt_store::Manifest) -> BTreeSet<wt_store::ContentId> {
    manifest.entries.iter().filter_map(|e| e.blob).collect()
}

#[cfg(target_os = "macos")]
fn object_path(store: &Path, id: &wt_store::ContentId) -> PathBuf {
    let hex = id.to_string();
    store.join("objects").join(&hex[..2]).join(&hex[2..])
}

#[cfg(target_os = "macos")]
fn assert_no_incomplete_published(store: &Path, iteration: usize) {
    for name in published_hashes(store) {
        let hash = wt_store::ContentId::from_hex(&name).expect("hex address");
        assert!(
            wt_store::read_published_snapshot(store, &hash).is_some(),
            "kill iteration {iteration}: {name} sits at a published address but fails the shared validity check"
        );
    }
}

#[cfg(target_os = "macos")]
const KILL_DELAYS_MS: &[u64] = &[10, 60, 120, 180, 240, 300, 350, 400];

#[cfg(target_os = "macos")]
#[test]
fn sigkilled_creates_never_leave_a_half_published_snapshot_behind() {
    let fx = Fixture::v2_repo(5, 50);

    for (i, delay_ms) in KILL_DELAYS_MS.iter().enumerate() {
        let mut child = Command::new(env!("CARGO_BIN_EXE_flashwt"))
            .args(["create", &format!("killed-{i}")])
            .arg("--dir")
            .arg(fx.worktree_path(&format!("origin-killed-{i}")))
            .env("WT_STORE", fx.store_path())
            .envs(BOTH_GATES.iter().copied())
            .env_remove("WT_HARDLINK")
            .env_remove("WT_NO_HARDLINK")
            .env_remove("WT_GC_GRACE")
            .current_dir(&fx.repo)
            .spawn()
            .expect("spawn wt create");
        thread::sleep(Duration::from_millis(*delay_ms));
        let _ = child.kill();
        let _ = child.wait();

        assert_no_incomplete_published(&fx.store_path(), i);

        let out = fx.wt_env(
            &[
                "create",
                &format!("after-{i}"),
                "--dir",
                &fx.worktree_path(&format!("origin-after-{i}"))
                    .to_string_lossy(),
            ],
            BOTH_GATES,
        );
        assert_created(&out);
        assert_tree_matches_source(
            &fx.repo.join("heavy"),
            &fx.worktree_path(&format!("origin-after-{i}")).join("heavy"),
        );
    }
}

#[cfg(target_os = "macos")]
#[test]
fn sweep_evicts_unreferenced_old_generation_but_shared_blobs_survive() {
    let fx = Fixture::v2_repo(5, 50);
    let store = fx.store_path();

    assert_created(&fx.wt_env(
        &[
            "create",
            "one",
            "--dir",
            &fx.worktree_path("origin-one").to_string_lossy(),
        ],
        BOTH_GATES,
    ));
    let before = published_hashes(&store);
    assert_eq!(before.len(), 1);
    let a_hash = before[0].clone();

    bump_source(&fx.repo.join("heavy"));
    assert_created(&fx.wt_env(
        &[
            "create",
            "two",
            "--dir",
            &fx.worktree_path("origin-two").to_string_lossy(),
        ],
        BOTH_GATES,
    ));
    let after = published_hashes(&store);
    assert_eq!(after.len(), 2);
    let b_hash = after
        .iter()
        .find(|h| **h != a_hash)
        .expect("the second generation published under a fresh hash");

    let a_blobs = blob_ids(&manifest_of(&store, &a_hash));
    let b_blobs = blob_ids(&manifest_of(&store, b_hash));
    let shared: Vec<_> = a_blobs.intersection(&b_blobs).copied().collect();
    let exclusive_a: Vec<_> = a_blobs.difference(&b_blobs).copied().collect();
    let exclusive_b: Vec<_> = b_blobs.difference(&a_blobs).copied().collect();
    assert!(!shared.is_empty());
    assert!(!exclusive_a.is_empty());
    assert!(!exclusive_b.is_empty());

    let removed = fx.wt_with_store(&["remove", "one"], &store);
    assert!(removed.status.success());

    assert!(
        fx.wt_with_store(&["store", "migrate", "--activate-mark-sweep"], &store)
            .status
            .success()
    );
    let swept = fx.wt_with_store(&["sweep", "--age", "0s"], &store);
    assert!(swept.status.success());

    assert!(
        !wt_store::snapshot_path(&store, &wt_store::ContentId::from_hex(&a_hash).unwrap()).exists()
    );
    assert!(
        wt_store::snapshot_path(&store, &wt_store::ContentId::from_hex(b_hash).unwrap()).exists()
    );
    for id in &exclusive_a {
        assert!(!object_path(&store, id).exists());
    }
    for id in shared.iter().chain(exclusive_b.iter()) {
        assert!(object_path(&store, id).is_file());
    }

    assert!(store.join("snapshots/index.tsv").is_file());
    let index = fs::read_to_string(store.join("snapshots/index.tsv")).unwrap();
    assert!(index.contains(b_hash));
}

#[cfg(target_os = "macos")]
#[test]
fn sweep_keeps_old_generation_alive_when_newer_snapshot_dir_vanishes() {
    let fx = Fixture::v2_repo(5, 50);
    let store = fx.store_path();

    assert_created(&fx.wt_env(
        &[
            "create",
            "one",
            "--dir",
            &fx.worktree_path("origin-one").to_string_lossy(),
        ],
        BOTH_GATES,
    ));
    let a_hash = published_hashes(&store)[0].clone();

    bump_source(&fx.repo.join("heavy"));
    assert_created(&fx.wt_env(
        &[
            "create",
            "two",
            "--dir",
            &fx.worktree_path("origin-two").to_string_lossy(),
        ],
        BOTH_GATES,
    ));
    let b_hash = published_hashes(&store)
        .into_iter()
        .find(|h| *h != a_hash)
        .unwrap();

    let a_manifest = manifest_of(&store, &a_hash);
    let b_manifest = manifest_of(&store, &b_hash);
    let exclusive_b: Vec<_> = blob_ids(&b_manifest)
        .difference(&blob_ids(&a_manifest))
        .copied()
        .collect();
    assert!(!exclusive_b.is_empty());

    fs::remove_dir_all(wt_store::snapshot_path(
        &store,
        &wt_store::ContentId::from_hex(&b_hash).unwrap(),
    ))
    .unwrap();

    assert!(
        fx.wt_with_store(&["store", "migrate", "--activate-mark-sweep"], &store)
            .status
            .success()
    );
    let swept = fx.wt_with_store(&["sweep", "--age", "0s"], &store);
    assert!(swept.status.success());

    assert!(
        wt_store::snapshot_path(&store, &wt_store::ContentId::from_hex(&a_hash).unwrap()).is_dir()
    );
    for entry in &a_manifest.entries {
        if let Some(id) = entry.blob {
            assert!(object_path(&store, &id).is_file());
        }
    }
    for id in &exclusive_b {
        assert!(!object_path(&store, id).exists());
    }
}

#[cfg(target_os = "macos")]
#[test]
fn incremental_snapshot_tree_files_are_private_and_self_contained() {
    let fx = Fixture::v2_repo(5, 50);
    let store = fx.store_path();

    assert_created(&fx.wt_env(
        &[
            "create",
            "one",
            "--dir",
            &fx.worktree_path("origin-one").to_string_lossy(),
        ],
        BOTH_GATES,
    ));
    let a_hash = published_hashes(&store)[0].clone();
    bump_source(&fx.repo.join("heavy"));
    assert_created(&fx.wt_env(
        &[
            "create",
            "two",
            "--dir",
            &fx.worktree_path("origin-two").to_string_lossy(),
        ],
        BOTH_GATES,
    ));
    let b_hash = published_hashes(&store)
        .into_iter()
        .find(|h| *h != a_hash)
        .unwrap();

    let sample =
        wt_store::snapshot_tree_path(&store, &wt_store::ContentId::from_hex(&b_hash).unwrap())
            .join("pkg01/file-000.txt");
    assert_eq!(fs::read_to_string(&sample).unwrap(), "package 1 file 0\n");

    let md = fs::symlink_metadata(&sample).unwrap();
    assert_eq!(md.nlink(), 1);
    assert!(md.is_file());

    fs::remove_dir_all(wt_store::snapshot_path(
        &store,
        &wt_store::ContentId::from_hex(&a_hash).unwrap(),
    ))
    .unwrap();
    assert_eq!(fs::read_to_string(&sample).unwrap(), "package 1 file 0\n");
}

#[cfg(target_os = "macos")]
#[test]
fn unusable_selection_index_never_fails_the_create() {
    let fx = Fixture::v2_repo(5, 50);

    assert_created(&fx.wt_env(
        &[
            "create",
            "one",
            "--dir",
            &fx.worktree_path("origin-one").to_string_lossy(),
        ],
        BOTH_GATES,
    ));
    let index = fx.store_path().join("snapshots/index.tsv");
    let journal = fx.store_path().join("snapshots/journal.tsv");
    if index.is_file() {
        let _ = fs::remove_file(&index);
    }
    fs::create_dir_all(&index).expect("squat a directory on index.tsv");
    if journal.is_file() {
        let _ = fs::remove_file(&journal);
    }
    fs::create_dir_all(&journal).expect("squat a directory on journal.tsv");

    bump_source(&fx.repo.join("heavy"));
    let out = fx.wt_env(
        &[
            "create",
            "two",
            "--dir",
            &fx.worktree_path("origin-two").to_string_lossy(),
        ],
        &[BOTH_GATES, &[("WT_TIMING", "1")]].concat(),
    );
    assert_created(&out);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("wt-stage snapshot-mode=build"),
        "no usable selection means the plain full build:\n{stderr}"
    );
    assert_tree_matches_source(
        &fx.repo.join("heavy"),
        &fx.worktree_path("origin-two").join("heavy"),
    );
}
