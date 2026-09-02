//! Integration tests for storage, materialization, caching, hardlinks, CoW,
//! branch stacking, toolchain relocation, copy acceleration, and verification.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};

use crate::common::{Fixture, list_files};
use wt_copy::{BackendKind, SourcePolicy, candidates, select_backend};
use wt_store::{DiskStore, FsCapabilities, probe_fs};

// =========================================================================
// CoW Materialization (from cow_materialization.rs)
// =========================================================================

fn file_snapshot(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
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

fn wt_cow_default(fx: &Fixture, args: &[&str], store: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_flashwt"))
        .args(args)
        .env("WT_STORE", store)
        .env_remove("WT_HARDLINK")
        .env_remove("WT_NO_HARDLINK")
        .current_dir(&fx.repo)
        .output()
        .expect("run wt binary")
}

fn assert_created_ok(out: &std::process::Output) {
    assert!(
        out.status.success(),
        "create failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn get_hydrated_file(worktree: &Path) -> PathBuf {
    let f = worktree.join("heavy/pkg00/nested/file-0.txt");
    assert!(f.exists(), "expected hydrated file at {}", f.display());
    f
}

#[cfg(unix)]
#[test]
fn default_hydration_produces_private_writable_files() {
    let fx = Fixture::heavy_repo(60);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let base = tempfile::tempdir().unwrap();
    let store = base.path().join("store");
    let parent = fx.repo.parent().unwrap();

    for name in ["one", "two"] {
        let out = wt_cow_default(&fx, &["create", name], &store);
        assert_created_ok(&out);
    }

    let one = get_hydrated_file(&parent.join("origin-one"));
    let two = get_hydrated_file(&parent.join("origin-two"));
    for path in [&one, &two] {
        let meta = fs::metadata(path).unwrap();
        assert_eq!(
            meta.nlink(),
            1,
            "{} must own a private inode",
            path.display()
        );
        assert_eq!(
            meta.mode() & 0o200,
            0o200,
            "{} must be owner-writable",
            path.display()
        );
        assert!(
            !meta.permissions().readonly(),
            "{} must not be read-only",
            path.display()
        );
    }
    assert_ne!(
        fs::metadata(&one).unwrap().ino(),
        fs::metadata(&two).unwrap().ino(),
        "default materialization must not link shared inodes"
    );
}

#[test]
fn in_place_rewrite_succeeds_and_stays_private_to_its_worktree() {
    let fx = Fixture::heavy_repo(60);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let base = tempfile::tempdir().unwrap();
    let store = base.path().join("store");
    let parent = fx.repo.parent().unwrap();

    for name in ["one", "two"] {
        let out = wt_cow_default(&fx, &["create", name], &store);
        assert_created_ok(&out);
    }

    let sibling_baseline = file_snapshot(&parent.join("origin-two").join("heavy"));
    let store_baseline = file_snapshot(&store);

    let target = get_hydrated_file(&parent.join("origin-one"));

    fs::write(&target, b"rewritten in place\n")
        .expect("in-place rewrite must succeed on a CoW-hydrated file");
    assert_eq!(fs::read(&target).unwrap(), b"rewritten in place\n");

    assert_eq!(
        file_snapshot(&parent.join("origin-two").join("heavy")),
        sibling_baseline,
        "sibling worktree saw the rewrite"
    );
    assert_eq!(
        file_snapshot(&store),
        store_baseline,
        "the store blob was poisoned through materialization"
    );

    let out = wt_cow_default(&fx, &["create", "three"], &store);
    assert_created_ok(&out);
    let three = file_snapshot(&parent.join("origin-three").join("heavy"));
    assert_eq!(sibling_baseline, three, "store diverged from its trees");
}

#[test]
#[cfg(target_os = "macos")]
fn before_first_write_hydrated_files_share_physical_blocks_with_the_blob() {
    let base = tempfile::tempdir().unwrap();
    let repo = base.path().join("origin");
    fs::create_dir_all(repo.join("heavy/pkg00/nested")).unwrap();

    let git = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(&repo)
            .status()
            .expect("run git");
        assert!(status.success(), "git {args:?} failed");
    };
    git(&["init", "--quiet"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["config", "user.name", "Test"]);

    let file_len: usize = 16 << 20;
    let big = vec![0x5au8; file_len];
    for i in 0..4 {
        fs::write(repo.join("heavy").join(format!("big-{i}.bin")), &big).unwrap();
    }
    fs::write(repo.join(".gitignore"), "heavy/\n").unwrap();
    fs::write(repo.join("src.txt"), "tracked source\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "--quiet", "-m", "init"]);

    fs::write(repo.join(".wtinclude"), "heavy/\n").unwrap();

    let store = base.path().join("store");
    let free_bytes = || {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(base.path().as_os_str().as_bytes()).unwrap();
        let mut st: libc::statfs = unsafe { std::mem::zeroed() };
        let rc = unsafe { libc::statfs(c.as_ptr(), &mut st) };
        assert_eq!(rc, 0, "statfs failed");
        (st.f_bsize as u64) * (st.f_bavail as u64)
    };

    let before = free_bytes();
    let out = Command::new(env!("CARGO_BIN_EXE_flashwt"))
        .args(["create", "one"])
        .env("WT_STORE", &store)
        .env_remove("WT_HARDLINK")
        .env_remove("WT_NO_HARDLINK")
        .current_dir(&repo)
        .output()
        .expect("run wt binary");
    assert_created_ok(&out);
    let consumed = before.saturating_sub(free_bytes());

    let logical = (file_len * 4) as u64;
    assert!(
        consumed < logical / 2,
        "hydrating must share blocks with the store before first write: \
         {consumed} new bytes for {logical} logical bytes"
    );

    let hydrated = repo.parent().unwrap().join("origin-one/heavy/big-0.bin");
    assert_eq!(fs::read(&hydrated).unwrap(), big);
    fs::write(&hydrated, b"diverged\n").unwrap();
    for entry in fs::read_dir(store.join("objects")).unwrap().flatten() {
        let shard = entry.path();
        for blob in fs::read_dir(&shard).unwrap().flatten() {
            let bytes = fs::read(blob.path()).unwrap();
            assert_eq!(
                bytes, big,
                "store blob was mutated by writing to a hydrated file"
            );
        }
    }
}

#[test]
fn cow_unavailable_falls_back_to_byte_copies_without_user_visible_failure() {
    #[cfg(target_os = "macos")]
    {
        struct HfsRamdisk {
            mount: PathBuf,
        }
        impl HfsRamdisk {
            fn attach() -> Option<HfsRamdisk> {
                let attached = Command::new("hdiutil")
                    .args(["attach", "-nomount", "ram://8192"])
                    .output()
                    .ok()?;
                if !attached.status.success() {
                    return None;
                }
                let device = String::from_utf8_lossy(&attached.stdout).trim().to_owned();
                let detach = |mount: Option<&str>| {
                    match mount {
                        Some(name) => {
                            let _ = Command::new("diskutil").arg("eject").arg(name).output();
                        }
                        None => {
                            let _ = Command::new("hdiutil").arg("detach").arg(&device).output();
                        }
                    };
                };
                let formatted = Command::new("diskutil")
                    .args(["eraseVolume", "HFS+", "wt-cow-test"])
                    .arg(&device)
                    .output()
                    .ok()?;
                if !formatted.status.success() {
                    detach(None);
                    return None;
                }
                let mount = PathBuf::from("/Volumes/wt-cow-test");
                if !mount.is_dir() {
                    detach(Some("wt-cow-test"));
                    return None;
                }
                Some(HfsRamdisk { mount })
            }
        }
        impl Drop for HfsRamdisk {
            fn drop(&mut self) {
                let _ = Command::new("diskutil")
                    .args(["eject", "wt-cow-test"])
                    .output();
            }
        }

        let Some(volume) = HfsRamdisk::attach() else {
            eprintln!("skipping: could not create an HFS+ ram disk here");
            return;
        };

        let fx = Fixture::heavy_repo(20);
        fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

        let base = tempfile::tempdir().unwrap();
        let store = base.path().join("store");
        let dest = volume.mount.join("origin-ramdisk");

        let out = Command::new(env!("CARGO_BIN_EXE_flashwt"))
            .args(["create", "ramdisk", "--dir"])
            .arg(&dest)
            .env("WT_STORE", &store)
            .current_dir(&fx.repo)
            .output()
            .expect("run wt binary");
        assert_created_ok(&out);

        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("copy-on-write unavailable"),
            "refused clones must be reported honestly:\n{stdout}"
        );
        assert!(
            stdout.contains("byte copies for 20 of 20 file(s)"),
            "every file should have fallen back to a byte copy:\n{stdout}"
        );

        let hydrated = get_hydrated_file(&dest);
        let meta = fs::metadata(&hydrated).unwrap();
        assert_eq!(meta.nlink(), 1, "byte copy owns its inode");
        assert!(!meta.permissions().readonly(), "byte copy is writable");
    }
}

// =========================================================================
// Hardlink Safety (from hardlink_safety.rs)
// =========================================================================

fn assert_snapshot_unchanged(before: &BTreeMap<PathBuf, Vec<u8>>, dir: &Path) {
    assert_eq!(&file_snapshot(dir), before, "{} changed", dir.display());
}

fn object_count(store: &Path) -> usize {
    list_files(&store.join("objects")).len()
}

fn running_as_root() -> bool {
    Command::new("id")
        .arg("-u")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
        .unwrap_or(false)
}

fn wt_hardlinked(fx: &Fixture, name: &str, store: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_flashwt"))
        .args(["create", name])
        .env("WT_STORE", store)
        .env("WT_HARDLINK", "1")
        .env("WT_SNAPSHOTS", "0")
        .env("WT_SNAPSHOTS_V2", "0")
        .current_dir(&fx.repo)
        .output()
        .expect("run wt binary")
}

#[cfg(unix)]
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
    assert_eq!(object_count(&store), objects_after_first);

    let one = get_hydrated_file(&parent.join("origin-one"));
    let two = get_hydrated_file(&parent.join("origin-two"));
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
    let baseline = file_snapshot(&two.join("heavy"));
    let target = get_hydrated_file(&parent.join("origin-one"));

    let Err(err) = fs::write(&target, b"poisoned by in-place rewrite\n") else {
        panic!("in-place rewrite must be refused");
    };
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
    assert_snapshot_unchanged(&baseline, &two.join("heavy"));

    assert!(wt("three").status.success(), "create three failed");
    let three = file_snapshot(&parent.join("origin-three").join("heavy"));
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
    let sibling_baseline = file_snapshot(&two.join("heavy"));

    let target = get_hydrated_file(&one);
    let nested = target.parent().unwrap();

    let Err(err) = fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(&target)
    else {
        panic!("truncate-in-place must be refused");
    };
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
    assert_snapshot_unchanged(&sibling_baseline, &two.join("heavy"));

    let Err(err) = fs::OpenOptions::new().append(true).open(&target) else {
        panic!("append-in-place must be refused");
    };
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
    assert_snapshot_unchanged(&sibling_baseline, &two.join("heavy"));

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

    fs::remove_file(&target).unwrap();
    fs::write(&target, b"recreated from scratch\n").unwrap();
    assert_snapshot_unchanged(&sibling_baseline, &two.join("heavy"));

    assert!(wt("three").status.success(), "create three failed");
    let three = file_snapshot(&parent.join("origin-three").join("heavy"));
    assert_eq!(sibling_baseline, three, "store diverged from its trees");
}

#[test]
fn disabling_hardlinks_falls_back_to_byte_copies_with_a_message() {
    let fx = Fixture::heavy_repo(60);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let base = tempfile::tempdir().unwrap();
    let store = base.path().join("store");

    let out = Command::new(env!("CARGO_BIN_EXE_flashwt"))
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

    let copied = get_hydrated_file(&fx.repo.parent().unwrap().join("origin-copied"));
    let meta = fs::metadata(&copied).unwrap();
    #[cfg(unix)]
    assert_eq!(meta.nlink(), 1, "byte copy must not share an inode");
    assert!(
        !meta.permissions().readonly(),
        "a private copy may stay writable"
    );
}

// =========================================================================
// Store Flow & Deduplication (from store_flow.rs)
// =========================================================================

fn calc_store_footprint(store: &Path) -> (usize, usize) {
    let mut files = 0usize;
    let mut bytes = 0usize;
    let mut stack = vec![store.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).expect("read store dir") {
            let p = entry.expect("store entry").path();
            if p.is_dir() {
                if p.file_name() == Some(std::ffi::OsStr::new("worktrees"))
                    || p.file_name() == Some(std::ffi::OsStr::new("mirrors"))
                    || p.file_name() == Some(std::ffi::OsStr::new("snapshots"))
                {
                    continue;
                }
                stack.push(p);
            } else if p.file_name() != Some(std::ffi::OsStr::new("ingest-cache.tsv"))
                && p.file_name() != Some(std::ffi::OsStr::new("verified.tsv"))
                && p.file_name() != Some(std::ffi::OsStr::new("gc-mode"))
            {
                files += 1;
                bytes += fs::metadata(&p).expect("store file metadata").len() as usize;
            }
        }
    }
    (files, bytes)
}

fn assert_same_file_tree(src: &Path, dest: &Path) {
    let rel = |base: &Path, p: &Path| p.strip_prefix(base).unwrap().to_path_buf();
    let a = list_files(src);
    let b = list_files(dest);
    assert_eq!(
        a.iter().map(|p| rel(src, p)).collect::<Vec<_>>(),
        b.iter().map(|p| rel(dest, p)).collect::<Vec<_>>(),
        "file sets differ"
    );
    for (pa, pb) in a.iter().zip(b.iter()) {
        assert_eq!(fs::read(pa).unwrap(), fs::read(pb).unwrap());
    }
}

#[test]
fn second_worktree_adds_no_new_store_content() {
    let fx = Fixture::heavy_repo(300);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let base = tempfile::tempdir().expect("tempdir");
    let store = base.path().join("store");
    let wt = |name: &str| fx.wt_with_store(&["create", name], &store);

    let first = wt("one");
    assert!(
        first.status.success(),
        "first create failed: {}",
        String::from_utf8_lossy(&first.stderr)
    );

    let (files_after_first, bytes_after_first) = calc_store_footprint(&store);
    assert!(files_after_first > 0, "store stayed empty after ingest");

    let second = wt("two");
    assert!(
        second.status.success(),
        "second create failed: {}",
        String::from_utf8_lossy(&second.stderr)
    );

    let (files_after_second, bytes_after_second) = calc_store_footprint(&store);
    assert_eq!(
        files_after_first, files_after_second,
        "second worktree added new store objects"
    );
    assert_eq!(
        bytes_after_first, bytes_after_second,
        "second worktree grew the store's footprint"
    );

    let dest_two = fx.repo.parent().unwrap().join("origin-two");
    assert_same_file_tree(&fx.repo.join("heavy"), &dest_two.join("heavy"));
    let dest_one = fx.repo.parent().unwrap().join("origin-one");
    assert_same_file_tree(&fx.repo.join("heavy"), &dest_one.join("heavy"));

    for name in ["one", "two"] {
        let wt_root = fx.repo.parent().unwrap().join(format!("origin-{name}"));
        let pointer = fs::read_to_string(wt_root.join(".git")).expect("worktree .git pointer");
        let git_dir = pointer
            .trim()
            .strip_prefix("gitdir: ")
            .expect(".git points at a gitdir");
        let ledger = Path::new(git_dir).join("wt-hydrated.tsv");
        let text = fs::read_to_string(&ledger)
            .unwrap_or_else(|e| panic!("missing ledger {}: {e}", ledger.display()));
        assert!(text.lines().count() >= 300, "ledger incomplete for {name}");
    }

    let stdout = String::from_utf8_lossy(&second.stdout);
    assert!(
        stdout.contains("the store") || stdout.contains("via store"),
        "second create must say it flowed through the store:\n{stdout}"
    );
}

#[test]
fn hash_mismatch_during_materialize_fails_loudly() {
    let fx = Fixture::heavy_repo(20);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let base = tempfile::tempdir().expect("tempdir");
    let store = base.path().join("store");

    let non_snap: &[(&str, &str)] = &[("WT_SNAPSHOTS", "0"), ("WT_SNAPSHOTS_V2", "0")];
    let first = fx.wt_with_store_env(&["create", "good"], &store, non_snap);
    assert!(first.status.success());

    let objects = store.join("objects");
    let blob = list_files(&objects)
        .into_iter()
        .find(|p| p.is_file())
        .expect("store has objects");
    let mut bytes = fs::read(&blob).unwrap();
    bytes[0] ^= 0xff;
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(&blob).unwrap().permissions();
        perms.set_mode(perms.mode() | 0o200);
        fs::set_permissions(&blob, perms).unwrap();
    }
    fs::write(&blob, &bytes).unwrap();

    let second = fx.wt_with_store_env(&["create", "bad"], &store, non_snap);
    assert!(
        !second.status.success(),
        "corrupt store content must fail the create"
    );
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(
        stderr.contains("hash verification"),
        "failure must name hash verification loudly:\n{stderr}"
    );
}

#[cfg(unix)]
#[test]
fn hydrated_trees_preserve_symlinks_and_permission_bits() {
    let fx = Fixture::heavy_repo(5);
    let heavy = fx.repo.join("heavy");

    let script = heavy.join("script.sh");
    fs::write(&script, "#!/bin/sh\necho hi\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    let plain = heavy.join("plain.txt");
    fs::write(&plain, "plain bytes\n").unwrap();
    std::os::unix::fs::symlink("pkg00/nested/file-0.txt", heavy.join("link-to-file")).unwrap();
    std::os::unix::fs::symlink("../missing/target", heavy.join("dangling-link")).unwrap();

    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let base = tempfile::tempdir().expect("tempdir");
    let store = base.path().join("store");
    for name in ["one", "two"] {
        let out = fx.wt_with_store(&["create", name], &store);
        assert!(
            out.status.success(),
            "create {name} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );

        let root = fx
            .repo
            .parent()
            .unwrap()
            .join(format!("origin-{name}"))
            .join("heavy");

        let link = root.join("link-to-file");
        let meta = fs::symlink_metadata(&link).expect("valid symlink must exist");
        assert!(meta.is_symlink(), "{name}: link-to-file is not a symlink");
        assert_eq!(
            fs::read_link(&link).unwrap(),
            Path::new("pkg00/nested/file-0.txt"),
            "{name}: symlink target changed"
        );

        let dangling = root.join("dangling-link");
        let meta = fs::symlink_metadata(&dangling).expect("dangling symlink must exist");
        assert!(meta.is_symlink(), "{name}: dangling link was materialized");
        assert_eq!(
            fs::read_link(&dangling).unwrap(),
            Path::new("../missing/target"),
            "{name}: dangling target changed"
        );
        assert!(!dangling.exists(), "{name}: dangling link grew a target");

        for (src, rel) in [(&script, "script.sh"), (&plain, "plain.txt")] {
            let dest = root.join(rel);
            let want = fs::metadata(src).unwrap().permissions().mode() & 0o777;
            let got = fs::metadata(&dest)
                .expect("file placed")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(got, want, "{name}: mode of {rel} changed");
        }
        assert_eq!(
            fs::read(root.join("plain.txt")).unwrap(),
            b"plain bytes\n",
            "{name}: content changed"
        );
    }
}

#[cfg(unix)]
#[test]
fn hardlink_mode_preserves_exec_bits_without_touching_the_shared_blob() {
    let fx = Fixture::heavy_repo(3);
    let heavy = fx.repo.join("heavy");
    let script = heavy.join("script.sh");
    fs::write(&script, "#!/bin/sh\necho hi\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let base = tempfile::tempdir().expect("tempdir");
    let store = base.path().join("store");
    let out = fx.wt_with_store_env(
        &["create", "one"],
        &store,
        &[("WT_HARDLINK", "1"), ("WT_SNAPSHOTS", "0")],
    );
    assert!(
        out.status.success(),
        "hardlink create failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let root = fx.repo.parent().unwrap().join("origin-one").join("heavy");

    let dest_script = root.join("script.sh");
    assert!(
        fs::metadata(&dest_script).unwrap().permissions().mode() & 0o111 == 0o111,
        "exec bit lost under hardlink mode"
    );
    assert_eq!(
        fs::metadata(&dest_script).unwrap().nlink(),
        1,
        "exec file must not stay hardlinked to the blob"
    );

    let plain = heavy.join("pkg00/nested/file-0.txt");
    let dest_plain = root.join(plain.strip_prefix(&heavy).unwrap());
    let meta = fs::metadata(&dest_plain).expect("non-exec file placed");
    assert!(meta.nlink() >= 2, "non-exec file should stay hardlinked");
    assert!(meta.permissions().readonly());

    let mut blobs = BTreeSet::new();
    collect_store_blobs(&store.join("objects"), &mut blobs);
    assert!(!blobs.is_empty());
    for blob in blobs {
        let mode = fs::metadata(&blob).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o333,
            0,
            "blob {:?} must stay non-writable and non-exec",
            blob
        );
    }
}

fn collect_store_blobs(dir: &Path, out: &mut BTreeSet<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read objects dir") {
        let p = entry.expect("objects entry").path();
        if p.is_dir() {
            collect_store_blobs(&p, out);
        } else {
            out.insert(p);
        }
    }
}

#[cfg(unix)]
#[test]
fn hydrated_trees_restore_directory_permission_bits() {
    let fx = Fixture::heavy_repo(6);
    let heavy = fx.repo.join("heavy");

    let locked = heavy.join("pkg00/nested");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o705)).unwrap();
    let private_pkg = heavy.join("pkg01");
    fs::set_permissions(&private_pkg, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let base = tempfile::tempdir().expect("tempdir");
    let store = base.path().join("store");
    for name in ["one", "two"] {
        let out = fx.wt_with_store_env(&["create", name], &store, &[("WT_SNAPSHOTS", "0")]);
        assert!(
            out.status.success(),
            "create {name} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );

        let root = fx.repo.parent().unwrap().join(format!("origin-{name}"));
        for (src, rel) in [
            (&heavy, "heavy"),
            (&locked, "heavy/pkg00/nested"),
            (&private_pkg, "heavy/pkg01"),
        ] {
            let want = fs::metadata(src).unwrap().permissions().mode() & 0o7777;
            let dest = root.join(rel);
            let got = fs::metadata(&dest)
                .unwrap_or_else(|e| panic!("directory {rel} missing: {e}"))
                .permissions()
                .mode()
                & 0o7777;
            assert_eq!(got, want, "{name}: directory mode of {rel} changed");
        }
    }
}

#[cfg(unix)]
#[test]
fn hardlink_exec_bit_repair_counts_as_copied_in_reporting() {
    let fx = Fixture::heavy_repo(3);
    let heavy = fx.repo.join("heavy");
    let script = heavy.join("script.sh");
    fs::write(&script, "#!/bin/sh\necho hi\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let base = tempfile::tempdir().expect("tempdir");
    let store = base.path().join("store");
    let out = fx.wt_with_store_env(
        &["create", "one"],
        &store,
        &[("WT_HARDLINK", "1"), ("WT_SNAPSHOTS", "0")],
    );
    assert!(
        out.status.success(),
        "hardlink create failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("hardlinks refused for 1 of 4 file(s)"),
        "the private-copy repair must be reported as one refused hardlink:\n{stdout}"
    );
}

// =========================================================================
// Cache Flow & Ingest Staleness (from cache_flow.rs)
// =========================================================================

fn assert_heavy_matches_source(src: &Path, dest_root: &Path) {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).expect("read heavy dir") {
            let p = entry.expect("entry").path();
            if p.is_dir() {
                walk(&p, out);
            } else {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    walk(src, &mut files);
    assert!(!files.is_empty());
    for f in files {
        let rel = f.strip_prefix(src).unwrap();
        let hydrated = dest_root.join("heavy").join(rel);
        assert_eq!(
            fs::read(&f).unwrap(),
            fs::read(&hydrated)
                .unwrap_or_else(|e| panic!("cannot read {}: {e}", hydrated.display())),
            "hydrated bytes differ for {}",
            rel.display()
        );
    }
}

#[test]
fn edited_source_lands_in_the_next_hydrated_tree() {
    let fx = Fixture::heavy_repo(8);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let base = tempfile::tempdir().expect("tempdir");
    let store = base.path().join("store");

    assert!(fx.wt_with_store(&["create", "one"], &store).status.success());

    let target = fx.repo.join("heavy/pkg00/nested/file-0.txt");
    fs::write(&target, "edited between creates\n").expect("edit source");
    let (files_before, _bytes_before) = calc_store_footprint(&store);

    assert!(fx.wt_with_store(&["create", "two"], &store).status.success());
    let (files_after, _bytes_after) = calc_store_footprint(&store);
    assert!(files_after > files_before);

    let parent = fx.repo.parent().unwrap();
    assert_heavy_matches_source(&fx.repo.join("heavy"), &parent.join("origin-two"));
    assert_eq!(
        fs::read(parent.join("origin-one/heavy/pkg00/nested/file-0.txt")).unwrap(),
        b"fake-heavy file 0 of 8\n"
    );
}

#[cfg(unix)]
#[test]
fn warm_create_reads_no_unchanged_file_bytes() {
    let fx = Fixture::heavy_repo(12);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let base = tempfile::tempdir().expect("tempdir");
    let store = base.path().join("store");

    assert!(fx.wt_with_store(&["create", "one"], &store).status.success());

    {
        let heavy = fx.repo.join("heavy");
        let mut stack = vec![heavy.clone()];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir).expect("read heavy") {
                let p = entry.expect("entry").path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    let mut perms = fs::metadata(&p).unwrap().permissions();
                    perms.set_mode(0o000);
                    fs::set_permissions(&p, perms).unwrap();
                }
            }
        }
    }

    assert!(fx.wt_with_store(&["create", "two"], &store).status.success());

    let expected = |i: usize| format!("fake-heavy file {i} of 12\n");
    let dest = fx.repo.parent().unwrap().join("origin-two/heavy");
    {
        let mut stack = vec![dest.clone()];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir).expect("read heavy") {
                let p = entry.expect("entry").path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    let mut perms = fs::metadata(&p).unwrap().permissions();
                    perms.set_mode(0o644);
                    fs::set_permissions(&p, perms).unwrap();
                }
            }
        }
    }
    for i in 0..12 {
        let p = dest.join(format!("pkg{:02}/nested/file-{i}.txt", i % 20));
        assert_eq!(
            fs::read(&p).unwrap_or_else(|e| panic!("cannot read {}: {e}", p.display())),
            expected(i).into_bytes()
        );
    }
}

#[test]
fn touched_file_still_hydrates_correct_bytes() {
    let fx = Fixture::heavy_repo(6);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let base = tempfile::tempdir().expect("tempdir");
    let store = base.path().join("store");

    assert!(fx.wt_with_store(&["create", "one"], &store).status.success());

    let target = fx.repo.join("heavy/pkg01/nested/file-1.txt");
    {
        let file = fs::OpenOptions::new()
            .append(true)
            .open(&target)
            .expect("open for touch");
        file.set_times(std::fs::FileTimes::new().set_modified(std::time::SystemTime::now()))
            .expect("bump mtime");
    }
    let (files_before, bytes_before) = calc_store_footprint(&store);

    assert!(fx.wt_with_store(&["create", "two"], &store).status.success());

    let (files_after, bytes_after) = calc_store_footprint(&store);
    assert_eq!((files_before, bytes_before), (files_after, bytes_after));
    assert_heavy_matches_source(
        &fx.repo.join("heavy"),
        &fx.repo.parent().unwrap().join("origin-two"),
    );
}

#[test]
fn deleted_cache_falls_back_to_full_reingest_and_populates_again() {
    let fx = Fixture::heavy_repo(10);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let base = tempfile::tempdir().expect("tempdir");
    let store = base.path().join("store");

    assert!(fx.wt_with_store(&["create", "one"], &store).status.success());
    assert!(store.join("ingest-cache.tsv").is_file());

    fs::remove_file(store.join("ingest-cache.tsv")).expect("delete cache");
    fs::write(
        fx.repo.join("heavy/pkg03/nested/file-3.txt"),
        "written while the cache was gone\n",
    )
    .expect("edit source");

    assert!(fx.wt_with_store(&["create", "two"], &store).status.success());
    assert!(store.join("ingest-cache.tsv").is_file());
    assert_heavy_matches_source(
        &fx.repo.join("heavy"),
        &fx.repo.parent().unwrap().join("origin-two"),
    );
}

#[test]
fn corrupt_cache_degrades_to_full_reingest_not_wrong_output() {
    let fx = Fixture::heavy_repo(10);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let base = tempfile::tempdir().expect("tempdir");
    let store = base.path().join("store");

    assert!(fx.wt_with_store(&["create", "one"], &store).status.success());

    fs::write(
        store.join("ingest-cache.tsv"),
        "\u{0}\u{1}garbage\t\t\t\nnot-a-cache",
    )
    .expect("corrupt cache");

    assert!(fx.wt_with_store(&["create", "two"], &store).status.success());
    assert_heavy_matches_source(
        &fx.repo.join("heavy"),
        &fx.repo.parent().unwrap().join("origin-two"),
    );
}

// =========================================================================
// Branch Stacking & Base Tracking (from branch_stacking.rs)
// =========================================================================

fn run_git_trimmed(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn create_with_base_records_symbolic_ref_and_initial_commit_in_mirror() {
    let store_dir = tempfile::tempdir().unwrap();
    let fx = Fixture::heavy_repo(10);
    run_git_trimmed(&fx.repo, &["branch", "-M", "main"]);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let initial_main_commit = run_git_trimmed(&fx.repo, &["rev-parse", "HEAD"]);

    let out = fx.wt_with_store(&["create", "stack-1", "--base", "main"], store_dir.path());
    assert!(out.status.success());

    let mirrors = wt_store::mirror::read_all(store_dir.path());
    assert!(!mirrors.is_empty());

    let mirror = mirrors
        .iter()
        .find_map(|r| r.mirror.as_ref().ok())
        .expect("valid mirror");

    assert_eq!(mirror.base_branch.as_deref(), Some("main"));
    assert_eq!(
        mirror.base_commit.as_deref(),
        Some(initial_main_commit.as_str())
    );
}

#[test]
fn base_movement_detected_in_json_and_human_output() {
    let fx = Fixture::heavy_repo(10);
    run_git_trimmed(&fx.repo, &["branch", "-M", "main"]);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let initial_main_commit = run_git_trimmed(&fx.repo, &["rev-parse", "HEAD"]);

    let out1 = fx.wt(&["create", "feat-1", "--base", "main"]);
    assert!(out1.status.success());

    fs::write(fx.repo.join("new-file.txt"), "advanced main\n").unwrap();
    run_git_trimmed(&fx.repo, &["add", "new-file.txt"]);
    run_git_trimmed(&fx.repo, &["commit", "-m", "advance main"]);
    let new_main_commit = run_git_trimmed(&fx.repo, &["rev-parse", "HEAD"]);
    assert_ne!(initial_main_commit, new_main_commit);

    let feat1_dir = fx.repo.parent().unwrap().join("origin-feat-1");
    let out_json = Command::new(env!("CARGO_BIN_EXE_flashwt"))
        .args(["create", "sub-feat", "--json"])
        .env("WT_STORE", fx.store_path())
        .current_dir(&feat1_dir)
        .output()
        .expect("run wt binary");

    assert!(out_json.status.success());
    let stdout = String::from_utf8_lossy(&out_json.stdout);
    let json: serde_json::Value = serde_json::from_str(stdout.trim()).expect("parse json");
    let diags = json["diagnostics"].as_array().expect("diagnostics array");

    let base_diag = diags
        .iter()
        .find(|d| d["code"] == "BASE_BRANCH_MOVED")
        .expect("BASE_BRANCH_MOVED diagnostic must be present");

    let msg = base_diag["message"].as_str().expect("message string");
    assert!(msg.contains("main"));
    assert!(msg.contains(&initial_main_commit));
    assert!(msg.contains(&new_main_commit));

    let out_human = Command::new(env!("CARGO_BIN_EXE_flashwt"))
        .args(["create", "sub-feat-2"])
        .env("WT_STORE", fx.store_path())
        .current_dir(&feat1_dir)
        .output()
        .expect("run wt binary");

    assert!(out_human.status.success());
    let stderr = String::from_utf8_lossy(&out_human.stderr);
    assert!(
        stderr.contains("warning: Base branch 'main' has moved")
            || stderr.contains("warning: base branch 'main' has moved")
    );

    let out_stacked = fx.wt(&["create", "feat-stacked", "--base", "feat-1", "--json"]);
    assert!(out_stacked.status.success());
    let stdout_stacked = String::from_utf8_lossy(&out_stacked.stdout);
    let json_stacked: serde_json::Value =
        serde_json::from_str(stdout_stacked.trim()).expect("parse json");
    let diags_stacked = json_stacked["diagnostics"]
        .as_array()
        .expect("diagnostics array");

    let stacked_diag = diags_stacked
        .iter()
        .find(|d| d["code"] == "BASE_BRANCH_MOVED")
        .expect("BASE_BRANCH_MOVED diagnostic for parent of feat-1 must be present");
    let msg_stacked = stacked_diag["message"].as_str().expect("message string");
    assert!(msg_stacked.contains("main"));
}

#[test]
fn no_diagnostic_when_base_has_not_moved() {
    let fx = Fixture::heavy_repo(10);
    run_git_trimmed(&fx.repo, &["branch", "-M", "main"]);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let out = fx.wt(&["create", "feat-clean", "--base", "main", "--json"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let json: serde_json::Value = serde_json::from_str(stdout.trim()).expect("parse json");
    let diags = json["diagnostics"].as_array().expect("diagnostics array");

    assert!(!diags.iter().any(|d| d["code"] == "BASE_BRANCH_MOVED"));
}

#[test]
fn remove_surfaces_base_movement_diagnostic() {
    let fx = Fixture::heavy_repo(10);
    run_git_trimmed(&fx.repo, &["branch", "-M", "main"]);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let out_create = fx.wt(&["create", "feat-to-remove", "--base", "main"]);
    assert!(out_create.status.success());

    fs::write(fx.repo.join("advance.txt"), "advance\n").unwrap();
    run_git_trimmed(&fx.repo, &["add", "advance.txt"]);
    run_git_trimmed(&fx.repo, &["commit", "-m", "advance"]);

    let out_remove = fx.wt(&["remove", "feat-to-remove", "--json"]);
    assert!(out_remove.status.success());
    let stdout = String::from_utf8_lossy(&out_remove.stdout);
    let json: serde_json::Value = serde_json::from_str(stdout.trim()).expect("parse json");
    let diags = json["diagnostics"].as_array().expect("diagnostics array");

    let base_diag = diags
        .iter()
        .find(|d| d["code"] == "BASE_BRANCH_MOVED")
        .expect("BASE_BRANCH_MOVED diagnostic should be present on remove");
    assert!(base_diag["message"].as_str().unwrap().contains("main"));
}

// =========================================================================
// Toolchain Relocation & Manifest Exclusions (from toolchain_relocation.rs)
// =========================================================================

#[test]
fn test_venv_post_hydration_relocation() {
    let base = tempfile::tempdir().unwrap();
    let repo = base.path().join("origin");
    fs::create_dir_all(&repo).unwrap();

    Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(&repo)
        .status()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(&repo)
        .status()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(&repo)
        .status()
        .unwrap();

    let venv = repo.join(".venv");
    fs::create_dir_all(venv.join("bin")).unwrap();
    fs::create_dir_all(venv.join("__pycache__")).unwrap();

    let cfg_content = format!(
        "home = /usr/bin\ninclude-system-site-packages = false\nversion = 3.11.2\nexecutable = /usr/bin/python3.11\ncommand = /usr/bin/python3 -m venv {}\n",
        venv.display()
    );
    fs::write(venv.join("pyvenv.cfg"), cfg_content).unwrap();

    let act_bash = format!(
        r#"VIRTUAL_ENV="{}"
export VIRTUAL_ENV
_OLD_VIRTUAL_PATH="$PATH"
PATH="$VIRTUAL_ENV/bin:$PATH"
export PATH
"#,
        venv.display()
    );
    fs::write(venv.join("bin/activate"), act_bash).unwrap();

    let act_csh = format!(r#"setenv VIRTUAL_ENV "{}""#, venv.display());
    fs::write(venv.join("bin/activate.csh"), act_csh).unwrap();

    let act_fish = format!(r#"set -gx VIRTUAL_ENV "{}""#, venv.display());
    fs::write(venv.join("bin/activate.fish"), act_fish).unwrap();

    let script_content = format!(
        "#!{}/bin/python3\nimport sys\nprint('hello from worktree')\n",
        venv.display()
    );
    let script_path = venv.join("bin/pytest");
    fs::write(&script_path, script_content).unwrap();
    #[cfg(unix)]
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();

    let pyc_bytes = b"\x6f\r\r\n\0\0\0\0\0\0\0\0dummybytecode";
    let pyc_path = venv.join("bin/cached.pyc");
    fs::write(&pyc_path, pyc_bytes).unwrap();
    let pyc_cache_path = venv.join("__pycache__/module.cpython-311.pyc");
    fs::write(&pyc_cache_path, pyc_bytes).unwrap();

    fs::write(repo.join("main.py"), "print('hello')\n").unwrap();
    Command::new("git")
        .args(["add", "main.py"])
        .current_dir(&repo)
        .status()
        .unwrap();
    Command::new("git")
        .args(["commit", "--quiet", "-m", "init"])
        .current_dir(&repo)
        .status()
        .unwrap();

    let store_dir = base.path().join("store");
    let wt_out = Command::new(env!("CARGO_BIN_EXE_flashwt"))
        .args(["create", "demo"])
        .env("WT_STORE", &store_dir)
        .current_dir(&repo)
        .output()
        .unwrap();

    assert!(wt_out.status.success());

    let dest = base.path().join("origin-demo");
    let dest_venv = dest.join(".venv");

    let updated_cfg = fs::read_to_string(dest_venv.join("pyvenv.cfg")).unwrap();
    assert!(updated_cfg.contains(&format!(
        "command = /usr/bin/python3 -m venv {}",
        dest_venv.display()
    )));
    assert!(!updated_cfg.contains(&venv.to_string_lossy().into_owned()));

    let updated_bash = fs::read_to_string(dest_venv.join("bin/activate")).unwrap();
    assert!(updated_bash.contains(&format!("VIRTUAL_ENV=\"{}\"", dest_venv.display())));
    assert!(!updated_bash.contains(&venv.to_string_lossy().into_owned()));

    let updated_csh = fs::read_to_string(dest_venv.join("bin/activate.csh")).unwrap();
    assert!(updated_csh.contains(&format!("setenv VIRTUAL_ENV \"{}\"", dest_venv.display())));

    let updated_fish = fs::read_to_string(dest_venv.join("bin/activate.fish")).unwrap();
    assert!(updated_fish.contains(&format!("set -gx VIRTUAL_ENV \"{}\"", dest_venv.display())));

    let updated_script = fs::read_to_string(dest_venv.join("bin/pytest")).unwrap();
    assert!(updated_script.starts_with(&format!("#!{}/bin/python3\n", dest_venv.display())));
    #[cfg(unix)]
    {
        let perms = fs::metadata(dest_venv.join("bin/pytest"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(perms & 0o777, 0o755);
    }

    assert_eq!(
        fs::read(dest_venv.join("bin/cached.pyc")).unwrap(),
        pyc_bytes
    );
    assert_eq!(
        fs::read(dest_venv.join("__pycache__/module.cpython-311.pyc")).unwrap(),
        pyc_bytes
    );
}

#[test]
fn test_starter_manifest_and_volatile_cache_exclusions() {
    let fx = Fixture::heavy_repo(5);

    let target_dir = fx.repo.join("target");
    fs::create_dir_all(target_dir.join("debug/deps")).unwrap();
    fs::write(target_dir.join("debug/deps/libfoo.rlib"), b"rlib data").unwrap();
    fs::create_dir_all(target_dir.join("debug/incremental/app-xyz")).unwrap();
    fs::write(
        target_dir.join("debug/incremental/app-xyz/s-abc.o"),
        b"incremental data",
    )
    .unwrap();

    let node_modules_dir = fx.repo.join("node_modules");
    fs::create_dir_all(node_modules_dir.join("pkg")).unwrap();
    fs::write(
        node_modules_dir.join("pkg/index.js"),
        b"console.log('pkg');",
    )
    .unwrap();
    fs::create_dir_all(node_modules_dir.join(".vite/deps")).unwrap();
    fs::write(node_modules_dir.join(".vite/deps/react.js"), b"vite cache").unwrap();

    let next_dir = fx.repo.join(".next");
    fs::create_dir_all(next_dir.join("cache/webpack")).unwrap();
    fs::write(next_dir.join("cache/webpack/bundle.pack"), b"next cache").unwrap();

    let init_out = fx.wt(&["init"]);
    assert!(init_out.status.success());

    let starter = fs::read_to_string(fx.repo.join(".wtinclude")).unwrap();
    assert!(starter.contains("!target/debug/incremental/"));
    assert!(starter.contains("!node_modules/.vite/"));
    assert!(starter.contains("!.next/cache/"));

    let out = fx.wt(&["create", "demo"]);
    assert!(out.status.success());

    let dest = fx.repo.parent().unwrap().join("origin-demo");
    assert!(dest.join("target/debug/deps/libfoo.rlib").is_file());
    assert!(dest.join("node_modules/pkg/index.js").is_file());

    assert!(!dest.join("target/debug/incremental").exists());
    assert!(!dest.join("node_modules/.vite").exists());
    assert!(!dest.join(".next/cache").exists());
}

#[test]
fn test_cargo_workspace_builds_in_hydrated_worktree() {
    let base = tempfile::tempdir().unwrap();
    let repo = base.path().join("origin");
    fs::create_dir_all(repo.join("src")).unwrap();

    Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(&repo)
        .status()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(&repo)
        .status()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(&repo)
        .status()
        .unwrap();

    let cargo_toml = r#"[package]
name = "sample-crate"
version = "0.1.0"
edition = "2021"

[dependencies]
"#;
    fs::write(repo.join("Cargo.toml"), cargo_toml).unwrap();
    fs::write(
        repo.join("src/main.rs"),
        "fn main() { println!(\"cargo-test-ok\"); }\n",
    )
    .unwrap();

    Command::new("git")
        .args(["add", "."])
        .current_dir(&repo)
        .status()
        .unwrap();
    Command::new("git")
        .args(["commit", "--quiet", "-m", "init"])
        .current_dir(&repo)
        .status()
        .unwrap();

    let build_out = Command::new("cargo")
        .args(["build"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(build_out.status.success());

    assert!(repo.join("target/debug/sample-crate").is_file());
    assert!(repo.join("target/debug/incremental").is_dir());

    let store_dir = base.path().join("store");
    let wt_out = Command::new(env!("CARGO_BIN_EXE_flashwt"))
        .args(["create", "demo"])
        .env("WT_STORE", &store_dir)
        .current_dir(&repo)
        .output()
        .unwrap();

    assert!(wt_out.status.success());

    let dest = base.path().join("origin-demo");
    assert!(dest.join("target/debug/deps").is_dir());
    assert!(!dest.join("target/debug/incremental").exists());

    let run_out = Command::new("cargo")
        .args(["run", "--quiet"])
        .current_dir(&dest)
        .output()
        .unwrap();

    assert!(run_out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&run_out.stdout).trim(),
        "cargo-test-ok"
    );
}

// =========================================================================
// Linux Copy Acceleration & Probing (from copy_acceleration.rs)
// =========================================================================

#[test]
fn store_initialization_probes_and_caches_filesystem_capabilities() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store_dir = temp.path().join("store");
    let store = DiskStore::open(&store_dir).expect("open store");

    let caps: FsCapabilities = store.fs_capabilities();
    assert!(caps.device_id > 0);

    let direct = probe_fs(&store_dir).expect("probe_fs");
    assert_eq!(caps.device_id, direct.device_id);
    assert_eq!(caps.fs_type, direct.fs_type);
    assert_eq!(caps.reflink_capable, direct.reflink_capable);

    #[cfg(target_os = "linux")]
    {
        assert!(caps.fs_type > 0);
        if caps.reflink_capable {
            assert!(
                caps.fs_type == (libc::BTRFS_SUPER_MAGIC as u64)
                    || caps.fs_type == (libc::XFS_SUPER_MAGIC as u64)
            );
        }
    }

    #[cfg(target_os = "macos")]
    {
        assert!(caps.reflink_capable);
    }
}

#[test]
fn strategy_selection_prefers_acceleration_on_linux() {
    let temp = tempfile::tempdir().expect("tempdir");
    let caps = probe_fs(temp.path()).expect("probe_fs");

    let candidate_list = candidates();
    assert!(!candidate_list.is_empty());

    #[cfg(target_os = "linux")]
    {
        assert!(
            candidate_list
                .iter()
                .any(|b| b.kind() == BackendKind::CopyFileRange)
        );
        assert!(
            candidate_list
                .iter()
                .any(|b| b.kind() == BackendKind::Reflink)
        );

        let picked = select_backend(temp.path(), SourcePolicy::Any);
        if caps.reflink_capable {
            assert_eq!(picked.kind(), BackendKind::Reflink);
        } else if caps.is_ext4() {
            assert_eq!(picked.kind(), BackendKind::CopyFileRange);
        }
    }

    #[cfg(target_os = "macos")]
    {
        let picked = select_backend(temp.path(), SourcePolicy::Any);
        if caps.reflink_capable {
            assert_eq!(picked.kind(), BackendKind::Clonefile);
        }
    }
}

#[test]
fn copy_file_range_accelerated_materialization_produces_identical_trees() {
    let fx = Fixture::heavy_repo(50);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let out = fx.wt(&["create", "accel", "--json"]);
    assert!(out.status.success());

    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.trim().lines().collect();
    assert_eq!(lines.len(), 1);

    let json: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(json["status"], "ok");
    assert_eq!(json["data"]["files_hydrated"], 50);

    let method = json["data"]["hydration_method"].as_str().unwrap();
    assert!(matches!(
        method,
        "clone" | "reflink" | "copy_file_range" | "byte_copy"
    ));

    let hydrated = fx
        .repo
        .parent()
        .unwrap()
        .join("origin-accel/heavy/pkg00/nested/file-0.txt");
    assert!(hydrated.exists());
    assert_eq!(
        fs::read_to_string(&hydrated).unwrap(),
        "fake-heavy file 0 of 50\n"
    );
    #[cfg(unix)]
    {
        let meta = fs::metadata(&hydrated).unwrap();
        assert_eq!(meta.permissions().mode() & 0o200, 0o200);
    }
}

#[test]
fn cross_device_or_fallback_copies_emit_diagnostic_warning_in_json() {
    let fx = Fixture::heavy_repo(20);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_flashwt"))
        .args(["create", "cross-vol", "--json"])
        .env("WT_SNAPSHOTS", "0")
        .env("WT_NO_HARDLINK", "1")
        .current_dir(&fx.repo)
        .output()
        .expect("run wt binary");

    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.trim().lines().collect();
    assert_eq!(lines.len(), 1);

    let json: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(json["status"], "ok");
    assert_eq!(json["data"]["hydration_method"], "byte_copy");
    assert_eq!(json["data"]["bytes_shared_cow"], 0);
    assert!(json["data"]["bytes_copied"].as_u64().unwrap() > 0);

    let diags = json["diagnostics"].as_array().unwrap();
    assert!(
        diags
            .iter()
            .any(|d| d["code"] == "CROSS_DEVICE_COPY_DEGRADATION")
    );
}

#[cfg(unix)]
#[test]
fn sequential_buffered_copy_preserves_executable_mode_and_content() {
    let temp = tempfile::tempdir().expect("tempdir");
    let src = temp.path().join("script.sh");
    let dest = temp.path().join("dest_script.sh");

    let script_content = "#!/bin/sh\necho 'hello from accelerated sequential copy'\n";
    fs::write(&src, script_content).expect("write script");
    fs::set_permissions(&src, fs::Permissions::from_mode(0o755)).expect("set mode");

    let copied_bytes = wt_copy::buffered_copy_file(&src, &dest).expect("buffered_copy_file");
    assert_eq!(copied_bytes, script_content.len() as u64);
    assert_eq!(
        fs::read_to_string(&dest).expect("read dest"),
        script_content
    );

    let dest_mode = fs::metadata(&dest).expect("metadata").permissions().mode();
    assert_eq!(dest_mode & 0o7777, 0o755);
}

// =========================================================================
// Trust-Once Materialization & Verified Ledger (from verify_once.rs)
// =========================================================================

fn tamper_verify_blob(store: &Path, restore_mtime: bool) {
    let blob = list_files(&store.join("objects"))
        .into_iter()
        .find(|p| p.is_file())
        .expect("store has objects");
    let mtime = fs::metadata(&blob).unwrap().modified().unwrap();
    let mut bytes = fs::read(&blob).unwrap();
    bytes[0] ^= 0xff;
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(&blob).unwrap().permissions();
        perms.set_mode(perms.mode() | 0o200);
        fs::set_permissions(&blob, perms).unwrap();
    }
    fs::write(&blob, &bytes).unwrap();
    if restore_mtime {
        let f = fs::OpenOptions::new()
            .write(true)
            .open(&blob)
            .expect("reopen blob");
        f.set_times(fs::FileTimes::new().set_modified(mtime))
            .expect("restore mtime");
    } else {
        let shifted = mtime + std::time::Duration::from_secs(60);
        let f = fs::OpenOptions::new()
            .write(true)
            .open(&blob)
            .expect("reopen blob");
        f.set_times(fs::FileTimes::new().set_modified(shifted))
            .expect("shift mtime");
    }
}

fn wt_verify_run(
    fx: &Fixture,
    args: &[&str],
    store: &Path,
    extra_env: &[(&str, &str)],
) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_flashwt"));
    cmd.args(args)
        .env("WT_STORE", store)
        .env("WT_SNAPSHOTS", "0")
        .env("WT_SNAPSHOTS_V2", "0")
        .env_remove("WT_HARDLINK")
        .env_remove("WT_NO_HARDLINK")
        .env_remove("WT_VERIFY")
        .current_dir(&fx.repo);
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    cmd.output().expect("run wt binary")
}

fn verify_fixture(files: usize) -> (Fixture, PathBuf) {
    let fx = Fixture::heavy_repo(files);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();
    let base = tempfile::tempdir().unwrap();
    let store = base.path().join("store");
    (fx, store)
}

#[test]
fn second_create_succeeds_on_tampering_no_stat_can_see() {
    let (fx, store) = verify_fixture(20);

    let first = wt_verify_run(&fx, &["create", "one"], &store, &[]);
    assert!(first.status.success());
    assert!(store.join("verified.tsv").is_file());

    tamper_verify_blob(&store, true);

    let second = wt_verify_run(&fx, &["create", "two"], &store, &[]);
    assert!(second.status.success());
}

#[test]
fn tampering_that_moves_mtime_still_fails_loudly() {
    let (fx, store) = verify_fixture(20);

    let first = wt_verify_run(&fx, &["create", "good"], &store, &[]);
    assert!(first.status.success());

    tamper_verify_blob(&store, false);

    let second = wt_verify_run(&fx, &["create", "bad"], &store, &[]);
    assert!(!second.status.success());
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(stderr.contains("hash verification"));
}

#[test]
fn deleted_ledger_forces_full_verification_again() {
    let (fx, store) = verify_fixture(20);

    let first = wt_verify_run(&fx, &["create", "one"], &store, &[]);
    assert!(first.status.success());

    fs::remove_file(store.join("verified.tsv")).expect("delete ledger");
    tamper_verify_blob(&store, true);

    let second = wt_verify_run(&fx, &["create", "two"], &store, &[]);
    assert!(!second.status.success());
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(stderr.contains("hash verification"));
}

#[test]
fn wt_verify_catches_even_stat_invisible_tampering() {
    let (fx, store) = verify_fixture(20);

    let first = wt_verify_run(&fx, &["create", "one"], &store, &[]);
    assert!(first.status.success());

    tamper_verify_blob(&store, true);

    let paranoid = wt_verify_run(&fx, &["create", "two"], &store, &[("WT_VERIFY", "1")]);
    assert!(!paranoid.status.success());
    let stderr = String::from_utf8_lossy(&paranoid.stderr);
    assert!(stderr.contains("hash verification"));
}
