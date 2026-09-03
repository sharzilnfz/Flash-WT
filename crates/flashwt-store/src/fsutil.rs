use std::fs;
use std::io::{self, Write};
use std::path::Path;

#[inline]
pub(crate) fn is_sync_disabled() -> bool {
    if let Some(val) = std::env::var_os("FLASHWT_TEST_NO_SYNC") {
        let s = val.to_string_lossy();
        if s == "0" || s.eq_ignore_ascii_case("false") {
            return false;
        }
        return true;
    }
    if let Some(val) = std::env::var_os("FLASHWT_NO_SYNC") {
        let s = val.to_string_lossy();
        if s == "0" || s.eq_ignore_ascii_case("false") {
            return false;
        }
        return true;
    }
    if std::env::var_os("FLASHWT_FORCE_SYNC").is_some()
        || std::env::var_os("FLASHWT_TEST_FORCE_SYNC").is_some()
    {
        return false;
    }
    if cfg!(test) {
        return true;
    }
    false
}

pub(crate) fn durable_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = fs::File::create(path)?;
    file.write_all(bytes)?;
    if !is_sync_disabled() {
        file.sync_all()?;
    }
    drop(file);
    sync_parent_dir(path)
}

pub(crate) fn durable_write_then_rename(tmp_path: &Path, final_path: &Path) -> io::Result<()> {
    if !is_sync_disabled() {
        let file = fs::File::open(tmp_path)?;
        file.sync_all()?;
        drop(file);
    }
    fs::rename(tmp_path, final_path)?;
    sync_parent_dir(final_path)
}

pub(crate) fn sync_dir(dir_path: &Path) -> io::Result<()> {
    if is_sync_disabled() {
        return Ok(());
    }
    let dir = fs::File::open(dir_path)?;
    dir.sync_all()
}

pub(crate) fn sync_parent_dir(path: &Path) -> io::Result<()> {
    sync_dir(path.parent().unwrap_or_else(|| Path::new(".")))
}

use std::fs::File;
use std::os::unix::io::AsRawFd;

#[derive(Debug)]
pub struct FlockGuard {
    file: File,
}

impl FlockGuard {
    pub fn lock_exclusive(file: File) -> io::Result<Self> {
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { file })
    }

    pub fn lock_shared(file: File) -> io::Result<Self> {
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { file })
    }

    pub fn lock_file_exclusive(path: &Path) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        Self::lock_exclusive(file)
    }
}

impl Drop for FlockGuard {
    fn drop(&mut self) {
        unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
    }
}

pub fn measure_tree_size(path: &Path) -> u64 {
    let mut total = 0;
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if meta.is_file() {
            total += meta.len();
        } else if meta.is_dir() {
            if let Ok(entries) = std::fs::read_dir(path) {
                for entry in entries.flatten() {
                    total += measure_tree_size(&entry.path());
                }
            }
        }
    }
    total
}
