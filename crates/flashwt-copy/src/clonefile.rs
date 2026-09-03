use std::io;
use std::path::Path;

use crate::{BackendKind, CopyBackend, Error, Result};

#[derive(Debug, Default)]
pub struct ClonefileBackend;

impl CopyBackend for ClonefileBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Clonefile
    }

    fn supports(&self, dir: &Path) -> bool {
        let (is_apfs, _) = crate::sys::probe_fs_capabilities(dir);
        is_apfs
    }

    fn copy_dir(&self, src: &Path, dest: &Path) -> Result<()> {
        crate::copy_tree::staged_copy(dest, &mut |staging| {
            let c_src = crate::sys::c_path(src)?;
            let c_staging = crate::sys::c_path(staging)?;

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

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests;
