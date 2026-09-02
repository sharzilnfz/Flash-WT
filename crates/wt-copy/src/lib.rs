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
#[cfg(target_os = "linux")]
mod copy_file_range;
mod copy_tree;
mod deep_copy;
mod engine;
mod hardlink;
mod materialize;
#[cfg(target_os = "linux")]
mod reflink;
mod selection;
mod sys;

#[cfg(target_os = "macos")]
pub use clonefile::ClonefileBackend;
#[cfg(target_os = "linux")]
pub use copy_file_range::{CopyFileRangeBackend, copy_file_range_file};
pub use deep_copy::DeepCopyBackend;
pub use engine::{CopyEngine, CopyReceipt};
pub use hardlink::HardlinkBackend;
#[cfg(target_os = "macos")]
pub use materialize::{CloneOut, FileMaterialize, HardlinkOut, placement_refused};
#[cfg(target_os = "linux")]
pub use materialize::{
    CopyFileRangeOut, FileMaterialize, HardlinkOut, ReflinkOut, placement_refused,
};
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub use materialize::{FileMaterialize, HardlinkOut, placement_refused};
pub use materialize::{Materializer, PlacementOutcome, StrategyPolicy};
#[cfg(target_os = "linux")]
pub use reflink::{ReflinkBackend, reflink_file};
pub use selection::{SourcePolicy, candidates, select_backend};
pub use sys::buffered_copy_file;

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
    /// Linux in-kernel `copy_file_range(2)` page splicing.
    CopyFileRange,
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
            BackendKind::CopyFileRange => "copy_file_range",
            BackendKind::Hardlink => "hardlink",
            BackendKind::DeepCopy => "deep-copy",
        }
    }
}

/// What went wrong in a backend copy.
#[derive(Debug)]
pub enum Error {
    /// The destination already exists. Backends never merge into or
    /// overwrite an existing tree.
    DestinationExists,
    /// The filesystem holding the paths does not support this backend.
    Unsupported,
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
pub trait CopyBackend {
    /// Which strategy this backend implements.
    fn kind(&self) -> BackendKind;

    /// True if this backend can operate on the filesystem holding
    /// `dir` (for example, clonefile requires APFS).
    fn supports(&self, dir: &Path) -> bool;

    /// Copy the tree at `src` to the not-yet-existing path `dest`.
    ///
    /// Returns [`Error::DestinationExists`] if `dest` exists,
    /// and [`Error::Io`] on failure. On any error `dest` does not exist.
    fn copy_dir(&self, src: &Path, dest: &Path) -> Result<()>;
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
