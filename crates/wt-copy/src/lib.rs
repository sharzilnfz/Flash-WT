//! Copy-backend contract (ticket 01, implemented in ticket 03).
//!
//! One trait hides every platform copy strategy: APFS `clonefile`,
//! Linux reflink, and hardlink. Callers never branch on the OS; they
//! ask the selection layer (ticket 03) for the best backend for a
//! given directory and call [`CopyBackend::copy_dir`].
//!
//! ## Source policy
//!
//! [`select_backend`] also takes a [`SourcePolicy`] promise about the
//! source tree. Hardlink strips write bits from the shared inode, and
//! because permissions live on the inode, the source path loses them
//! too — hydrating FROM a live checkout would silently make its files
//! unwritable. The rules:
//!
//! - Hydration from the Store passes
//!   [`SourcePolicy::Immutable`]: blobs and snapshot trees are
//!   content-addressed and never mutate.
//! - Hydration from a live checkout (anything outside the Store)
//!   passes [`SourcePolicy::Any`]: hardlink is excluded up front and
//!   selection falls through to deep copy.

#[cfg(target_os = "macos")]
mod clonefile;
mod copy_tree;
mod deep_copy;
mod hardlink;
mod materialize;
#[cfg(target_os = "linux")]
mod reflink;
mod selection;
mod sys;

#[cfg(target_os = "macos")]
pub use clonefile::ClonefileBackend;
pub use deep_copy::DeepCopyBackend;
pub use hardlink::HardlinkBackend;
// CloneOut wraps fclonefileat(2) and only exists on macOS; other
// platforms take the byte-copy path (see wt-cli select_strategy).
#[cfg(target_os = "macos")]
pub use materialize::{placement_refused, CloneOut, FileMaterialize, HardlinkOut};
#[cfg(not(target_os = "macos"))]
pub use materialize::{placement_refused, FileMaterialize, HardlinkOut};
#[cfg(target_os = "linux")]
pub use reflink::ReflinkBackend;
pub use selection::{candidates, select_backend, SourcePolicy};

use std::io;
use std::path::Path;

/// Which concrete strategy a backend uses.
///
/// Stable, displayable names. Selection code and CLI output (ticket 02:
/// "prints what it linked and from where") must report these strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BackendKind {
    /// macOS APFS whole-directory `clonefile(2)`.
    Clonefile,
    /// Linux reflink copies (btrfs/XFS `FICLONE`).
    Reflink,
    /// Plain hardlinks. Fast; shared inodes are made read-only so
    /// in-place rewrites fail instead of corrupting sibling trees
    /// (ticket 07).
    Hardlink,
    /// Portable byte-by-byte fallback. Slow; always available.
    DeepCopy,
}

impl BackendKind {
    /// The stable, displayable name used in CLI output and selection
    /// reports (see the enum docs).
    pub fn as_str(self) -> &'static str {
        match self {
            BackendKind::Clonefile => "clonefile",
            BackendKind::Reflink => "reflink",
            BackendKind::Hardlink => "hardlink",
            BackendKind::DeepCopy => "deep-copy",
        }
    }
}

/// Whether a backend is safe to enable by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Safety {
    /// Safe to use without extra machinery.
    Safe,
    /// Exists but must stay disabled until the shared-write hazard is
    /// solved. No current backend reports this: ticket 07 cleared the
    /// hardlink hazard with copy-on-shared-write protection, but the
    /// classification and its refusal path stay part of the contract.
    UnsafePending,
}

/// What went wrong in a backend copy.
#[derive(Debug)]
pub enum Error {
    /// The destination already exists. Backends never merge into or
    /// overwrite an existing tree.
    DestinationExists,
    /// The filesystem holding the paths does not support this backend.
    Unsupported,
    /// This backend is compiled in but not yet safe to run
    /// ([`Safety::UnsafePending`]) and a caller tried to use it anyway.
    UnsafeBackend,
    /// Any filesystem failure during the copy.
    Io(io::Error),
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::DestinationExists => write!(f, "destination already exists"),
            Error::Unsupported => write!(f, "backend unsupported on this filesystem"),
            Error::UnsafeBackend => write!(f, "backend is unsafe-pending and disabled"),
            Error::Io(e) => write!(f, "copy failed: {e}"),
        }
    }
}

impl std::error::Error for Error {}

/// Copy result: [`Error`] on failure.
pub type Result<T> = std::result::Result<T, Error>;

/// A strategy for copying one directory tree as cheaply as the
/// filesystem allows.
///
/// Contract for implementors (ticket 03):
///
/// - `copy_dir` copies the **entire** tree rooted at `src`, including
///   nested directories, regular files, and permissions. It does not
///   follow symlinks out of `src`; symlinks inside are recreated as-is
///   where the mechanism allows it, otherwise skipped (never followed).
/// - `dest` must not exist. Implementors create `dest` itself; callers
///   only ensure its parent directory exists.
/// - `copy_dir` is all-or-nothing: the tree is materialized under a
///   private `<dest>.<pid>.tmp` staging path, then renamed onto
///   `dest` in a single step. If it returns `Err`, `dest` does not
///   exist and no partial tree survives anywhere.
/// - `supports(dir)` answers for the filesystem that holds `dir`. It
///   must be cheap enough to call per hydration and must not mutate
///   anything.
/// - `safety()` is a static property of the backend kind, not of the
///   filesystem.
pub trait CopyBackend {
    /// Which strategy this backend implements.
    fn kind(&self) -> BackendKind;

    /// Static safety classification. Every shipped backend reports
    /// [`Safety::Safe`]: hardlink earned it in ticket 07 by making
    /// shared inodes read-only (copy-on-shared-write).
    fn safety(&self) -> Safety {
        Safety::Safe
    }

    /// True if this backend can operate on the filesystem holding
    /// `dir` (for example, clonefile requires APFS).
    fn supports(&self, dir: &Path) -> bool;

    /// Copy the tree at `src` to the not-yet-existing path `dest`.
    ///
    /// Returns [`Error::DestinationExists`] if `dest` exists,
    /// [`Error::UnsafeBackend`] when `safety()` is
    /// [`Safety::UnsafePending`], and [`Error::Io`] on failure. On
    /// any error `dest` does not exist.
    fn copy_dir(&self, src: &Path, dest: &Path) -> Result<()>;
}

/// Refuse a backend that exists only behind an explicit opt-in gate
/// ([`Safety::UnsafePending`]). Enforced by every backend's shared
/// placement plumbing before anything touches the filesystem.
pub(crate) fn ensure_backend_runnable(safety: Safety) -> Result<()> {
    match safety {
        Safety::Safe => Ok(()),
        Safety::UnsafePending => Err(Error::UnsafeBackend),
    }
}

/// Fast fail for a destination that already exists (including as a
/// dangling symlink), per the trait contract. Shared by every
/// backend's placement plumbing; the authoritative guard against
/// racing creators is the final atomic rename over it.
pub(crate) fn ensure_dest_free(dest: &Path) -> Result<()> {
    if dest.symlink_metadata().is_ok() {
        Err(Error::DestinationExists)
    } else {
        Ok(())
    }
}
