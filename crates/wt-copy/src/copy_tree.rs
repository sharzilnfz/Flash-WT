//! Tree walker and atomic-placement wrapper shared by the per-file
//! backends (reflink, hardlink, deep copy) and by clonefile (ticket
//! 04).
//!
//! [`copy_tree`] walks `src` depth-first and rebuilds the same shape
//! under an existing `dest`: directories are created exclusively,
//! regular files are handed to `copy_file`, and symlinks are
//! recreated verbatim via [`std::fs::read_link`] — never followed
//! (trait contract).
//!
//! [`staged_copy`] is the placement contract every backend's
//! `copy_dir` goes through: materialize into a private
//! `<dest>.<pid>.tmp` staging tree, then rename it onto `dest` in one
//! step. On any error the staging tree is removed before returning,
//! so a failed copy never leaves a half-built destination that looks
//! trustworthy — "on Err, dest does not exist".

use std::fs;
use std::io;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use crate::{Error, Result};

/// Copy the tree at `src` into the not-yet-existing directory `dest`,
/// creating it exclusively.
pub(crate) fn copy_tree(
    src: &Path,
    dest: &Path,
    copy_file: &mut dyn FnMut(&Path, &Path) -> io::Result<()>,
) -> io::Result<()> {
    // Exclusive: two racing fills cannot silently merge into the same
    // paths.
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

/// All-or-nothing `copy_dir` plumbing shared by every backend.
///
/// `fill` must build the entire result tree rooted exactly at the
/// staging path it is handed, creating that root itself — walkers
/// exclusively in [`copy_tree`], `clonefile(2)` by syscall semantics.
/// On success the staging tree is renamed onto `dest`; on any error
/// it is removed first, so `dest` is either the complete copy or
/// absent.
pub(crate) fn staged_copy(
    dest: &Path,
    fill: &mut dyn FnMut(&Path) -> Result<()>,
) -> Result<()> {
    // Fast fail for the common case. The authoritative guard is the
    // final rename: a dest created between this check and the rename
    // can only be replaced atomically or make the rename fail — the
    // old check-then-copy TOCTOU window is gone.
    crate::ensure_dest_free(dest)?;

    let staging = staging_path(dest);
    let outcome = fill(&staging).and_then(|()| rename_staged(&staging, dest));
    if outcome.is_err() {
        // Contract: on Err, dest does not exist and nothing partial
        // survives. Best effort — this also sweeps up a crashed run's
        // leftover under a reused pid, so the next attempt succeeds.
        drop(fs::remove_dir_all(&staging));
    }
    outcome
}

/// `<dest>.<pid>.tmp` beside `dest`: unique per process, invisible to
/// anything not looking for our scratch names.
fn staging_path(dest: &Path) -> PathBuf {
    let mut name = match dest.file_name() {
        Some(n) => n.to_os_string(),
        None => std::ffi::OsString::new(),
    };
    name.push(format!(".{}.tmp", std::process::id()));
    dest.with_file_name(name)
}

/// The one step that makes the copy atomic. If `dest` appeared since
/// the pre-check, the kernel refuses (or replaces only atomically);
/// we surface that as the documented `DestinationExists`.
fn rename_staged(staging: &Path, dest: &Path) -> Result<()> {
    fs::rename(staging, dest).map_err(|e| match e.raw_os_error() {
        Some(libc::EEXIST | libc::ENOTEMPTY) => Error::DestinationExists,
        _ => Error::Io(e),
    })
}

/// Failure injection for the mid-copy test (ticket 04). Compiled only
/// under `cfg(test)`; production builds carry no hook.
///
/// Thread-local so parallel test threads cannot arm or consume each
/// other's injections.
#[cfg(test)]
pub(crate) mod test_hooks {
    use std::cell::Cell;
    use std::io;

    thread_local! {
        static FILES_UNTIL_FAILURE: Cell<usize> = const { Cell::new(usize::MAX) };
    }

    /// Fail the Nth regular-file copy of the current thread's next
    /// backend run.
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

    /// Mid-copy failure must leave no destination and no staging
    /// leftovers — through every runnable walker-based backend's real
    /// `copy_dir`, with the failure injected after two files have
    /// been copied.
    ///
    /// Clonefile has no middle to fail in: the whole tree lands in a
    /// single kernel call, so atomicity is the filesystem's job and
    /// the hook cannot reach it. It still gets the post-failure
    /// sanity copy below.
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

            // And the backend still works afterwards: no poisoned state.
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
