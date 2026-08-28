// Tests assert with unwrap/expect by design: a panic IS the failure
// signal under test, so the workspace restriction lints stay off here.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Ticket 05: hydration flows through the content-addressed store.
//! Everything is asserted through the CLI seam: two worktrees of the
//! same project must share store content, and corrupt store content
//! must fail loudly instead of landing in a fresh tree.

mod common;

use std::fs;
use std::path::Path;

use common::{Fixture, list_files};

/// Total bytes and file count of everything under the store root —
/// the externally observable measure of how much content it holds.
fn store_footprint(store: &Path) -> (usize, usize) {
    let mut files = 0usize;
    let mut bytes = 0usize;
    let mut stack = vec![store.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).expect("read store dir") {
            let p = entry.expect("store entry").path();
            if p.is_dir() {
                // GC bookkeeping (ticket 07 mirrors, snapshots) is not content.
                if p.file_name() == Some(std::ffi::OsStr::new("worktrees"))
                    || p.file_name() == Some(std::ffi::OsStr::new("mirrors"))
                    || p.file_name() == Some(std::ffi::OsStr::new("snapshots"))
                {
                    continue;
                }
                stack.push(p);
            } else {
                files += 1;
                bytes += fs::metadata(&p).expect("store file metadata").len() as usize;
            }
        }
    }
    (files, bytes)
}

fn assert_same_tree(src: &Path, dest: &Path) {
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

    let (files_after_first, bytes_after_first) = store_footprint(&store);
    assert!(files_after_first > 0, "store stayed empty after ingest");

    let second = wt("two");
    assert!(
        second.status.success(),
        "second create failed: {}",
        String::from_utf8_lossy(&second.stderr)
    );

    // Dedupe: ingesting the same heavy directory again stored nothing
    // new — every blob already existed at its hash address.
    let (files_after_second, bytes_after_second) = store_footprint(&store);
    assert_eq!(
        files_after_first, files_after_second,
        "second worktree added new store objects"
    );
    assert_eq!(
        bytes_after_first, bytes_after_second,
        "second worktree grew the store's footprint"
    );

    // Both projections are complete and byte-identical to the source.
    let dest_two = fx.repo.parent().unwrap().join("origin-two");
    assert_same_tree(&fx.repo.join("heavy"), &dest_two.join("heavy"));
    let dest_one = fx.repo.parent().unwrap().join("origin-one");
    assert_same_tree(&fx.repo.join("heavy"), &dest_one.join("heavy"));

    // Each worktree holds a reference ledger ticket 06 will consume.
    // A worktree's .git is a pointer file, so resolve the linked
    // gitdir it names.
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

    // Corrupt one stored blob behind the CLI's back. `put` skips
    // existing objects by path, so the next ingest keeps the bad blob
    // and materialize is the step that must catch it.
    let objects = store.join("objects");
    let blob = list_files(&objects)
        .into_iter()
        .find(|p| p.is_file())
        .expect("store has objects");
    let mut bytes = fs::read(&blob).unwrap();
    bytes[0] ^= 0xff;
    // Since ticket 07, linked-out blobs carry a read-only shared
    // inode; disk-level corruption does not respect that, so neither
    // does the simulation.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
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

/// A heavy directory holding valid symlinks, a dangling symlink, and
/// mixed exec bits must hydrate back with all of that intact — on the
/// cold create and on the warm one served from cache hits alike.
#[test]
fn hydrated_trees_preserve_symlinks_and_permission_bits() {
    use std::os::unix::fs::PermissionsExt;

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
            std::path::Path::new("pkg00/nested/file-0.txt"),
            "{name}: symlink target changed"
        );

        // Dangling: lstat (not stat) is the only way to see it.
        let dangling = root.join("dangling-link");
        let meta = fs::symlink_metadata(&dangling).expect("dangling symlink must exist");
        assert!(meta.is_symlink(), "{name}: dangling link was materialized");
        assert_eq!(
            fs::read_link(&dangling).unwrap(),
            std::path::Path::new("../missing/target"),
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

/// Hardlinked placement shares one inode with the store blob, so an
/// exec-bit mismatch must be repaired by a private byte copy instead
/// of a chmod — the blob and every other linked tree keep their
/// read-only, non-exec inode.
#[test]
fn hardlink_mode_preserves_exec_bits_without_touching_the_shared_blob() {
    use std::collections::BTreeSet;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

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

    // Exec bit restored WITHOUT sharing: the private replacement copy
    // owns its inode.
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

    // Non-exec files keep the shared read-only inode.
    let plain = heavy.join("pkg00/nested/file-0.txt");
    let dest_plain = root.join(plain.strip_prefix(&heavy).unwrap());
    let meta = fs::metadata(&dest_plain).expect("non-exec file placed");
    assert!(meta.nlink() >= 2, "non-exec file should stay hardlinked");
    assert!(meta.permissions().readonly());

    // And the store blobs themselves never gained exec or write bits.
    let mut blobs = BTreeSet::new();
    collect_blobs(&store.join("objects"), &mut blobs);
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

fn collect_blobs(dir: &std::path::Path, out: &mut std::collections::BTreeSet<std::path::PathBuf>) {
    for entry in fs::read_dir(dir).expect("read objects dir") {
        let p = entry.expect("objects entry").path();
        if p.is_dir() {
            collect_blobs(&p, out);
        } else {
            out.insert(p);
        }
    }
}

/// Regression (dir-mode fidelity): hydration used to recreate
/// directories through `create_dir_all`, normalizing every directory
/// through the umask and losing modes like 0705 or 0700. Ingest now
/// records each walked directory's permission bits next to the file
/// modes, and materialize restores them deepest-first after placement.
#[test]
fn hydrated_trees_restore_directory_permission_bits() {
    use std::os::unix::fs::PermissionsExt;

    let fx = Fixture::heavy_repo(6);
    let heavy = fx.repo.join("heavy");

    // A restrictive subdirectory whose source mode differs from what a
    // umask-normalized create_dir_all would produce (0755 under the
    // usual 0022).
    let locked = heavy.join("pkg00/nested");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o705)).unwrap();
    let private_pkg = heavy.join("pkg01");
    fs::set_permissions(&private_pkg, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let base = tempfile::tempdir().expect("tempdir");
    let store = base.path().join("store");
    // WT_SNAPSHOTS off: exercise the per-file fallback ladder, which is
    // the path that builds directories itself. The snapshot clone
    // carries directory metadata natively.
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

/// Regression (honest reporting): under WT_HARDLINK, a link whose exec
/// bits cannot carry the recorded mode is replaced by a private byte
/// copy. That replacement IS a copy, so it must be counted in the
/// run's `copied` total and surface as "refused" in the summary line.
#[test]
fn hardlink_exec_bit_repair_counts_as_copied_in_reporting() {
    use std::os::unix::fs::PermissionsExt;

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
        "the private-copy repair must be reported as one refused hardlink \
         (1 repaired of 4 hydrated):\n{stdout}"
    );
}
