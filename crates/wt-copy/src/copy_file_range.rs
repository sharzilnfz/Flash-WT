//! Linux copy_file_range backend for in-kernel zero-copy page splicing on ext4.

use std::fs;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use crate::copy_tree::copy_tree;
use crate::{BackendKind, CopyBackend, Error, Result};

/// Copy-file-range backend for Linux ext4 page splicing without FICLONE overhead.
#[derive(Debug, Default)]
pub struct CopyFileRangeBackend;

impl CopyBackend for CopyFileRangeBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::CopyFileRange
    }

    fn supports(&self, dir: &Path) -> bool {
        match crate::sys::statfs_of(dir) {
            Ok(st) => (st.f_type as libc::c_long) == (libc::EXT4_SUPER_MAGIC as libc::c_long),
            Err(_) => false,
        }
    }

    fn copy_dir(&self, src: &Path, dest: &Path) -> Result<()> {
        crate::copy_tree::staged_copy(dest, self.safety(), &mut |staging| {
            let mut clone_file = |from: &Path, to: &Path| copy_file_range_file(from, to);
            copy_tree(src, staging, &mut clone_file).map_err(Error::Io)
        })
    }
}

/// Copy `from` to `to` using the Linux `copy_file_range(2)` syscall.
pub fn copy_file_range_file(from: &Path, to: &Path) -> io::Result<()> {
    let src = fs::File::open(from)?;
    let meta = src.metadata()?;
    let mode = meta.permissions().mode() & 0o7777;
    let len = meta.len();
    let dest = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(to)?;

    let mut remaining = len as usize;
    let mut off_in: libc::loff_t = 0;
    let mut off_out: libc::loff_t = 0;

    while remaining > 0 {
        let chunk = remaining.min(1024 * 1024 * 1024);
        let rc = unsafe {
            libc::copy_file_range(
                src.as_raw_fd(),
                &mut off_in,
                dest.as_raw_fd(),
                &mut off_out,
                chunk,
                0,
            )
        };
        if rc < 0 {
            let err = io::Error::last_os_error();
            drop(fs::remove_file(to));
            return Err(err);
        }
        if rc == 0 {
            break;
        }
        remaining -= rc as usize;
    }
    fs::set_permissions(to, fs::Permissions::from_mode(mode))
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_file_range_copies_file_contents_and_mode() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("src.txt");
        let dest = dir.path().join("dest.txt");
        fs::write(&src, "ext4 copy file range test\n").expect("write");
        fs::set_permissions(&src, fs::Permissions::from_mode(0o755)).expect("chmod");

        match copy_file_range_file(&src, &dest) {
            Ok(()) => {
                assert_eq!(
                    fs::read_to_string(&dest).expect("read"),
                    "ext4 copy file range test\n"
                );
                let mode = fs::metadata(&dest).expect("meta").permissions().mode();
                assert_eq!(mode & 0o7777, 0o755);
            }
            Err(e)
                if e.raw_os_error() == Some(libc::ENOSYS)
                    || e.raw_os_error() == Some(libc::EXDEV)
                    || e.raw_os_error() == Some(libc::EOPNOTSUPP) =>
            {
                // Unsupported environment in container/emulation
            }
            Err(e) => panic!("unexpected error: {e}"),
        }
    }
}
