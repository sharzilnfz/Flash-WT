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

    #[cfg(target_os = "linux")]
    unsafe {
        libc::posix_fadvise(
            src.as_raw_fd(),
            0,
            meta.len() as libc::off_t,
            libc::POSIX_FADV_SEQUENTIAL,
        );
    }
    #[cfg(target_os = "macos")]
    unsafe {
        libc::fcntl(src.as_raw_fd(), libc::F_RDAHEAD, 1);
    }

    let mut dest = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(to)?;

    #[cfg(target_os = "linux")]
    unsafe {
        libc::posix_fadvise(
            dest.as_raw_fd(),
            0,
            meta.len() as libc::off_t,
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

/// Resolve the closest existing path by walking up ancestor directories if `path` does not exist.
pub(crate) fn find_existing_ancestor(path: &Path) -> &Path {
    if path.as_os_str().is_empty() {
        return Path::new(".");
    }
    let mut current = path;
    while !current.exists() {
        if let Some(parent) = current.parent() {
            if parent.as_os_str().is_empty() {
                return Path::new(".");
            }
            current = parent;
        } else {
            return Path::new(".");
        }
    }
    current
}

/// Resolve the device ID (`st_dev`) of `path`, falling back to its nearest existing ancestor.
pub(crate) fn probe_device_id(path: &Path) -> io::Result<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let existing = find_existing_ancestor(path);
        let meta = std::fs::metadata(existing)?;
        Ok(meta.dev())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(0)
    }
}

/// True if `src` and `dest` reside on different filesystem devices.
pub(crate) fn is_cross_device(src: &Path, dest: &Path) -> bool {
    match (probe_device_id(src), probe_device_id(dest)) {
        (Ok(src_dev), Ok(dest_dev)) if src_dev != dest_dev => {
            #[cfg(target_os = "linux")]
            {
                if is_btrfs_filesystem(src) && is_btrfs_filesystem(dest) {
                    return false;
                }
            }
            true
        }
        (Ok(_src_dev), Ok(_dest_dev)) => false,
        _ => false,
    }
}

#[cfg(target_os = "linux")]
fn is_btrfs_filesystem(path: &Path) -> bool {
    let existing = find_existing_ancestor(path);
    match statfs_of(existing) {
        Ok(st) => (st.f_type as libc::c_long) == (libc::BTRFS_SUPER_MAGIC as libc::c_long),
        Err(_) => false,
    }
}

/// Probe reflink and ext4 capabilities for the filesystem holding `path`.
pub(crate) fn probe_fs_capabilities(path: &Path) -> (bool, bool) {
    let existing = find_existing_ancestor(path);
    let Ok(st) = statfs_of(existing) else {
        return (false, false);
    };

    #[cfg(target_os = "macos")]
    {
        use std::ffi::CStr;
        let fstype = unsafe { CStr::from_ptr(st.f_fstypename.as_ptr()) };
        let is_apfs = fstype.to_string_lossy().eq_ignore_ascii_case("apfs");
        (is_apfs, false)
    }

    #[cfg(target_os = "linux")]
    {
        let f_type = st.f_type as libc::c_long;
        let is_reflink = f_type == (libc::BTRFS_SUPER_MAGIC as libc::c_long)
            || f_type == (libc::XFS_SUPER_MAGIC as libc::c_long);
        let is_ext4 = f_type == (libc::EXT4_SUPER_MAGIC as libc::c_long);
        (is_reflink, is_ext4)
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = st;
        (false, false)
    }
}

/// Recursively count regular files and total file size in bytes under `dir`.
pub(crate) fn count_files_and_bytes(dir: &Path) -> io::Result<(u64, u64)> {
    use std::fs;
    let mut files = 0u64;
    let mut bytes = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(path) = stack.pop() {
        for entry in fs::read_dir(&path)? {
            let entry = entry?;
            let ft = entry.file_type()?;
            if ft.is_dir() {
                stack.push(entry.path());
            } else if ft.is_file() {
                files += 1;
                bytes += entry.metadata()?.len();
            }
        }
    }
    Ok((files, bytes))
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
