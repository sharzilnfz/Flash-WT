//! Unit tests for the file-shaped materializers. CloneOut is macOS
//! only; the tempdirs `tempfile` hands out are on APFS on every
//! supported host.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use super::*;

fn blob(dir: &Path, contents: &[u8]) -> std::path::PathBuf {
    let p = dir.join("blob.bin");
    fs::write(&p, contents).expect("write blob");
    p
}

#[test]
fn only_the_hardlink_backend_reports_a_shared_inode() {
    assert!(HardlinkOut.shares_inode_with_source());
    #[cfg(target_os = "macos")]
    assert!(!CloneOut.shares_inode_with_source());
}

#[test]
fn hardlink_out_shares_a_read_only_inode() {
    let base = tempfile::tempdir().expect("tempdir");
    let src = blob(base.path(), b"shared bytes\n");
    let dest = base.path().join("dest.txt");

    HardlinkOut.materialize_file(&src, &dest).expect("link");
    assert_eq!(fs::read(&dest).unwrap(), b"shared bytes\n");

    let meta = fs::metadata(&dest).unwrap();
    assert_eq!(meta.nlink(), 2, "link must share one inode");
    assert!(
        meta.permissions().readonly(),
        "linked copies carry stripped write bits"
    );
}

#[test]
fn hardlink_out_refuses_existing_dest() {
    let base = tempfile::tempdir().expect("tempdir");
    let src = blob(base.path(), b"x\n");
    let dest = base.path().join("dest.txt");
    fs::write(&dest, "already here\n").unwrap();

    let err = HardlinkOut
        .materialize_file(&src, &dest)
        .expect_err("EEXIST expected");
    assert_eq!(err.raw_os_error(), Some(libc::EEXIST));
}

#[cfg(target_os = "macos")]
#[test]
fn clone_out_makes_a_private_writable_clone() {
    use std::io::Write;

    let base = tempfile::tempdir().expect("tempdir");
    let src = blob(base.path(), b"cloned bytes\n");
    let dest = base.path().join("dest.txt");

    CloneOut.materialize_file(&src, &dest).expect("clone");
    assert_eq!(fs::read(&dest).unwrap(), b"cloned bytes\n");

    let meta = fs::metadata(&dest).unwrap();
    assert_eq!(meta.nlink(), 1, "the clone owns a fresh inode, not a link");
    assert!(
        !meta.permissions().readonly(),
        "hydrated files must be writable"
    );

    // The sharing is physical until first write: cloning an 8 MiB
    // file consumes no measurable new disk, where a byte copy would
    // consume its full size. Measured via the volume's free space,
    // because recent APFS reports st_blocks as if fully allocated
    // even for clones.
    let big = base.path().join("big.blob");
    let mut f = fs::File::create(&big).unwrap();
    f.write_all(&vec![0xa5u8; 8 << 20]).unwrap();
    drop(f);
    let before = free_bytes(base.path());
    let dest_big = base.path().join("big.clone");
    CloneOut
        .materialize_file(&big, &dest_big)
        .expect("clone big");
    let consumed = before.saturating_sub(free_bytes(base.path()));
    assert!(
        consumed < (4 << 20),
        "a pre-write clone must share blocks: {consumed} new bytes for 8 MiB"
    );

    // First write diverges privately; the blob keeps its bytes.
    fs::write(&dest_big, b"diverged\n").unwrap();
    assert_eq!(fs::read(&big).unwrap(), vec![0xa5u8; 8 << 20]);
    assert_eq!(fs::read(&dest_big).unwrap(), b"diverged\n");
}

/// Bytes available on the volume holding `dir`, via statfs(2).
#[cfg(target_os = "macos")]
fn free_bytes(dir: &Path) -> u64 {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(dir.as_os_str().as_bytes()).expect("no NUL in tempdir path");
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is a valid NUL-terminated path; `st` is a correctly
    // sized allocation owned by this call.
    let rc = unsafe { libc::statfs(c.as_ptr(), &mut st) };
    assert_eq!(rc, 0, "statfs failed");
    (st.f_bsize as u64) * (st.f_bavail as u64)
}

#[cfg(target_os = "macos")]
#[test]
fn clone_out_refuses_existing_dest() {
    let base = tempfile::tempdir().expect("tempdir");
    let src = blob(base.path(), b"x\n");
    let dest = base.path().join("dest.txt");
    fs::write(&dest, "already here\n").unwrap();

    let err = CloneOut
        .materialize_file(&src, &dest)
        .expect_err("EEXIST expected");
    assert_eq!(err.raw_os_error(), Some(libc::EEXIST));
}

#[cfg(target_os = "macos")]
#[test]
fn clone_out_reports_invalid_input_instead_of_panicking_on_nameless_dest() {
    let base = tempfile::tempdir().expect("tempdir");
    let src = blob(base.path(), b"x\n");
    // `base/..` has no file_name: the old `.expect` panicked here.
    let dest = base.path().join("..");

    let err = CloneOut
        .materialize_file(&src, &dest)
        .expect_err("InvalidInput expected");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
}

#[test]
fn refusals_fall_back_but_permission_problems_stay_loud() {
    // Filesystem-level refusal: silent byte-copy fallback territory.
    for code in [
        libc::ENOTSUP,
        libc::EOPNOTSUPP,
        libc::ENOSYS,
        libc::EXDEV,
        libc::EPERM,
        libc::EMLINK,
    ] {
        let e = std::io::Error::from_raw_os_error(code);
        assert!(placement_refused(&e), "{code} should count as refused");
    }
    // Destination-side permission problems are real failures.
    for code in [libc::EACCES, libc::EROFS, libc::EEXIST] {
        let e = std::io::Error::from_raw_os_error(code);
        assert!(!placement_refused(&e), "{code} must stay loud");
    }
}

#[test]
fn materializer_force_byte_copy_places_file_and_reports_byte_copy() {
    let base = tempfile::tempdir().expect("tempdir");
    let src = blob(base.path(), b"hello byte copy\n");
    let dest = base.path().join("dest.txt");

    let materializer = Materializer::select(StrategyPolicy::ForceByteCopy, false, false, false);
    assert_eq!(materializer.strategy(), "byte-copy");
    assert!(materializer.backend().is_none());

    let outcome = materializer
        .materialize_file(&src, &dest, Some(0o755))
        .expect("materialize");
    assert_eq!(outcome.strategy, "byte-copy");
    assert!(!outcome.is_shared_cow);
    assert!(!outcome.is_mode_repaired);
    assert_eq!(fs::read(&dest).unwrap(), b"hello byte copy\n");

    let meta = fs::metadata(&dest).unwrap();
    assert_eq!(meta.permissions().mode() & 0o7777, 0o755);
    assert_eq!(meta.nlink(), 1);
}

#[test]
fn materializer_hardlink_shared_when_exec_matches_and_repairs_when_exec_differs() {
    let base = tempfile::tempdir().expect("tempdir");
    let materializer = Materializer::select(StrategyPolicy::Hardlink, false, false, false);
    assert_eq!(materializer.strategy(), "hardlink");

    // 1. Source is 0644 (not executable), target mode is 0644 (not executable) -> stays shared hardlink
    let src1 = base.path().join("blob1.bin");
    fs::write(&src1, b"shared text\n").expect("write blob1");
    let dest1 = base.path().join("dest1.txt");
    let outcome1 = materializer
        .materialize_file(&src1, &dest1, Some(0o644))
        .expect("materialize");
    assert_eq!(outcome1.strategy, "hardlink");
    assert!(outcome1.is_shared_cow);
    assert!(!outcome1.is_mode_repaired);
    assert_eq!(fs::metadata(&dest1).unwrap().nlink(), 2);

    // 2. Source is 0644 (not executable), target mode is 0755 (executable) -> repaired to private byte copy
    let src2 = base.path().join("blob2.bin");
    fs::write(&src2, b"exec script\n").expect("write blob2");
    let dest2 = base.path().join("dest2.sh");
    let outcome2 = materializer
        .materialize_file(&src2, &dest2, Some(0o755))
        .expect("materialize");
    assert_eq!(outcome2.strategy, "hardlink");
    assert!(!outcome2.is_shared_cow);
    assert!(outcome2.is_mode_repaired);
    let meta2 = fs::metadata(&dest2).unwrap();
    assert_eq!(meta2.nlink(), 1, "mode repair must create private copy");
    assert_eq!(meta2.permissions().mode() & 0o7777, 0o755);
    assert_eq!(fs::read(&dest2).unwrap(), b"exec script\n");
}

#[test]
fn materializer_creates_missing_parent_directories_automatically() {
    let base = tempfile::tempdir().expect("tempdir");
    let src = blob(base.path(), b"nested file content\n");
    let dest = base.path().join("a").join("b").join("c").join("deep.txt");

    let materializer = Materializer::select(StrategyPolicy::ForceByteCopy, false, false, false);
    let outcome = materializer
        .materialize_file(&src, &dest, Some(0o644))
        .expect("materialize with missing parents");

    assert_eq!(outcome.strategy, "byte-copy");
    assert!(dest.exists());
    assert_eq!(fs::read(&dest).unwrap(), b"nested file content\n");
}

#[test]
fn materializer_cross_device_selection_falls_back_to_byte_copy() {
    let base = tempfile::tempdir().expect("tempdir");
    let src = blob(base.path(), b"cross device data\n");
    let dest = base.path().join("dest.txt");

    let materializer = Materializer::new(
        StrategyPolicy::Default,
        base.path(),
        true,  // is_cross_device
        true,  // reflink_capable
        false, // is_ext4
    );
    assert_eq!(materializer.strategy(), "copy-on-write");
    assert!(materializer.backend().is_none());

    let outcome = materializer
        .materialize_file(&src, &dest, None)
        .expect("materialize");
    assert_eq!(outcome.strategy, "copy-on-write");
    assert!(!outcome.is_shared_cow);
    assert_eq!(fs::read(&dest).unwrap(), b"cross device data\n");
}
