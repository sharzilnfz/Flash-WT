//! File-shaped materialization (fast-hydration ticket 03).
//!
//! [`CopyBackend`](crate::CopyBackend) is directory-shaped: clone or
//! walk a whole source tree into a fresh destination tree.
//! Store-backed materialization is file-shaped: one stored blob at
//! one content address becomes one fresh destination file. This
//! module defines that interface and its implementations:
//!
//! - [`HardlinkOut`] links the blob into place and strips write bits
//!   from the shared inode (the ticket 07 safety contract). No longer
//!   the default; callers opt in explicitly.
//! - [`CloneOut`] (macOS) asks the kernel to clone the blob's
//!   contents into a NEW destination file via `fclonefileat(2)`. The
//!   destination gets its own private inode sharing the blob's
//!   physical blocks until first write, and carries normal writable
//!   permissions.

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// A strategy for placing stored content at one destination path.
///
/// Contract for implementors:
///
/// - `src` is an existing regular file (a store blob); `dest` must
///   not exist. Neither path is followed through symlinks created
///   after the call starts; the caller owns directory setup.
/// - Errors are returned with their raw OS error intact. Classifying
///   them — "this filesystem cannot do that, fall back" versus "real
///   failure, stay loud" — is the caller's job, because the answer
///   depends on which fallbacks remain.
pub trait FileMaterialize {
    /// Stable displayable strategy name for CLI reporting.
    fn name(&self) -> &'static str;

    /// Place the contents of `src` at the not-yet-existing `dest`.
    fn materialize_file(&self, src: &Path, dest: &Path) -> io::Result<()>;
}

/// Hard-link the blob into place, then strip the write bits.
///
/// Shared-inode safety lives here rather than in the store: an
/// in-place rewrite of a linked copy would corrupt every other tree
/// and the store itself (the pnpm lesson, ticket 07). Replacement-
/// style writes (rename-over, unlink plus recreate) break the share
/// and stay private.
#[derive(Debug, Default)]
pub struct HardlinkOut;

impl FileMaterialize for HardlinkOut {
    fn name(&self) -> &'static str {
        "hardlink"
    }

    fn materialize_file(&self, src: &Path, dest: &Path) -> io::Result<()> {
        fs::hard_link(src, dest)?;
        let mut perms = fs::metadata(dest)?.permissions();
        perms.set_mode(perms.mode() & !0o222);
        fs::set_permissions(dest, perms)
    }
}

/// Per-file copy-on-write clone via macOS `fclonefileat(2)`.
///
/// The syscall takes an already-open source descriptor, the
/// destination directory, and the destination name: the store blob
/// becomes `srcfd`; the fresh tree supplies the rest. The kernel
/// copies nothing eagerly — the new file shares the blob's physical
/// blocks until either side writes, then diverges privately.
#[cfg(target_os = "macos")]
#[derive(Debug, Default)]
pub struct CloneOut;

#[cfg(target_os = "macos")]
impl FileMaterialize for CloneOut {
    fn name(&self) -> &'static str {
        "copy-on-write"
    }

    fn materialize_file(&self, src: &Path, dest: &Path) -> io::Result<()> {
        use std::ffi::CString;
        use std::os::fd::AsRawFd;
        use std::os::unix::ffi::OsStrExt;

        let blob = fs::File::open(src)?;
        let dir = fs::File::open(dest.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "destination has no parent")
        })?)?;
        let name = dest.file_name().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "destination has no name")
        })?;
        let name = CString::new(name.as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "name contains NUL byte"))?;

        // Flags are 0: CLONE_NOFOLLOW and friends guard against
        // symlinked source PATHS, but the source here is a descriptor
        // this function opened itself on a known-regular store blob.
        //
        // SAFETY: both descriptors are open for the duration of the
        // call; `name` is a valid NUL-terminated buffer that outlives
        // it. fclonefileat keeps none of the pointers.
        let rc = unsafe { libc::fclonefileat(blob.as_raw_fd(), dir.as_raw_fd(), name.as_ptr(), 0) };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }

        // fclonefileat clones the source's attributes, so the
        // destination inherits the blob's permissions. Store blobs are
        // normalized to `default_file_mode()` at put time
        // (fast-hydration ticket 05), which is exactly what a plain
        // byte copy would have produced — no per-file chmod here. A
        // store written by an older version still holds 0600 blobs;
        // their clones are owner-rw-only until those blobs are
        // re-ingested. Accepted and documented: one syscall per NEW
        // blob beats one per hydrated file.
        Ok(())
    }
}

/// Placement errors that mean "this filesystem cannot do that" and
/// deserve a silent byte-copy fallback rather than a failure: no
/// kernel/fs support, cross-device placement (store and worktree on
/// different volumes), link-count exhaustion, or fs-level refusal.
///
/// Permission problems on the destination (`EACCES`, `EROFS`) are
/// real failures and stay loud.
#[cfg(unix)]
pub fn placement_refused(e: &io::Error) -> bool {
    matches!(
        e.raw_os_error(),
        Some(code)
            if code == libc::EPERM
                || code == libc::EXDEV
                || code == libc::EMLINK
                || code == libc::ENOTSUP
                || code == libc::EOPNOTSUPP
                || code == libc::ENOSYS
    )
}

#[cfg(not(unix))]
pub fn placement_refused(_e: &io::Error) -> bool {
    true
}

#[cfg(test)]
mod tests;
