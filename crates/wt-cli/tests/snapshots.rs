//! Whole-directory snapshot hydration, end to end (fast-hydration
//! ticket 08).
//!
//! Every test runs the real `wt` binary against a real temporary git
//! repository with `WT_SNAPSHOTS=1` and an isolated `WT_STORE`, then
//! asserts on output and disk state. Plan coverage:
//!
//! - miss builds + publishes, second create hits (one clone)
//! - hits produce private, writable, normal-mode inodes
//! - `WT_VERIFY=1` bypasses hits and hashes every blob
//! - concurrent identical creates: loser consumes the winner
//! - evicted snapshot referenced elsewhere: next create rebuilds
//! - fifo under the gate fails loudly before placement; gate off
//!   keeps today's skip behavior
//! - non-empty destination falls back to the per-file merge
//! - GC marks through published manifests, never through debris

mod common;

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread;
use std::time::Duration;

use common::list_files;

/// A throwaway git repo whose `heavy/` directory exercises every
/// manifest kind: nested regular files, an executable, an explicit
/// empty directory, and a symlink.
struct RichFixture {
    repo: PathBuf,
    _base: tempfile::TempDir,
}

impl RichFixture {
    fn new() -> RichFixture {
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
        // Empty directories must survive hydration explicitly.
        fs::create_dir(heavy.join("empty")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("../exec.sh", heavy.join("bin-link")).unwrap();

        fs::write(repo.join(".gitignore"), "heavy/\n").unwrap();
        fs::write(repo.join(".wtinclude"), "heavy/\n").unwrap();
        fs::write(repo.join("src.txt"), "tracked source\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "--quiet", "-m", "init"]);

        RichFixture {
            repo,
            _base: base,
        }
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
            .args(["--dir", &self.worktree_path(worktree_name).to_string_lossy()])
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
    let status = Command::new("git").args(args).current_dir(dir).status().expect("run git");
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

/// The whole hydrated tree matches the source, byte for byte, path
/// for path — including symlinks and the empty directory.
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
            assert!(md.file_type().is_symlink(), "{} must stay a symlink", rel.display());
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
        }
    }
    // The explicit empty directory must exist.
    assert!(
        hydrated_heavy.join("empty").is_dir(),
        "empty directory vanished during hydration"
    );
}

const SNAPSHOTS_ON: &[(&str, &str)] = &[("WT_SNAPSHOTS", "1")];

#[test]
fn miss_builds_publishes_and_second_create_hits_with_private_files() {
    let fx = RichFixture::new();
    let source = fx.repo.join("heavy");

    // MISS: builds and publishes.
    let out = fx.wt(&["create", "one"], SNAPSHOTS_ON, "origin-one");
    assert_created(&out);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("via snapshot"),
        "first create should report the snapshot path: {stdout}"
    );

    // Exactly one published snapshot directory, valid per the shared
    // checker.
    let snapshots_dir = fx.store_path().join("snapshots");
    let published: Vec<String> = fs::read_dir(&snapshots_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n != "tmp")
        .collect();
    assert_eq!(published.len(), 1, "exactly one snapshot expected");

    // The mirror carries ONE snapshot record and ZERO child blob
    // records (the manifest marks those).
    let mirrors = wt_store::read_mirrors(&fx.store_path());
    assert_eq!(mirrors.len(), 1);
    let mirror = mirrors[0].mirror.as_ref().expect("valid mirror");
    assert_eq!(mirror.snapshots.len(), 1, "one snapshot record");
    assert!(
        mirror.files.is_empty(),
        "snapshot hydration writes no per-file blob records"
    );

    // HIT: second create reuses the same published snapshot.
    let snap_dir = snapshots_dir.join(&published[0]);
    let born = fs::metadata(&snap_dir).unwrap().created().unwrap();
    let out = fx.wt(
        &["create", "two"],
        &[("WT_SNAPSHOTS", "1"), ("WT_TIMING", "1")],
        "origin-two",
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

    // Both worktrees carry private, writable, normal-mode inodes —
    // clonefile gives fresh inodes, never links back into the store.
    for name in ["origin-one", "origin-two"] {
        let heavy_root = fx.worktree_path(name).join("heavy");
        assert_tree_matches_source(&source, &heavy_root);

        let plain = heavy_root.join("pkg00/nested/file-0.txt");
        let meta = fs::metadata(&plain).unwrap();
        assert_eq!(meta.nlink(), 1, "{} must own a private inode", name);
        assert_ne!(meta.mode() & 0o777, 0o444);
        assert_eq!(meta.mode() & 0o200, 0o200, "{name} file must be owner-writable");

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
    // Private means private: the two trees share no inode.
    let a = fs::metadata(fx.worktree_path("origin-one").join("heavy/exec.sh"))
        .unwrap()
        .ino();
    let b = fs::metadata(fx.worktree_path("origin-two").join("heavy/exec.sh"))
        .unwrap()
        .ino();
    assert_ne!(a, b, "cloned trees must not share inodes with each other");
}

#[test]
fn gate_off_changes_nothing_at_all() {
    let fx = RichFixture::new();
    let out = fx.wt(&["create", "one"], &[], "origin-one");
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

#[test]
fn wt_verify_bypasses_hits_and_hashes_every_blob() {
    let fx = RichFixture::new();

    // First create publishes a snapshot (normal trust).
    assert_created(&fx.wt(&["create", "one"], SNAPSHOTS_ON, "origin-one"));

    // Tamper with a blob while preserving size AND mtime: the
    // verified-ledger trust path cannot see this.
    let mut tampered = None;
    let objects = fx.store_path().join("objects");
    for shard in fs::read_dir(&objects).unwrap() {
        for blob in fs::read_dir(shard.unwrap().path()).unwrap() {
            let path = blob.unwrap().path();
            let meta = fs::metadata(&path).unwrap();
            if meta.len() == b"file zero\n".len() as u64 {
                let mtime = meta.modified().unwrap();
                fs::write(&path, b"FILE ZERO").unwrap(); // same length
                let f = fs::OpenOptions::new().write(true).open(&path).unwrap();
                f.set_times(std::fs::FileTimes::new().set_modified(mtime)).unwrap();
                tampered = Some(path);
            }
        }
    }
    assert!(tampered.is_some(), "fixture must contain the target blob");

    // A normal hit trusts verified-at-publish (the documented model):
    // it must succeed even though the underlying blob rotted.
    let out = fx.wt(&["create", "two"], SNAPSHOTS_ON, "origin-two");
    assert_created(&out);

    // But WT_VERIFY=1 bypasses hits entirely: rebuild hashes every
    // blob and must fail LOUDLY rather than land bad bytes.
    let out = fx.wt(
        &["create", "three"],
        &[("WT_SNAPSHOTS", "1"), ("WT_VERIFY", "1")],
        "origin-three",
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

#[test]
fn concurrent_identical_creates_one_publish_wins_loser_consumes_winner() {
    let fx = RichFixture::new();

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
                Command::new(env!("CARGO_BIN_EXE_wt"))
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
            Command::new(env!("CARGO_BIN_EXE_wt"))
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
        .filter(|n| n != "tmp")
        .collect();
    assert_eq!(published.len(), 1, "identical content must converge on one snapshot");

    assert_tree_matches_source(
        &fx.repo.join("heavy"),
        &fx.worktree_path("race-one").join("heavy"),
    );
    assert_tree_matches_source(
        &fx.repo.join("heavy"),
        &fx.worktree_path("race-two").join("heavy"),
    );
}

#[test]
fn evicted_snapshot_referenced_by_mirror_rebuilds_on_next_create() {
    let fx = RichFixture::new();

    assert_created(&fx.wt(&["create", "one"], SNAPSHOTS_ON, "origin-one"));

    // Simulate eviction: the snapshot directory disappears while a
    // live mirror still references it.
    let snapshots_dir = fx.store_path().join("snapshots");
    let hash = fs::read_dir(&snapshots_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .find(|n| *n != "tmp")
        .expect("published snapshot");
    fs::remove_dir_all(snapshots_dir.join(&hash)).unwrap();

    // GC must treat the reference as unresolvable, not fatal: nothing
    // marked through it, nothing crashed.
    let mirrors = wt_store::read_mirrors(&fx.store_path());
    let mirror = mirrors[0].mirror.as_ref().unwrap();
    assert_eq!(mirror.snapshots.len(), 1);
    let store = wt_store::DiskStore::open(fx.store_path()).unwrap();
    let report = store
        .compute_marks(std::time::SystemTime::now(), Duration::from_secs(900))
        .unwrap();
    assert_eq!(report.unresolved_snapshots, 1);
    assert!(report.marked.is_empty(), "no blobs may be marked through a missing snapshot");

    // The next create treats it as a miss and REBUILDS rather than
    // failing or corrupting anything.
    let out = fx.wt(&["create", "two"], SNAPSHOTS_ON, "origin-two");
    assert_created(&out);
    assert_tree_matches_source(
        &fx.repo.join("heavy"),
        &fx.worktree_path("origin-two").join("heavy"),
    );
    assert!(snapshots_dir.join(&hash).exists(), "snapshot rebuilt at the same address");
}

#[test]
fn fifo_fails_loudly_under_gate_and_is_skipped_without_it() {
    let fx = RichFixture::new();
    let fifo = fx.repo.join("heavy/pkg00/fifo");
    unsafe {
        assert_eq!(libc::mkfifo(c_fifo(&fifo), 0o644), 0, "mkfifo failed");
    }

    // Gate ON: loud error BEFORE any placement happens.
    let out = fx.wt(&["create", "one"], SNAPSHOTS_ON, "origin-one");
    assert!(!out.status.success(), "fifos must fail loudly under the gate");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not a regular file"),
        "error must explain the rejection: {stderr}"
    );
    assert!(
        !fx.worktree_path("origin-one").join("heavy").exists(),
        "nothing may be placed when ingest rejects the tree"
    );

    // Gate OFF: long-standing behavior — skipped silently, create
    // succeeds exactly as it always did.
    let out = fx.wt(&["create", "two"], &[], "origin-two");
    assert_created(&out);
    assert!(
        fx.worktree_path("origin-two")
            .join("heavy/pkg00/nested/file-0.txt")
            .exists()
    );
}

unsafe fn c_fifo(path: &Path) -> *const std::ffi::c_char {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    c.into_raw()
}

#[test]
fn invalid_winner_debris_falls_back_to_per_file_ladder() {
    // Pre-seed INVALID debris on the exact snapshot address the create
    // would publish to. The builder must neither overwrite nor trust
    // it: it falls back to the existing per-file ladder and keeps the
    // semantics of a gate-off run.
    let fx = RichFixture::new();

    // Derive the manifest hash by ingesting exactly what the CLI will
    // ingest: same bytes -> same blob ids -> same manifest hash.
    let store = wt_store::DiskStore::open(fx.store_path()).unwrap();
    use wt_store::{Manifest, SnapshotEntry, Store as _};
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

    let out = fx.wt(&["create", "one"], SNAPSHOTS_ON, "origin-one");
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
        "garbage\n",
        "debris is left untouched"
    );

    // The per-file ladder preserves gate-off semantics for everything
    // it can place (symlinks are skipped there — long-standing v1
    // behavior, recorded in ADR-0005).
    let heavy = fx.worktree_path("origin-one").join("heavy");
    assert_eq!(
        fs::read_to_string(heavy.join("pkg00/nested/file-0.txt")).unwrap(),
        "file zero\n"
    );
    assert!(heavy.join("empty").is_dir());
}

#[test]
fn gc_marks_through_valid_manifests_only() {
    // Store-level check of the mark rule: a live mirror naming a
    // VALID published snapshot marks every file entry's blob; once
    // the snapshot is gone, nothing is marked through it.
    let base = tempfile::tempdir().unwrap();
    let mut store = wt_store::DiskStore::open(base.path().join("store")).unwrap();
    use wt_store::{Manifest, SnapshotEntry, Store as _};

    let b1 = store.put(b"alpha").unwrap();
    let b2 = store.put(b"beta").unwrap();
    let entries = vec![
        SnapshotEntry::dir("d"),
        SnapshotEntry::file("d/a", b1, 0o644),
        SnapshotEntry::file("d/b", b2, 0o644),
    ];
    let m = Manifest::new(entries).unwrap();
    assert_eq!(
        store.publish_snapshot(m.entries.clone(), false).unwrap(),
        Ok(wt_store::PublishOutcome::Published)
    );

    let wt_dir = base.path().join("wt");
    fs::create_dir_all(&wt_dir).unwrap();
    let gitdir = base.path().join("wt.git");
    fs::create_dir_all(&gitdir).unwrap();
    fs::write(gitdir.join("wt-hydrated.tsv"), "").unwrap();
    store
        .publish_worktree_mirror(&wt_dir, &gitdir, std::iter::empty(), [&m.hash])
        .unwrap();

    let now = std::time::SystemTime::now();
    let grace = Duration::from_secs(900);
    let report = store.compute_marks(now, grace).unwrap();
    assert_eq!(
        report.referenced_snapshots,
        [m.hash].into_iter().collect::<std::collections::BTreeSet<_>>()
    );
    assert!(
        report.marked.contains(&b1) && report.marked.contains(&b2),
        "every file entry's blob must be marked through the manifest"
    );

    // Evict the snapshot: same mirror now marks through NOTHING.
    fs::remove_dir_all(store.snapshot_path(&m.hash)).unwrap();
    let report = store.compute_marks(now, grace).unwrap();
    assert_eq!(report.unresolved_snapshots, 1);
    assert!(report.marked.is_empty());
    store.flush().unwrap();
}

#[test]
fn snapshot_covers_everything_the_ladder_places_plus_symlinks() {
    // The per-file ladder skips symlinks (long-standing behavior);
    // the snapshot path represents them faithfully. So the snapshot
    // tree must be exactly the ladder's tree PLUS the symlink.
    let fx = RichFixture::new();
    assert_created(&fx.wt(&["create", "plain"], &[], "origin-plain"));
    assert_created(&fx.wt(&["create", "snap"], SNAPSHOTS_ON, "origin-snap"));

    let rel_prefix = |root: &Path| {
        list_files(root)
            .iter()
            .map(|p| p.strip_prefix(root).unwrap().to_path_buf())
            .collect::<Vec<_>>()
    };
    let plain = rel_prefix(&fx.worktree_path("origin-plain").join("heavy"));
    let snap = rel_prefix(&fx.worktree_path("origin-snap").join("heavy"));

    // Every path the ladder places, the snapshot places too.
    for p in &plain {
        assert!(
            snap.contains(p),
            "snapshot must place {p:?} like the ladder does"
        );
    }
    // And only the snapshot carries the symlink.
    assert!(
        !plain.contains(&PathBuf::from("bin-link")),
        "ladder skips symlinks (pre-existing behavior)"
    );
    assert!(snap.contains(&PathBuf::from("bin-link")));
}
