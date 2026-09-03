#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use common::{TreeFixture, assert_trees_identical, list_files, unix_symlink};
use flashwt_copy::{
    BackendKind, CopyBackend, DeepCopyBackend, Error, SourcePolicy, candidates, select_backend,
};

fn runnable_backends(dir: &Path) -> Vec<Box<dyn CopyBackend>> {
    candidates()
        .into_iter()
        .filter(|b| b.supports(dir))
        .collect()
}

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
        let meta = std::fs::symlink_metadata(&link).expect("dir link must exist");
        assert!(
            meta.is_symlink(),
            "{:?} expanded a directory symlink",
            backend.kind()
        );
        assert_eq!(
            std::fs::read_link(&link).expect("read_link"),
            std::path::Path::new("pkg00/nested"),
            "{:?} retargeted the directory symlink",
            backend.kind()
        );
    }
}

#[test]
fn all_backends_preserve_dangling_symlinks() {
    let fixture = TreeFixture::heavy_tree(2);
    unix_symlink("no/such/target", &fixture.src.join("dangling.txt"));

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
        assert!(
            meta.is_symlink(),
            "{:?} materialized a dangling symlink",
            backend.kind()
        );
        assert_eq!(
            std::fs::read_link(dest.join("dangling.txt")).expect("read_link"),
            std::path::Path::new("no/such/target"),
            "{:?} rewrote a dangling symlink's target",
            backend.kind()
        );
    }
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "wall-clock acceptance; run with --ignored on a quiet machine"]
fn clonefile_clones_a_thousand_file_directory_well_under_a_second() {
    use flashwt_copy::ClonefileBackend;

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

#[cfg(target_os = "linux")]
#[test]
#[ignore = "wall-clock acceptance; run with --ignored on a quiet machine"]
fn reflink_copies_on_a_supporting_linux_filesystem() {
    use flashwt_copy::ReflinkBackend;

    let fixture = TreeFixture::heavy_tree(1000);
    let backend = ReflinkBackend;
    if !backend.supports(&fixture.src) {
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

#[test]
fn hardlink_backend_runs_with_copy_on_shared_write_guard() {
    use std::os::unix::fs::MetadataExt;

    use flashwt_copy::HardlinkBackend;

    let backend = HardlinkBackend;
    assert_eq!(backend.kind(), BackendKind::Hardlink);
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

#[test]
fn selection_picks_the_best_available_backend() {
    let fixture = TreeFixture::heavy_tree(1);
    let dir = fixture.src.parent().unwrap();

    let picked = select_backend(dir, SourcePolicy::Any);
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

    let all = candidates();
    assert!(!all.is_empty());
    assert!(all.iter().any(|b| b.kind() == BackendKind::DeepCopy));
    assert_eq!(all.last().unwrap().kind(), BackendKind::DeepCopy);
    assert_eq!(all[all.len() - 2].kind(), BackendKind::Hardlink);

    let _immutable = select_backend(dir, SourcePolicy::Immutable);

    let nowhere = Path::new("/definitely/not/here");
    for policy in [SourcePolicy::Immutable, SourcePolicy::Any] {
        let picked = select_backend(nowhere, policy);
        assert!(picked.supports(nowhere));
    }
}

#[test]
fn deep_copy_is_always_available() {
    let backend = DeepCopyBackend;
    assert_eq!(backend.kind(), BackendKind::DeepCopy);
    assert!(backend.supports(Path::new("/")));

    let fixture = TreeFixture::heavy_tree(10);
    let dest = fixture.src.parent().unwrap().join("dest-deep");
    backend
        .copy_dir(&fixture.src, &dest)
        .expect("deep copy_dir");
    assert_trees_identical(&fixture.src, &dest);
}

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
