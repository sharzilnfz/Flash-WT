use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

pub(crate) fn statfs_of(path: &Path) -> io::Result<libc::statfs> {
    let c_path = c_path(path)?;

    let mut st: libc::statfs = unsafe { std::mem::zeroed() };

    if unsafe { libc::statfs(c_path.as_ptr(), &mut st) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(st)
}

#[cfg(target_os = "linux")]
pub(crate) fn read_only(path: &Path) -> io::Result<bool> {
    let c_path = c_path(path)?;

    let mut vfs: libc::statvfs = unsafe { std::mem::zeroed() };

    if unsafe { libc::statvfs(c_path.as_ptr(), &mut vfs) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(vfs.f_flag & libc::ST_RDONLY != 0)
}

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

    let copied = io::copy(&mut src, &mut dest)?;
    fs::set_permissions(to, fs::Permissions::from_mode(mode))?;
    Ok(copied)
}

pub(crate) fn c_path(path: &Path) -> io::Result<CString> {
    CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL byte"))
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FsCapabilities {
    pub apfs_clonefile: bool,

    pub ficlone: bool,

    pub copy_file_range: bool,
}

pub fn probe_capabilities(path: &Path) -> FsCapabilities {
    #[allow(unused_variables)]
    let (is_primary, is_secondary) = probe_fs_capabilities(path);

    #[cfg(target_os = "macos")]
    {
        FsCapabilities {
            apfs_clonefile: is_primary,
            ficlone: false,
            copy_file_range: false,
        }
    }

    #[cfg(target_os = "linux")]
    {
        FsCapabilities {
            apfs_clonefile: false,
            ficlone: is_primary,
            copy_file_range: is_secondary,
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (is_primary, is_secondary);
        FsCapabilities {
            apfs_clonefile: false,
            ficlone: false,
            copy_file_range: false,
        }
    }
}

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

pub(crate) fn filesystem_name(path: &Path) -> String {
    let existing = find_existing_ancestor(path);
    let Ok(st) = statfs_of(existing) else {
        return "unknown".to_string();
    };

    #[cfg(target_os = "macos")]
    {
        use std::ffi::CStr;
        let fstype = unsafe { CStr::from_ptr(st.f_fstypename.as_ptr()) };
        fstype.to_string_lossy().into_owned()
    }

    #[cfg(target_os = "linux")]
    {
        let f_type = st.f_type as libc::c_long;
        match f_type {
            x if x == (libc::EXT4_SUPER_MAGIC as libc::c_long) => "ext4".to_string(),
            x if x == (libc::BTRFS_SUPER_MAGIC as libc::c_long) => "btrfs".to_string(),
            x if x == (libc::XFS_SUPER_MAGIC as libc::c_long) => "xfs".to_string(),
            0x01021994 => "tmpfs".to_string(),
            0x6969 => "nfs".to_string(),
            0x794c7630 => "overlayfs".to_string(),
            0x65735546 => "fuse".to_string(),
            0x4d44 => "vfat".to_string(),
            _ => format!("unknown (0x{f_type:x})"),
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = st;
        "unknown".to_string()
    }
}

pub fn refusal_reason_for_errno(errno: i32) -> String {
    #[cfg(unix)]
    {
        match errno {
            libc::EXDEV => "cross-device link (EXDEV)".to_string(),
            code if code == libc::ENOTSUP || code == libc::EOPNOTSUPP => {
                "filesystem does not support operation (ENOTSUP/EOPNOTSUPP)".to_string()
            }
            libc::ENOSYS => "syscall not implemented by kernel (ENOSYS)".to_string(),
            libc::EMLINK => "maximum link count reached (EMLINK)".to_string(),
            libc::EPERM => "operation not permitted by filesystem (EPERM)".to_string(),
            _ => {
                let err = io::Error::from_raw_os_error(errno);
                format!("placement refused: {err} (errno {errno})")
            }
        }
    }
    #[cfg(not(unix))]
    {
        let err = io::Error::from_raw_os_error(errno);
        format!("placement refused: {err} (errno {errno})")
    }
}

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
