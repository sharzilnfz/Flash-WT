//! Unit tests for the clonefile backend. macOS only; the tempdirs
//! `tempfile` hands out are on APFS on every supported host.

use std::fs;

use super::*;
use crate::Error;

#[test]
fn supports_reports_apfs_tempdir() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert!(super::ClonefileBackend.supports(dir.path()));
}

#[test]
fn clones_nested_tree_and_rejects_existing_dest() {
    let base = tempfile::tempdir().expect("tempdir");
    let src = base.path().join("src");
    fs::create_dir_all(src.join("a/b")).expect("mkdir");
    fs::write(src.join("a/b/f.txt"), "cloned bytes\n").expect("write");

    let dest = base.path().join("dest");
    ClonefileBackend.copy_dir(&src, &dest).expect("copy_dir");
    assert_eq!(
        fs::read_to_string(dest.join("a/b/f.txt")).expect("read"),
        "cloned bytes\n"
    );

    assert!(matches!(
        ClonefileBackend.copy_dir(&src, &dest),
        Err(Error::DestinationExists)
    ));
}
