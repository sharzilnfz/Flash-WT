use std::fs;
use std::io;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use crate::{Error, Result};

pub(crate) fn copy_tree(
    src: &Path,
    dest: &Path,
    copy_file: &mut dyn FnMut(&Path, &Path) -> io::Result<()>,
) -> io::Result<()> {
    fs::create_dir(dest)?;
    walk(src, dest, copy_file)
}

fn walk(
    src: &Path,
    dest: &Path,
    copy_file: &mut dyn FnMut(&Path, &Path) -> io::Result<()>,
) -> io::Result<()> {
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let from = entry.path();
        let to = dest.join(entry.file_name());

        if file_type.is_symlink() {
            let target = fs::read_link(&from)?;
            symlink(target, &to)?;
        } else if file_type.is_dir() {
            fs::create_dir(&to)?;
            walk(&from, &to, copy_file)?;
        } else if file_type.is_file() {
            #[cfg(test)]
            test_hooks::maybe_fail_injected()?;
            copy_file(&from, &to)?;
        } else {
            continue;
        }
    }
    Ok(())
}

pub(crate) fn staged_copy(dest: &Path, fill: &mut dyn FnMut(&Path) -> Result<()>) -> Result<()> {
    crate::ensure_dest_free(dest)?;

    let staging = staging_path(dest);
    let outcome = fill(&staging).and_then(|()| rename_staged(&staging, dest));
    if outcome.is_err() {
        drop(fs::remove_dir_all(&staging));
    }
    outcome
}

fn staging_path(dest: &Path) -> PathBuf {
    let mut name = match dest.file_name() {
        Some(n) => n.to_os_string(),
        None => std::ffi::OsString::new(),
    };
    name.push(format!(".{}.tmp", std::process::id()));
    dest.with_file_name(name)
}

fn rename_staged(staging: &Path, dest: &Path) -> Result<()> {
    fs::rename(staging, dest).map_err(|e| match e.raw_os_error() {
        Some(libc::EEXIST | libc::ENOTEMPTY) => Error::DestinationExists,
        _ => Error::Io(e),
    })
}

#[cfg(test)]
pub(crate) mod test_hooks {
    use std::cell::Cell;
    use std::io;

    thread_local! {
        static FILES_UNTIL_FAILURE: Cell<usize> = const { Cell::new(usize::MAX) };
    }

    pub fn arm_after(n: usize) {
        FILES_UNTIL_FAILURE.with(|c| c.set(n));
    }

    pub fn disarm() {
        FILES_UNTIL_FAILURE.with(|c| c.set(usize::MAX));
    }

    pub fn maybe_fail_injected() -> io::Result<()> {
        FILES_UNTIL_FAILURE.with(|c| match c.get() {
            usize::MAX => Ok(()),
            1 => {
                c.set(usize::MAX);
                Err(io::Error::other("injected mid-copy failure"))
            }
            n => {
                c.set(n - 1);
                Ok(())
            }
        })
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BackendKind, candidates};
    use std::fs;

    fn fixture(base: &Path) -> PathBuf {
        let src = base.join("src");
        fs::create_dir_all(src.join("a/b")).expect("mkdir");
        for i in 0..4 {
            fs::write(src.join(format!("f{i}.txt")), format!("bytes {i}\n")).expect("write");
        }
        fs::write(src.join("a/b/deep.txt"), "deep\n").expect("write");
        src
    }

    fn runnable(base: &Path) -> Vec<Box<dyn crate::CopyBackend>> {
        candidates()
            .into_iter()
            .filter(|b| b.supports(base))
            .collect()
    }

    #[test]
    fn mid_copy_failure_leaves_no_dest_and_no_staging_leftovers() {
        let dir = tempfile::tempdir().expect("tempdir");
        let base = dir.path();
        let src = fixture(base);

        for backend in runnable(base) {
            let dest = base.join(format!("dest-{:?}", backend.kind()));

            if backend.kind() == BackendKind::Clonefile {
                backend.copy_dir(&src, &dest).expect("clonefile");
                assert!(dest.join("a/b/deep.txt").exists());
                let _ = fs::remove_dir_all(&dest);
                continue;
            }

            test_hooks::arm_after(2);
            let result = backend.copy_dir(&src, &dest);
            test_hooks::disarm();

            assert!(result.is_err(), "{:?} should have failed", backend.kind());
            assert!(!dest.exists(), "{:?} left a dest behind", backend.kind());
            assert!(
                !dest.parent().map(staging_leftovers_for).unwrap_or_default(),
                "{:?} left staging leftovers",
                backend.kind()
            );

            backend.copy_dir(&src, &dest).expect("retry");
            assert!(dest.join("a/b/deep.txt").exists());
            let _ = fs::remove_dir_all(&dest);
        }
    }

    fn staging_leftovers_for(parent: &Path) -> bool {
        fs::read_dir(parent)
            .expect("read parent")
            .filter_map(|e| e.ok())
            .any(|e| {
                let n = e.file_name().to_string_lossy().into_owned();
                n.starts_with("dest-") && n.ends_with(".tmp")
            })
    }
}
