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
pub trait FileMaterialize: Send + Sync {
    /// Stable displayable strategy name for CLI reporting.
    fn name(&self) -> &'static str;

    /// Place the contents of `src` at the not-yet-existing `dest`.
    fn materialize_file(&self, src: &Path, dest: &Path) -> io::Result<()>;

    /// Whether a successful placement leaves `dest` sharing one inode
    /// with the source blob (hardlink), as opposed to owning a private
    /// inode (clone, byte copy). A caller that applies per-file modes
    /// after placement must not chmod a shared inode — permissions
    /// live on the inode, so the store blob and every sibling tree
    /// would change too. Such callers treat `true` as "replace with a
    /// private copy before applying any mode the link cannot carry".
    fn shares_inode_with_source(&self) -> bool {
        false
    }
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

    fn shares_inode_with_source(&self) -> bool {
        true
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
        // (fast-hydration ticket 05). Any recorded source mode is the
        // CALLER's business: the clone owns a private inode
        // (`shares_inode_with_source` is false), so a post-placement
        // chmod never reaches back into the store.
        Ok(())
    }
}

/// Per-file copy-on-write clone via Linux `ioctl(FICLONE)` on btrfs/XFS.
#[cfg(target_os = "linux")]
#[derive(Debug, Default)]
pub struct ReflinkOut;

#[cfg(target_os = "linux")]
impl FileMaterialize for ReflinkOut {
    fn name(&self) -> &'static str {
        "reflink"
    }

    fn materialize_file(&self, src: &Path, dest: &Path) -> io::Result<()> {
        crate::reflink::reflink_file(src, dest)
    }
}

/// Per-file page-splicing copy via Linux `copy_file_range(2)` on ext4.
#[cfg(target_os = "linux")]
#[derive(Debug, Default)]
pub struct CopyFileRangeOut;

#[cfg(target_os = "linux")]
impl FileMaterialize for CopyFileRangeOut {
    fn name(&self) -> &'static str {
        "copy_file_range"
    }

    fn materialize_file(&self, src: &Path, dest: &Path) -> io::Result<()> {
        crate::copy_file_range::copy_file_range_file(src, dest)
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

use crate::sys::buffered_copy_file;

/// Placement strategy policy controlling how blobs are materialized.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum StrategyPolicy {
    /// Default: per-file CoW clones where the filesystem supports
    /// them (APFS clonefile, Linux reflink, or copy_file_range), byte copies elsewhere.
    #[default]
    Default,
    /// Experimental hardlinked materialization: linked inodes are made
    /// read-only, so in-place rewrites fail loudly.
    Hardlink,
    /// Forced byte copies — the portable fallback.
    ForceByteCopy,
}

/// The result and diagnostics of placing one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlacementOutcome {
    /// The display name of the strategy attempted or used.
    pub strategy: &'static str,
    /// Whether the file is sharing storage (CoW clone / shared hardlink inode).
    pub is_shared_cow: bool,
    /// Whether the file required a permission mode repair (e.g. replacing a shared hardlink
    /// with a private byte copy because executable bits differed from target mode).
    pub is_mode_repaired: bool,
}

/// Encapsulated file materializer that handles backend selection,
/// fallback placement, directory repair on ENOENT, and permission mode normalization.
pub struct Materializer {
    backend: Option<Box<dyn FileMaterialize>>,
    strategy: &'static str,
}

impl Materializer {
    /// Create a materializer for the given source and destination paths, automatically
    /// inspecting device IDs and filesystem capabilities.
    pub fn for_paths(policy: StrategyPolicy, src_root: &Path, dest_root: &Path) -> Self {
        let is_cross = crate::sys::is_cross_device(src_root, dest_root);
        let (reflink_capable, is_ext4) = crate::sys::probe_fs_capabilities(dest_root);
        Self::select(policy, is_cross, reflink_capable, is_ext4)
    }

    /// Select a materializer based on strategy policy and filesystem capabilities.
    pub fn select(
        policy: StrategyPolicy,
        is_cross_device: bool,
        reflink_capable: bool,
        is_ext4: bool,
    ) -> Self {
        let (backend, strategy): (Option<Box<dyn FileMaterialize>>, &'static str) = match policy {
            StrategyPolicy::ForceByteCopy => (None, "byte-copy"),
            StrategyPolicy::Hardlink => (Some(Box::new(HardlinkOut)), "hardlink"),
            StrategyPolicy::Default => {
                #[cfg(target_os = "macos")]
                {
                    let _ = is_ext4;
                    if !is_cross_device && reflink_capable {
                        (Some(Box::new(CloneOut)), "copy-on-write")
                    } else {
                        (None, "copy-on-write")
                    }
                }

                #[cfg(target_os = "linux")]
                {
                    if !is_cross_device && reflink_capable {
                        (Some(Box::new(ReflinkOut)), "reflink")
                    } else if !is_cross_device && is_ext4 {
                        (Some(Box::new(CopyFileRangeOut)), "copy_file_range")
                    } else {
                        (None, "copy-on-write")
                    }
                }

                #[cfg(not(any(target_os = "macos", target_os = "linux")))]
                {
                    let _ = (is_cross_device, reflink_capable, is_ext4);
                    (None, "copy-on-write")
                }
            }
        };

        Self { backend, strategy }
    }

    /// The display name of the strategy this materializer attempts.
    pub fn strategy(&self) -> &'static str {
        self.strategy
    }

    /// The inner backend used for placement, if any.
    pub fn backend(&self) -> Option<&dyn FileMaterialize> {
        self.backend.as_deref()
    }

    /// Place one file at `dest` from `src`, creating parent directories if missing,
    /// falling back to byte copy if placement is refused, and normalizing permission `mode`.
    pub fn materialize_file(
        &self,
        src: &Path,
        dest: &Path,
        mode: Option<u32>,
    ) -> io::Result<PlacementOutcome> {
        let placed = match self.place_once(src, dest) {
            Ok(placed) => placed,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent)?;
                    self.place_once(src, dest)?
                } else {
                    return Err(e);
                }
            }
            Err(e) => return Err(e),
        };

        let mut is_mode_repaired = false;
        if let Some(target_mode) = mode {
            let shared_inode = placed
                && self
                    .backend
                    .as_deref()
                    .is_some_and(|b| b.shares_inode_with_source());
            is_mode_repaired = self.finalize_mode(shared_inode, target_mode, src, dest)?;
        }

        let is_shared_cow = placed && !is_mode_repaired;

        Ok(PlacementOutcome {
            strategy: self.strategy,
            is_shared_cow,
            is_mode_repaired,
        })
    }

    fn place_once(&self, src: &Path, dest: &Path) -> io::Result<bool> {
        if dest.exists() || dest.is_symlink() {
            let _ = fs::remove_file(dest);
        }
        if let Some(backend) = &self.backend {
            match backend.materialize_file(src, dest) {
                Ok(()) => return Ok(true),
                Err(e) if placement_refused(&e) => {}
                Err(e) => return Err(e),
            }
        }
        buffered_copy_file(src, dest)?;
        Ok(false)
    }

    fn finalize_mode(
        &self,
        shared_inode: bool,
        target_mode: u32,
        src: &Path,
        dest: &Path,
    ) -> io::Result<bool> {
        let current = fs::metadata(dest)?.permissions().mode();
        if shared_inode {
            if current & 0o111 == target_mode & 0o111 {
                return Ok(false);
            }
            fs::remove_file(dest)?;
            buffered_copy_file(src, dest)?;
            fs::set_permissions(dest, fs::Permissions::from_mode(target_mode))?;
            return Ok(true);
        }
        if current & 0o7777 != target_mode & 0o7777 {
            fs::set_permissions(dest, fs::Permissions::from_mode(target_mode))?;
        }
        Ok(false)
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests;
