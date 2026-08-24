// Tests assert with unwrap/expect by design: a panic IS the failure
// signal under test, so the workspace restriction lints stay off here.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Fast-hydration ticket 03: CoW materialization as the default.
//!
//! Hydrated worktrees must behave like normal writable checkouts:
//! every hydrated file is a fresh private inode (st_nlink == 1) with
//! normal writable permissions, sharing the store blob's physical
//! blocks only until first write. Everything below runs the real `wt`
//! binary and asserts on files on disk.

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::{list_files, Fixture};

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

/// Run `wt create` with the environment stripped of both materialize
/// flags, so the test exercises the true default regardless of what
/// the invoking shell exports.
fn wt_default(fx: &Fixture, args: &[&str], store: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_wt"))
        .args(args)
        .env("WT_STORE", store)
        .env_remove("WT_HARDLINK")
        .env_remove("WT_NO_HARDLINK")
        .current_dir(&fx.repo)
        .output()
        .expect("run wt binary")
}

fn assert_created(out: &std::process::Output) {
    assert!(
        out.status.success(),
        "create failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn hydrated_file(worktree: &Path) -> PathBuf {
    let f = worktree.join("heavy/pkg00/nested/file-0.txt");
    assert!(f.exists(), "expected hydrated file at {}", f.display());
    f
}

#[test]
fn default_hydration_produces_private_writable_files() {
    let fx = Fixture::heavy_repo(60);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let base = tempfile::tempdir().unwrap();
    let store = base.path().join("store");
    let parent = fx.repo.parent().unwrap();

    for name in ["one", "two"] {
        let out = wt_default(&fx, &["create", name], &store);
        assert_created(&out);
    }

    let one = hydrated_file(&parent.join("origin-one"));
    let two = hydrated_file(&parent.join("origin-two"));
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
    // Private means private: the two trees do not share an inode.
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
        let out = wt_default(&fx, &["create", name], &store);
        assert_created(&out);
    }

    let sibling_baseline = snapshot(&parent.join("origin-two").join("heavy"));
    let store_baseline = snapshot(&store);

    let target = hydrated_file(&parent.join("origin-one"));

    // The package-manager rewrite that hardlinks refused loudly: on
    // the CoW default it just works, privately.
    fs::write(&target, b"rewritten in place\n")
        .expect("in-place rewrite must succeed on a CoW-hydrated file");
    assert_eq!(fs::read(&target).unwrap(), b"rewritten in place\n");

    // The write diverged without touching anything else.
    assert_eq!(
        snapshot(&parent.join("origin-two").join("heavy")),
        sibling_baseline,
        "sibling worktree saw the rewrite"
    );
    assert_eq!(
        snapshot(&store),
        store_baseline,
        "the store blob was poisoned through materialization"
    );

    // And the next tree hydrates clean bytes from the untouched store.
    let out = wt_default(&fx, &["create", "three"], &store);
    assert_created(&out);
    let three = snapshot(&parent.join("origin-three").join("heavy"));
    assert_eq!(sibling_baseline, three, "store diverged from its trees");
}

#[test]
/// Only meaningful where CoW clones exist (APFS): on filesystems
/// without clone support wt correctly falls back to byte copies,
/// which consume the full logical size and would fail this assert.
#[cfg(target_os = "macos")]
fn before_first_write_hydrated_files_share_physical_blocks_with_the_blob() {
    // Small files cannot show block sharing; build a repo whose heavy
    // directory holds several megabytes per file.
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

    // 4 x 16 MiB = 64 MiB of heavy content, so real sharing (a few KiB)
    // and a byte copy (>= 64 MiB) are separated by a wide margin.
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
        // SAFETY: valid NUL-terminated path, correctly sized buffer.
        let rc = unsafe { libc::statfs(c.as_ptr(), &mut st) };
        assert_eq!(rc, 0, "statfs failed");
        (st.f_bsize as u64) * (st.f_bavail as u64)
    };

    let before = free_bytes();
    let out = Command::new(env!("CARGO_BIN_EXE_wt"))
        .args(["create", "one"])
        .env("WT_STORE", &store)
        .env_remove("WT_HARDLINK")
        .env_remove("WT_NO_HARDLINK")
        .current_dir(&repo)
        .output()
        .expect("run wt binary");
    assert_created(&out);
    let consumed = before.saturating_sub(free_bytes());

    // Measured, not asserted: hydration must not consume anything
    // close to the logical size of the heavy tree. A byte-copy
    // materialization would spend at least 64 MiB here; clones share
    // the store blob's blocks and spend almost nothing.
    let logical = (file_len * 4) as u64;
    assert!(
        consumed < logical / 2,
        "hydrating must share blocks with the store before first write: \
         {consumed} new bytes for {logical} logical bytes"
    );

    // The content is still byte-perfect and privately diverges on
    // write without touching the store blob.
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
    // Put the destination on an HFS+ ram disk. HFS+ cannot serve as a
    // clone target and is a different volume than the APFS store, so
    // every placement attempt is refused at the filesystem level and
    // hydration must fall back to plain byte copies silently. Skipped
    // when ram disks are unavailable.
    let Some(volume) = HfsRamdisk::attach() else {
        eprintln!("skipping: could not create an HFS+ ram disk here");
        return;
    };

    let fx = Fixture::heavy_repo(20);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let base = tempfile::tempdir().unwrap();
    let store = base.path().join("store");
    let dest = volume.mount.join("origin-ramdisk");

    let out = Command::new(env!("CARGO_BIN_EXE_wt"))
        .args(["create", "ramdisk", "--dir"])
        .arg(&dest)
        .env("WT_STORE", &store)
        .current_dir(&fx.repo)
        .output()
        .expect("run wt binary");
    assert_created(&out);

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("copy-on-write unavailable"),
        "refused clones must be reported honestly:\n{stdout}"
    );
    assert!(
        stdout.contains("byte copies for 20 of 20 file(s)"),
        "every file should have fallen back to a byte copy:\n{stdout}"
    );

    // The fallback produces ordinary private writable files.
    let hydrated = hydrated_file(&dest);
    let meta = fs::metadata(&hydrated).unwrap();
    assert_eq!(meta.nlink(), 1, "byte copy owns its inode");
    assert!(!meta.permissions().readonly(), "byte copy is writable");
}

/// An HFS+ volume in memory: clone-hostile territory on macOS.
struct HfsRamdisk {
    mount: PathBuf,
}

impl HfsRamdisk {
    /// Attach, format, mount. `None` whenever the host refuses any
    /// step — the caller skips rather than fails.
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
        // eraseVolume mounts the fresh volume itself.
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
