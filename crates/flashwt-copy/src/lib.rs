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
pub use materialize::{BatchItem, BatchReceipt, Materializer, PlacementOutcome, StrategyPolicy};
#[cfg(target_os = "macos")]
pub use materialize::{CloneOut, FileMaterialize, HardlinkOut, placement_refused};
#[cfg(target_os = "linux")]
pub use materialize::{
    CopyFileRangeOut, FileMaterialize, HardlinkOut, ReflinkOut, placement_refused,
};
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub use materialize::{FileMaterialize, HardlinkOut, placement_refused};
#[cfg(target_os = "linux")]
pub use reflink::{ReflinkBackend, reflink_file};
pub use selection::{
    CandidateRefusal, SelectionOutcome, SourcePolicy, candidates, select_backend,
    select_backend_detailed,
};
pub use sys::{FsCapabilities, buffered_copy_file, probe_capabilities, refusal_reason_for_errno};

use std::io;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BackendKind {
    Clonefile,

    Reflink,

    CopyFileRange,

    Hardlink,

    DeepCopy,
}

impl BackendKind {
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

#[derive(Debug)]
pub enum Error {
    DestinationExists,

    Unsupported,

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

pub type Result<T> = std::result::Result<T, Error>;

pub trait CopyBackend {
    fn kind(&self) -> BackendKind;

    fn supports(&self, dir: &Path) -> bool;

    fn copy_dir(&self, src: &Path, dest: &Path) -> Result<()>;
}

pub(crate) fn ensure_dest_free(dest: &Path) -> Result<()> {
    if dest.symlink_metadata().is_ok() {
        Err(Error::DestinationExists)
    } else {
        Ok(())
    }
}
