// Tests assert with unwrap/expect by design: a panic IS the failure
// signal under test, so the workspace restriction lints stay off here.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Ticket 07 hardlink safety, asserted through the CLI seam, under
//! the WT_HARDLINK=1 opt-in (hardlinks are no longer the default;
//! fast-hydration ticket 03).
//!
//! Hydration can link store objects into worktrees. A package manager
//! rewriting a linked file in place must never reach the sibling
//! worktree or the store; replacement-style writes must keep working
//! on private copies. Everything below runs the real `wt` binary and
//! inspects only files on disk.

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::{Fixture, list_files};

/// rel path -> bytes for every regular file under `dir`.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    list_files(dir)
        .into_iter()
        .map(|p| {
            (
                p.strip_prefix(dir).expect("path under root").to_path_buf(),
                fs::read(&p).expect("read"),
            )
        })
        .collect()
}

fn assert_snapshot_unchanged(before: &BTreeMap<PathBuf, Vec<u8>>, dir: &Path) {
    assert_eq!(&snapshot(dir), before, "{} changed", dir.display());
}

fn object_count(store: &Path) -> usize {
    list_files(&store.join("objects")).len()
}

/// In-place rewrites of shared inodes fail with EACCES — unless the
/// suite runs as root, which ignores permission bits entirely.
fn running_as_root() -> bool {
    Command::new("id")
        .arg("-u")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
        .unwrap_or(false)
}

fn hydrated_file(worktree: &Path) -> PathBuf {
    let f = worktree.join("heavy/pkg00/nested/file-0.txt");
    assert!(f.exists(), "expected hydrated file at {}", f.display());
    f
}

/// Run `wt create` with hardlinked materialization explicitly opted
/// into: hardlinks are no longer the default (fast-hydration
/// ticket 03).
fn wt_hardlinked(fx: &Fixture, name: &str, store: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_wt"))
        .args(["create", name])
        .env("WT_STORE", store)
        .env("WT_HARDLINK", "1")
        .env("WT_SNAPSHOTS", "0")
        .env("WT_SNAPSHOTS_V2", "0")
        .current_dir(&fx.repo)
        .output()
        .expect("run wt binary")
}

#[test]
fn opting_into_hardlinks_shares_one_inode_per_blob_across_worktrees() {
    let fx = Fixture::heavy_repo(60);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let base = tempfile::tempdir().unwrap();
    let store = base.path().join("store");
    let parent = fx.repo.parent().unwrap();
    let wt = |name: &str| wt_hardlinked(&fx, name, &store);

    let first = wt("one");
    assert!(
        first.status.success(),
        "first create failed: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    let objects_after_first = object_count(&store);

    let second = wt("two");
    assert!(
        second.status.success(),
        "second create failed: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    // Dedupe survives linking: no new content entered the store.
    assert_eq!(object_count(&store), objects_after_first);

    let one = hydrated_file(&parent.join("origin-one"));
    let two = hydrated_file(&parent.join("origin-two"));
    let m_one = fs::metadata(&one).unwrap();
    let m_two = fs::metadata(&two).unwrap();
    assert_eq!(m_one.ino(), m_two.ino(), "worktrees must share one inode");
    assert!(m_one.nlink() >= 2, "inode must be linked from both trees");
}

#[test]
fn opted_in_hardlinks_refuse_in_place_rewrites_and_protect_siblings() {
    if running_as_root() {
        eprintln!("skipping: root bypasses permission-based protection");
        return;
    }
    let fx = Fixture::heavy_repo(60);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let base = tempfile::tempdir().unwrap();
    let store = base.path().join("store");
    let parent = fx.repo.parent().unwrap();
    let wt = |name: &str| wt_hardlinked(&fx, name, &store);
    assert!(wt("one").status.success(), "create one failed");
    assert!(wt("two").status.success(), "create two failed");

    let two = parent.join("origin-two");
    let baseline = snapshot(&two.join("heavy"));
    let target = hydrated_file(&parent.join("origin-one"));

    // The package-manager rewrite that used to corrupt everything.
    let Err(err) = fs::write(&target, b"poisoned by in-place rewrite\n") else {
        panic!("in-place rewrite must be refused");
    };
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);

    assert_snapshot_unchanged(&baseline, &two.join("heavy"));

    // The store is untouched too: a fresh tree hydrates clean bytes.
    assert!(wt("three").status.success(), "create three failed");
    let three = snapshot(&parent.join("origin-three").join("heavy"));
    assert_eq!(baseline, three, "store was poisoned through the link");
}

#[test]
fn package_manager_rewrite_patterns_stay_isolated_across_worktrees() {
    if running_as_root() {
        eprintln!("skipping: root bypasses permission-based protection");
        return;
    }
    let fx = Fixture::heavy_repo(60);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let base = tempfile::tempdir().unwrap();
    let store = base.path().join("store");
    let parent = fx.repo.parent().unwrap();
    let wt = |name: &str| wt_hardlinked(&fx, name, &store);
    assert!(wt("one").status.success(), "create one failed");
    assert!(wt("two").status.success(), "create two failed");

    let one = parent.join("origin-one");
    let two = parent.join("origin-two");
    let sibling_baseline = snapshot(&two.join("heavy"));

    let target = hydrated_file(&one);
    let nested = target.parent().unwrap();

    // Pattern 1: truncate-and-write in place (npm's old habit).
    let Err(err) = fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(&target)
    else {
        panic!("truncate-in-place must be refused");
    };
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
    assert_snapshot_unchanged(&sibling_baseline, &two.join("heavy"));

    // Pattern 2: append in place.
    let Err(err) = fs::OpenOptions::new().append(true).open(&target) else {
        panic!("append-in-place must be refused");
    };
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
    assert_snapshot_unchanged(&sibling_baseline, &two.join("heavy"));

    // Pattern 3: atomic rename-over — the well-behaved pattern. It
    // succeeds and gets a private writable copy.
    let tmp = nested.join("file-0.txt.tmp");
    fs::write(&tmp, b"rewritten via rename\n").unwrap();
    fs::rename(&tmp, &target).expect("rename-over must succeed");
    assert_eq!(
        fs::read(&target).unwrap(),
        b"rewritten via rename\n",
        "rename-over result not visible to writer"
    );
    fs::write(&target, b"rewritten again in place\n")
        .expect("after breaking the share, the file is private and writable");
    assert_snapshot_unchanged(&sibling_baseline, &two.join("heavy"));

    // Pattern 4: delete and recreate.
    fs::remove_file(&target).unwrap();
    fs::write(&target, b"recreated from scratch\n").unwrap();
    assert_snapshot_unchanged(&sibling_baseline, &two.join("heavy"));

    // The store never saw any of it.
    assert!(wt("three").status.success(), "create three failed");
    let three = snapshot(&parent.join("origin-three").join("heavy"));
    assert_eq!(sibling_baseline, three, "store diverged from its trees");
}

#[test]
fn disabling_hardlinks_falls_back_to_byte_copies_with_a_message() {
    let fx = Fixture::heavy_repo(60);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let base = tempfile::tempdir().unwrap();
    let store = base.path().join("store");

    let out = Command::new(env!("CARGO_BIN_EXE_wt"))
        .args(["create", "copied"])
        .env("WT_STORE", store)
        .env("WT_NO_HARDLINK", "1")
        .current_dir(&fx.repo)
        .output()
        .expect("run wt binary");
    assert!(
        out.status.success(),
        "create failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("hardlink mode off"),
        "disabling hardlinks must say so clearly:\n{stdout}"
    );

    // Byte copies are private: own inode, writable, correct bytes.
    let copied = hydrated_file(&fx.repo.parent().unwrap().join("origin-copied"));
    let meta = fs::metadata(&copied).unwrap();
    assert_eq!(meta.nlink(), 1, "byte copy must not share an inode");
    assert!(
        !meta.permissions().readonly(),
        "a private copy may stay writable"
    );
}
