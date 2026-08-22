//! Tree walker shared by the per-file backends (reflink, hardlink,
//! deep copy).
//!
//! Walks `src` depth-first and rebuilds the same shape under `dest`:
//! directories are created, regular files are handed to `copy_file`,
//! and symlinks are recreated verbatim via [`std::fs::read_link`] —
//! never followed (trait contract).

use std::fs;
use std::io;
use std::os::unix::fs::symlink;
use std::path::Path;

pub(crate) fn copy_tree(
    src: &Path,
    dest: &Path,
    copy_file: &mut dyn FnMut(&Path, &Path) -> io::Result<()>,
) -> io::Result<()> {
    fs::create_dir_all(dest)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let from = entry.path();
        let to = dest.join(entry.file_name());

        if file_type.is_symlink() {
            let target = fs::read_link(&from)?;
            symlink(target, &to)?;
        } else if file_type.is_dir() {
            copy_tree(&from, &to, copy_file)?;
        } else {
            copy_file(&from, &to)?;
        }
    }
    Ok(())
}
