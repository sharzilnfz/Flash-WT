//! The single `statfs(2)` probe behind every backend's filesystem
//! predicate (ticket 04).
//!
//! Before this module, clonefile, reflink, and hardlink each carried
//! a near-duplicate wrapper repeating the CString / zeroed-struct /
//! SAFETY boilerplate. The unsafe FFI now lives here alone; the
//! backends keep pure predicate functions over its results.

use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// `statfs(2)` against the filesystem holding `path`.
///
/// SAFETY (the two blocks below): `c_path` is a valid NUL-terminated
/// path that outlives the call; `st` is a correctly sized allocation
/// owned by this call, initialized to zero because `statfs` fills
/// only the fields it knows and we read only those. The kernel writes
/// at most `size_of::<libc::statfs>()` bytes into it and keeps no
/// pointer. `libc` offers no safe binding for this syscall.
pub(crate) fn statfs_of(path: &Path) -> io::Result<libc::statfs> {
    let c_path = c_path(path)?;
    // SAFETY: see the function-level contract above.
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: see the function-level contract above.
    if unsafe { libc::statfs(c_path.as_ptr(), &mut st) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(st)
}

/// Whether the filesystem holding `path` is mounted read-only.
///
/// Linux `statfs` carries no flags field, so this probes `statvfs(3)`
/// instead.
///
/// SAFETY: same shape as [`statfs_of`]: valid NUL-terminated path
/// that outlives the call, correctly sized caller-owned buffer.
#[cfg(target_os = "linux")]
pub(crate) fn read_only(path: &Path) -> io::Result<bool> {
    let c_path = c_path(path)?;
    // SAFETY: see the function-level contract above.
    let mut vfs: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: see the function-level contract above.
    if unsafe { libc::statvfs(c_path.as_ptr(), &mut vfs) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(vfs.f_flag & libc::ST_RDONLY != 0)
}

/// Fallback parallel buffered copy with `posix_fadvise(POSIX_FADV_SEQUENTIAL)`.
pub fn buffered_copy_file(from: &Path, to: &Path) -> io::Result<u64> {
    use std::fs;
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let mut src = fs::File::open(from)?;
    let meta = src.metadata()?;
    let mode = meta.permissions().mode() & 0o7777;
    let len = meta.len();

    #[cfg(unix)]
    unsafe {
        libc::posix_fadvise(
            src.as_raw_fd(),
            0,
            len as libc::off_t,
            libc::POSIX_FADV_SEQUENTIAL,
        );
    }

    let mut dest = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(to)?;

    #[cfg(unix)]
    unsafe {
        libc::posix_fadvise(
            dest.as_raw_fd(),
            0,
            len as libc::off_t,
            libc::POSIX_FADV_SEQUENTIAL,
        );
    }

    let mut buf = vec![0u8; 128 * 1024];
    let mut copied = 0u64;
    loop {
        let n = io::Read::read(&mut src, &mut buf)?;
        if n == 0 {
            break;
        }
        io::Write::write_all(&mut dest, &buf[..n])?;
        copied += n as u64;
    }
    fs::set_permissions(to, fs::Permissions::from_mode(mode))?;
    Ok(copied)
}

fn c_path(path: &Path) -> io::Result<CString> {
    CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL byte"))
}

// Tests assert with unwrap/expect by design: a panic IS the failure
// signal under test, so the workspace restriction lints stay off here.
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod tests {
    use super::*;

    #[test]
    fn probes_a_real_directory_and_fails_for_missing_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        let st = statfs_of(dir.path()).expect("statfs of tempdir");
        assert!(st.f_bsize > 0, "statfs must report a block size");

        #[cfg(target_os = "linux")]
        // Ordinary tempdirs are not mounted read-only.
        assert!(!read_only(dir.path()).expect("statvfs"));

        let err = statfs_of(Path::new("/definitely/not/here")).expect_err("ENOENT expected");
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn buffered_copy_file_copies_bytes_and_preserves_mode() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("src.txt");
        let dest = dir.path().join("dest.txt");
        fs::write(&src, "sequential buffered bytes\n").expect("write");
        fs::set_permissions(&src, fs::Permissions::from_mode(0o755)).expect("chmod");

        let bytes = buffered_copy_file(&src, &dest).expect("buffered copy");
        assert_eq!(bytes, 26);
        assert_eq!(
            fs::read_to_string(&dest).expect("read"),
            "sequential buffered bytes\n"
        );
        let mode = fs::metadata(&dest).expect("meta").permissions().mode();
        assert_eq!(mode & 0o7777, 0o755);
    }
}
