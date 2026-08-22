//! Unit tests for the reflink backend. Only meaningful on btrfs/XFS;
//! the tests skip silently on tmpfs or ext4 test hosts.

use std::fs;

use super::*;

#[test]
fn reflinks_when_the_filesystem_supports_it() {
    let base = match tempfile::tempdir() {
        Ok(d) => d,
        Err(e) => panic!("tempdir: {e}"),
    };
    if !ReflinkBackend.supports(base.path()) {
        return; // tmpfs/ext4 host; covered by CI runners on btrfs/XFS.
    }

    let src = base.path().join("src");
    fs::create_dir_all(src.join("nested")).expect("mkdir");
    fs::write(src.join("nested/f.txt"), "reflinked bytes\n").expect("write");

    let dest = base.path().join("dest");
    ReflinkBackend.copy_dir(&src, &dest).expect("copy_dir");
    assert_eq!(
        fs::read_to_string(dest.join("nested/f.txt")).expect("read"),
        "reflinked bytes\n"
    );

    assert!(matches!(
        ReflinkBackend.copy_dir(&src, &dest),
        Err(crate::Error::DestinationExists)
    ));
}
