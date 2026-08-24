//! Linux reflink backend (`FICLONE` ioctl on btrfs/XFS).
//!
//! Reflink is a per-file operation on Linux: the walker rebuilds the
//! tree and asks the filesystem to clone each file's extents, so
//! bytes stay shared copy-on-write. Permissions are applied
//! explicitly because `FICLONE` clones only data.

use std::fs;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use crate::copy_tree::copy_tree;
use crate::{BackendKind, CopyBackend, Error, Result};

#[derive(Debug, Default)]
pub struct ReflinkBackend;

/// Filesystem magics whose `statfs` reports reflink support.
const REFLINK_MAGICS: [libc::c_long; 2] = [
    libc::BTRFS_SUPER_MAGIC as libc::c_long,
    libc::XFS_SUPER_MAGIC as libc::c_long,
];

impl CopyBackend for ReflinkBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Reflink
    }

    /// True when `dir` sits on btrfs or XFS, the two mainline
    /// filesystems implementing `FICLONE`. Cheap and side-effect free.
    fn supports(&self, dir: &Path) -> bool {
        match crate::sys::statfs_of(dir) {
            Ok(st) => REFLINK_MAGICS.contains(&(st.f_type as libc::c_long)),
            Err(_) => false,
        }
    }

    fn copy_dir(&self, src: &Path, dest: &Path) -> Result<()> {
        crate::copy_tree::staged_copy(dest, self.safety(), &mut |staging| {
            let mut clone_file = |from: &Path, to: &Path| reflink_file(from, to).map_err(Error::Io);
            copy_tree(src, staging, &mut clone_file).map_err(Error::Io)
        })
    }
}

fn reflink_file(from: &Path, to: &Path) -> io::Result<()> {
    let src = fs::File::open(from)?;
    let mode = src.metadata()?.permissions().mode() & 0o7777;
    let dest = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(to)?;
    // SAFETY: both fds are open; FICLONE reads only from `src`.
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
    // Mode parity with fs::copy: OpenOptions' .mode() passes through
    // the umask, so a 0755 script would land as 0755 & !umask. Set
    // the final mode explicitly once FICLONE has succeeded.
    fs::set_permissions(to, fs::Permissions::from_mode(mode))
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests;
