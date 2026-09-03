use std::fs;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use crate::copy_tree::copy_tree;
use crate::{BackendKind, CopyBackend, Error, Result};

#[derive(Debug, Default)]
pub struct ReflinkBackend;

const REFLINK_MAGICS: [libc::c_long; 2] = [
    libc::BTRFS_SUPER_MAGIC as libc::c_long,
    libc::XFS_SUPER_MAGIC as libc::c_long,
];

impl CopyBackend for ReflinkBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Reflink
    }

    fn supports(&self, dir: &Path) -> bool {
        match crate::sys::statfs_of(dir) {
            Ok(st) => REFLINK_MAGICS.contains(&(st.f_type as libc::c_long)),
            Err(_) => false,
        }
    }

    fn copy_dir(&self, src: &Path, dest: &Path) -> Result<()> {
        crate::copy_tree::staged_copy(dest, &mut |staging| {
            let mut clone_file = |from: &Path, to: &Path| reflink_file(from, to);
            copy_tree(src, staging, &mut clone_file).map_err(Error::Io)
        })
    }
}

pub fn reflink_file(from: &Path, to: &Path) -> io::Result<()> {
    let src = fs::File::open(from)?;
    let mode = src.metadata()?.permissions().mode() & 0o7777;
    let dest = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(to)?;

    let rc = unsafe {
        libc::ioctl(
            dest.as_raw_fd(),
            libc::FICLONE as libc::c_ulong,
            src.as_raw_fd(),
        )
    };
    if rc != 0 {
        let err = io::Error::last_os_error();
        drop(fs::remove_file(to));
        return Err(err);
    }

    fs::set_permissions(to, fs::Permissions::from_mode(mode))
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests;
