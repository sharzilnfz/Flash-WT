//! APFS whole-directory `clonefile(2)` backend (macOS).
//!
//! One syscall clones the entire tree — metadata, permissions, and
//! symlinks included — as copy-on-write blocks. This is the fastest
//! safe mechanism in the project and the reason macOS is the primary
//! target.

use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use crate::{BackendKind, CopyBackend, Error, Result};

/// APFS `clonefile(2)` backend (macOS only).
#[derive(Debug, Default)]
pub struct ClonefileBackend;

impl CopyBackend for ClonefileBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Clonefile
    }

    /// True when `dir` sits on APFS (`statfs(2)` reports the
    /// `apfs` filesystem type). Cheap and side-effect free.
    fn supports(&self, dir: &Path) -> bool {
        fstype_is(dir, b"apfs")
    }

    fn copy_dir(&self, src: &Path, dest: &Path) -> Result<()> {
        crate::copy_tree::staged_copy(dest, self.safety(), &mut |staging| {
            let c_src = c_path(src)?;
            let c_staging = c_path(staging)?;

            // The syscall creates the staging root itself (and fails
            // EEXIST if it exists), so the whole-tree clone stays a
            // single atomic kernel call; `staged_copy` then renames it
            // onto `dest`.
            //
            // SAFETY: both pointers are valid NUL-terminated paths that
            // outlive the call; clonefile keeps none of them.
            let rc = unsafe { libc::clonefile(c_src.as_ptr(), c_staging.as_ptr(), 0) };
            if rc == 0 {
                return Ok(());
            }
            match io::Error::last_os_error().raw_os_error() {
                Some(libc::ENOTSUP) | Some(libc::ENOSYS) | Some(libc::EOPNOTSUPP) => {
                    Err(Error::Unsupported)
                }
                _ => Err(io::Error::last_os_error().into()),
            }
        })
    }
}

/// True if the filesystem holding `path` has the given `statfs`
/// type name (for example `apfs`). A pure predicate over
/// [`crate::sys::statfs_of`].
fn fstype_is(path: &Path, want: &[u8]) -> bool {
    let Ok(st) = crate::sys::statfs_of(path) else {
        return false;
    };
    let name: &[libc::c_char] = &st.f_fstypename;
    name.get(want.len()) == Some(&0) && name.iter().zip(want).all(|(a, b)| *a as u8 == *b)
}

fn c_path(path: &Path) -> io::Result<CString> {
    CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL byte"))
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests;
