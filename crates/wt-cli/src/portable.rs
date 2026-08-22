//! Portable last-resort [`CopyBackend`]: plain byte-by-byte recursion.
//!
//! Ticket 03 owns the fast platform backends (clonefile, reflink,
//! hardlink); until those land, selection falls through to this one so
//! hydration works everywhere today.

use std::fs;
use std::path::Path;

use wt_copy::{BackendKind, CopyBackend, Error, Result};

#[derive(Debug, Default)]
pub struct PortableDeepCopy;

impl CopyBackend for PortableDeepCopy {
    fn kind(&self) -> BackendKind {
        BackendKind::DeepCopy
    }

    fn supports(&self, _dir: &Path) -> bool {
        true
    }

    fn copy_dir(&self, src: &Path, dest: &Path) -> Result<()> {
        if dest.exists() {
            return Err(Error::DestinationExists);
        }
        copy_tree(src, dest)?;
        Ok(())
    }
}

fn copy_tree(src: &Path, dest: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dest)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dest.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            recreate_symlink(&from, &to);
        } else if file_type.is_dir() {
            copy_tree(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

fn recreate_symlink(from: &Path, to: &Path) {
    #[cfg(unix)]
    {
        if let Ok(target) = fs::read_link(from) {
            let _ = std::os::unix::fs::symlink(target, to);
        }
    }
    #[cfg(not(unix))]
    let _ = (from, to);
}
