//! Deep unified copy engine for whole-directory copying and batch file materialization.

use std::io;
use std::path::{Path, PathBuf};

use crate::materialize::{Materializer, PlacementOutcome, StrategyPolicy, placement_refused};
use crate::selection::{SourcePolicy, select_backend};
use crate::sys::{count_files_and_bytes, find_existing_ancestor, is_cross_device};
use crate::{BackendKind, CopyBackend, DeepCopyBackend, Error, HardlinkBackend, Result};

/// Execution report returned by [`CopyEngine::copy_dir`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyReceipt {
    /// The display name of the strategy used (e.g. "clonefile", "reflink", "copy_file_range", "hardlink", "deep-copy").
    pub strategy: &'static str,
    /// Total number of regular files copied.
    pub files_copied: u64,
    /// Total bytes copied across all regular files.
    pub bytes_copied: u64,
}

/// Aggregated statistics from batch file materialization via [`CopyEngine::materialize_files`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BatchPlacementReceipt {
    /// Total number of files successfully placed.
    pub total_placed: usize,
    /// Number of files sharing storage via copy-on-write clone or shared hardlink inode.
    pub shared_cow_count: usize,
    /// Number of files whose permissions were repaired via private copy.
    pub repaired_modes_count: usize,
}

impl BatchPlacementReceipt {
    /// Create a new batch placement receipt.
    pub fn new(total_placed: usize, shared_cow_count: usize, repaired_modes_count: usize) -> Self {
        Self {
            total_placed,
            shared_cow_count,
            repaired_modes_count,
        }
    }
}

/// High-level copy engine encapsulating filesystem probing, strategy selection,
/// whole-directory copying, and batch file materialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CopyEngine {
    policy: StrategyPolicy,
}

impl CopyEngine {
    /// Create a new copy engine with the specified placement strategy policy.
    pub fn new(policy: StrategyPolicy) -> Self {
        Self { policy }
    }

    /// The configured strategy policy for this engine.
    pub fn policy(&self) -> StrategyPolicy {
        self.policy
    }

    /// Copy an entire directory tree from `src` to `dest`.
    ///
    /// Automatically inspects filesystem capabilities and device boundaries between `src`
    /// and `dest`, selecting the fastest safe placement backend and falling back transparently
    /// to byte copies when necessary.
    pub fn copy_dir(
        &self,
        src: &Path,
        dest: &Path,
        source_policy: SourcePolicy,
    ) -> Result<CopyReceipt> {
        let is_cross = is_cross_device(src, dest);
        let dest_ancestor = find_existing_ancestor(dest);

        let backend: Box<dyn CopyBackend> = match self.policy {
            StrategyPolicy::ForceByteCopy => Box::new(DeepCopyBackend),
            StrategyPolicy::Hardlink => {
                if !is_cross && source_policy == SourcePolicy::Immutable {
                    let hardlink = HardlinkBackend;
                    if hardlink.supports(dest_ancestor) {
                        Box::new(hardlink)
                    } else {
                        Box::new(DeepCopyBackend)
                    }
                } else {
                    Box::new(DeepCopyBackend)
                }
            }
            StrategyPolicy::Default => {
                if is_cross {
                    Box::new(DeepCopyBackend)
                } else {
                    select_backend(dest_ancestor, source_policy)
                }
            }
        };

        let strategy_kind = backend.kind();
        match backend.copy_dir(src, dest) {
            Ok(()) => {
                let (files_copied, bytes_copied) = count_files_and_bytes(dest)?;
                Ok(CopyReceipt {
                    strategy: strategy_kind.as_str(),
                    files_copied,
                    bytes_copied,
                })
            }
            Err(Error::Unsupported) if strategy_kind != BackendKind::DeepCopy => {
                let fallback = DeepCopyBackend;
                fallback.copy_dir(src, dest)?;
                let (files_copied, bytes_copied) = count_files_and_bytes(dest)?;
                Ok(CopyReceipt {
                    strategy: fallback.kind().as_str(),
                    files_copied,
                    bytes_copied,
                })
            }
            Err(Error::Io(e))
                if strategy_kind != BackendKind::DeepCopy && placement_refused(&e) =>
            {
                let fallback = DeepCopyBackend;
                fallback.copy_dir(src, dest)?;
                let (files_copied, bytes_copied) = count_files_and_bytes(dest)?;
                Ok(CopyReceipt {
                    strategy: fallback.kind().as_str(),
                    files_copied,
                    bytes_copied,
                })
            }
            Err(e) => Err(e),
        }
    }

    /// Place one file at `dest` from `src`, automatically detecting cross-device
    /// boundaries and filesystem capabilities, and normalizing permission `mode`.
    pub fn materialize_file(
        &self,
        src: &Path,
        dest: &Path,
        mode: Option<u32>,
    ) -> io::Result<PlacementOutcome> {
        let materializer = Materializer::for_paths(self.policy, src, dest);
        materializer.materialize_file(src, dest, mode)
    }

    /// Batch helper placing multiple files and aggregating total placed,
    /// shared CoW count, and repaired modes.
    pub fn materialize_files(
        &self,
        items: &[(PathBuf, PathBuf, Option<u32>)],
    ) -> io::Result<BatchPlacementReceipt> {
        let mut total_placed = 0;
        let mut shared_cow_count = 0;
        let mut repaired_modes_count = 0;

        for (src, dest, mode) in items {
            let outcome = self.materialize_file(src, dest, *mode)?;
            total_placed += 1;
            if outcome.is_shared_cow {
                shared_cow_count += 1;
            }
            if outcome.is_mode_repaired {
                repaired_modes_count += 1;
            }
        }

        Ok(BatchPlacementReceipt {
            total_placed,
            shared_cow_count,
            repaired_modes_count,
        })
    }
}

impl Default for CopyEngine {
    fn default() -> Self {
        Self::new(StrategyPolicy::Default)
    }
}
