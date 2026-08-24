//! Copy-backend integration tests (ticket 03).
//!
//! Everything runs through the frozen [`wt_copy::CopyBackend`] trait
//! against real temporary directories. Platform-specific backends are
//! exercised where the host filesystem supports them and skipped
//! (never failed) where it does not.

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use common::{assert_trees_identical, list_files, unix_symlink, TreeFixture};
use wt_copy::{
    candidates, select_backend, BackendKind, CopyBackend, DeepCopyBackend, Error, Safety,
    SourcePolicy,
};

/// Backends that can actually operate on `dir` right now: supported,
/// and enabled by safety. Since ticket 07 this includes hardlink.
fn runnable_backends(dir: &Path) -> Vec<Box<dyn CopyBackend>> {
    candidates()
        .into_iter()
        .filter(|b| b.safety() == Safety::Safe && b.supports(dir))
        .collect()
}

/// The one integration test through the trait: every backend that can
/// run on this filesystem must produce a faithful copy of the fixture
/// tree, including nested directories, permissions, and symlinks.
#[test]
fn every_safe_backend_copies_the_fixture_tree_through_the_trait() {
    let fixture = TreeFixture::heavy_tree(40);
    for backend in runnable_backends(fixture.src.parent().unwrap()) {
        let dest = fixture.src.parent().unwrap().join("dest-through-trait");
        let _ = std::fs::remove_dir_all(&dest);

        backend.copy_dir(&fixture.src, &dest).expect("copy_dir");
        assert_trees_identical(&fixture.src, &dest);
    }
}

/// A symlink pointing outside `src` must be recreated as a symlink
/// with the same target — its content must not be copied into the
/// destination tree.
#[test]
fn backends_recreate_outgoing_symlinks_instead_of_following_them() {
    let fixture = TreeFixture::heavy_tree(5);
    for backend in runnable_backends(fixture.src.parent().unwrap()) {
        let dest = fixture.src.parent().unwrap().join("dest-symlink");
        let _ = std::fs::remove_dir_all(&dest);

        backend.copy_dir(&fixture.src, &dest).expect("copy_dir");

        let outside_bytes = std::fs::read(fixture.outside.join("outside.txt")).expect("read");
        let copied = list_files(&dest);
        let materialized = copied
            .iter()
            .filter(|p| !p.is_symlink())
            .any(|p| std::fs::read(p).is_ok_and(|b| b == outside_bytes));
        assert!(
            !materialized,
            "{:?} materialized the outgoing symlink's target",
            backend.kind(),
        );
    }
}

/// `dest` must not exist; every backend refuses to merge or overwrite.
#[test]
fn all_backends_reject_an_existing_destination() {
    let fixture = TreeFixture::heavy_tree(3);
    for backend in runnable_backends(fixture.src.parent().unwrap()) {
        let dest = fixture.src.parent().unwrap().join("dest-exists");
        let _ = std::fs::remove_dir_all(&dest);
        std::fs::create_dir(&dest).expect("mkdir dest");

        match backend.copy_dir(&fixture.src, &dest) {
            Err(Error::DestinationExists) => {}
            other => panic!("{:?} returned {other:?} for existing dest", backend.kind()),
        }
    }
}

/// An error must not leave a half-copied tree behind that looks
/// trustworthy: copying onto an occupied path fails cleanly.
#[test]
fn failed_copy_leaves_no_new_files_in_dest() {
    let fixture = TreeFixture::heavy_tree(3);
    let dest = fixture.src.parent().unwrap().join("dest-failed");
    std::fs::create_dir(&dest).expect("mkdir dest");
    let before = list_files(&dest).len();

    let deep = DeepCopyBackend;
    assert!(matches!(
        deep.copy_dir(&fixture.src, &dest),
        Err(Error::DestinationExists)
    ));
    assert_eq!(list_files(&dest).len(), before, "failed copy mutated dest");
}

/// Empty directories must survive the copy: real heavy trees have
/// them (`node_modules/.bin/`, `.gitkeep`-style placeholders).
#[test]
fn all_backends_preserve_empty_directories() {
    let fixture = TreeFixture::heavy_tree(2);
    std::fs::create_dir(fixture.src.join("empty-pkg")).expect("mkdir empty");
    std::fs::create_dir_all(fixture.src.join("a/very/deep/only-dirs")).expect("mkdir nested");

    for backend in runnable_backends(fixture.src.parent().unwrap()) {
        let dest = fixture
            .src
            .parent()
            .unwrap()
            .join(format!("dest-empty-{:?}", backend.kind()));
        let _ = std::fs::remove_dir_all(&dest);

        backend.copy_dir(&fixture.src, &dest).expect("copy_dir");
        assert!(
            dest.join("empty-pkg").is_dir(),
            "{:?} lost an empty directory",
            backend.kind()
        );
        assert!(
            dest.join("a/very/deep/only-dirs").is_dir(),
            "{:?} lost a nested directory-only chain",
            backend.kind()
        );
    }
}

/// A symlink pointing at a directory inside `src` is recreated as a
/// symlink with the same target — its subtree must not be duplicated.
#[test]
fn backends_recreate_symlinks_to_directories_instead_of_expanding_them() {
    let fixture = TreeFixture::heavy_tree(3);
    unix_symlink("pkg00/nested", &fixture.src.join("dir-link"));

    for backend in runnable_backends(fixture.src.parent().unwrap()) {
        let dest = fixture
            .src
            .parent()
            .unwrap()
            .join(format!("dest-dir-link-{:?}", backend.kind()));
        let _ = std::fs::remove_dir_all(&dest);

        backend.copy_dir(&fixture.src, &dest).expect("copy_dir");

        let link = dest.join("dir-link");
        let meta = std::fs::symlink_metadata(&link)
            .expect("dir link must exist");
        assert!(meta.is_symlink(), "{:?} expanded a directory symlink", backend.kind());
        assert_eq!(
            std::fs::read_link(&link).expect("read_link"),
            std::path::Path::new("pkg00/nested"),
            "{:?} retargeted the directory symlink",
            backend.kind()
        );
        // A symlink at that path means the backend did not replace it
        // with a real directory holding duplicated content.
    }
}

/// Symlinks whose target does not exist are recreated verbatim too;
/// nothing may try to resolve or materialize them.
#[test]
fn all_backends_preserve_dangling_symlinks() {
    let fixture = TreeFixture::heavy_tree(2);
    unix_symlink("no/such/target", &fixture.src.join("dangling.txt"));
    // Sanity: it really dangles on this host.
    assert!(!fixture.src.join("dangling.txt").exists());

    for backend in runnable_backends(fixture.src.parent().unwrap()) {
        let dest = fixture
            .src
            .parent()
            .unwrap()
            .join(format!("dest-dangling-{:?}", backend.kind()));
        let _ = std::fs::remove_dir_all(&dest);

        backend.copy_dir(&fixture.src, &dest).expect("copy_dir");

        let meta = std::fs::symlink_metadata(dest.join("dangling.txt"))
            .expect("dangling symlink must be recreated");
        assert!(meta.is_symlink(), "{:?} materialized a dangling symlink", backend.kind());
        assert_eq!(
            std::fs::read_link(dest.join("dangling.txt")).expect("read_link"),
            std::path::Path::new("no/such/target"),
            "{:?} rewrote a dangling symlink's target",
            backend.kind()
        );
    }
}

/// Acceptance: clonefile clones a thousand-file directory in well
/// under a second on APFS.
///
/// Ignored by default: wall-clock acceptance, flaky on loaded CI
/// machines. Run explicitly with `cargo test -p wt-copy -- --ignored`.
#[cfg(target_os = "macos")]
#[test]
#[ignore = "wall-clock acceptance; run with --ignored on a quiet machine"]
fn clonefile_clones_a_thousand_file_directory_well_under_a_second() {
    use wt_copy::ClonefileBackend;

    let fixture = TreeFixture::heavy_tree(1000);
    let dest = fixture.src.parent().unwrap().join("dest-clonefile-perf");

    let backend = ClonefileBackend;
    assert!(
        backend.supports(&fixture.src),
        "tempdir is expected to be APFS on macOS"
    );

    let start = Instant::now();
    backend
        .copy_dir(&fixture.src, &dest)
        .expect("clonefile copy_dir");
    let elapsed = start.elapsed();

    assert_trees_identical(&fixture.src, &dest);
    assert!(
        elapsed < Duration::from_secs(1),
        "clonefile took {elapsed:?}, acceptance is well under one second"
    );
}

/// Acceptance: reflink passes the same shape of test on a supporting
/// Linux filesystem (btrfs/XFS). Skipped elsewhere, including tmpfs.
///
/// Ignored by default: wall-clock acceptance. Run explicitly with
/// `cargo test -p wt-copy -- --ignored`.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "wall-clock acceptance; run with --ignored on a quiet machine"]
fn reflink_copies_on_a_supporting_linux_filesystem() {
    use wt_copy::ReflinkBackend;

    let fixture = TreeFixture::heavy_tree(1000);
    let backend = ReflinkBackend;
    if !backend.supports(&fixture.src) {
        // /tmp is usually tmpfs or ext4; nothing to prove here.
        return;
    }

    let dest = fixture.src.parent().unwrap().join("dest-reflink-perf");
    let start = Instant::now();
    backend
        .copy_dir(&fixture.src, &dest)
        .expect("reflink copy_dir");
    let elapsed = start.elapsed();

    assert_trees_identical(&fixture.src, &dest);
    assert!(elapsed < Duration::from_secs(1), "reflink took {elapsed:?}");
}

/// Ticket 07: hardlink mode runs, guarded — linked files share one
/// read-only inode with the source.
#[test]
fn hardlink_backend_runs_with_copy_on_shared_write_guard() {
    use std::os::unix::fs::MetadataExt;

    use wt_copy::HardlinkBackend;

    let backend = HardlinkBackend;
    assert_eq!(backend.kind(), BackendKind::Hardlink);
    assert_eq!(backend.safety(), Safety::Safe);
    let fixture = TreeFixture::heavy_tree(2);
    assert!(
        backend.supports(&fixture.src),
        "tempdirs sit on ordinary writable filesystems"
    );
    let dest = fixture.src.parent().unwrap().join("dest-hardlink");
    backend.copy_dir(&fixture.src, &dest).expect("copy_dir");

    let src_file = fixture.src.join("pkg00/nested/file-0.txt");
    let dest_file = dest.join("pkg00/nested/file-0.txt");
    let meta = std::fs::metadata(&dest_file).expect("meta");
    assert!(meta.nlink() >= 2, "linked file must share its inode");
    assert!(
        meta.permissions().readonly(),
        "shared inode must be read-only"
    );
    assert!(
        std::fs::write(&dest_file, b"poison").is_err(),
        "in-place rewrite of a shared inode must fail"
    );
    assert_eq!(
        std::fs::read(&src_file).expect("read"),
        b"file 0 of 2\n",
        "source content changed"
    );
}

/// Selection picks the best available backend per filesystem: always
/// something safe, and the fastest thing that works. The integration
/// default is `Any` — the conservative promise — so hardlink is only
/// ever picked by callers that explicitly claim immutability.
#[test]
fn selection_picks_the_best_available_backend() {
    let fixture = TreeFixture::heavy_tree(1);
    let dir = fixture.src.parent().unwrap();

    let picked = select_backend(dir, SourcePolicy::Any);
    assert_eq!(picked.safety(), Safety::Safe);
    assert!(picked.supports(dir));
    assert_ne!(
        picked.kind(),
        BackendKind::Hardlink,
        "Any policy must never select hardlink"
    );

    #[cfg(target_os = "macos")]
    assert_eq!(
        picked.kind(),
        BackendKind::Clonefile,
        "APFS tempdirs must select clonefile"
    );

    // Candidates are ordered fastest-first and end with the portable
    // fallback; hardlink is the last fast candidate (ticket 07).
    let all = candidates();
    assert!(!all.is_empty());
    assert!(all.iter().any(|b| b.kind() == BackendKind::DeepCopy));
    assert_eq!(all.last().unwrap().kind(), BackendKind::DeepCopy);
    assert_eq!(all[all.len() - 2].kind(), BackendKind::Hardlink);

    // With an Immutable promise, a filesystem without clone support
    // may pick hardlink — but on APFS clonefile still wins.
    let immutable = select_backend(dir, SourcePolicy::Immutable);
    assert_eq!(immutable.safety(), Safety::Safe);

    // Even against paths where nothing but the floor reports support,
    // both policies yield deep copy rather than panicking.
    let nowhere = Path::new("/definitely/not/here");
    for policy in [SourcePolicy::Immutable, SourcePolicy::Any] {
        assert_eq!(select_backend(nowhere, policy).kind(), BackendKind::DeepCopy);
    }
}

/// Selection falls back to deep copy when no fast mechanism applies.
/// Simulated by asking each candidate about a path on a filesystem
/// where only the fallback reports support — here we assert the
/// contract directly on the fallback itself.
#[test]
fn deep_copy_is_always_available() {
    let backend = DeepCopyBackend;
    assert_eq!(backend.kind(), BackendKind::DeepCopy);
    assert_eq!(backend.safety(), Safety::Safe);
    assert!(backend.supports(Path::new("/")));

    let fixture = TreeFixture::heavy_tree(10);
    let dest = fixture.src.parent().unwrap().join("dest-deep");
    backend
        .copy_dir(&fixture.src, &dest)
        .expect("deep copy_dir");
    assert_trees_identical(&fixture.src, &dest);
}

/// Symlinks inside `src` that point at other files inside `src` are
/// recreated as-is too (relative form preserved).
#[test]
fn internal_symlinks_are_preserved_verbatim() {
    let fixture = TreeFixture::heavy_tree(3);
    unix_symlink(
        "pkg00/nested/file-0.txt",
        &fixture.src.join("internal-link.txt"),
    );

    let deep = DeepCopyBackend;
    let dest = fixture.src.parent().unwrap().join("dest-internal-link");
    deep.copy_dir(&fixture.src, &dest).expect("deep copy_dir");
    assert_trees_identical(&fixture.src, &dest);
}
