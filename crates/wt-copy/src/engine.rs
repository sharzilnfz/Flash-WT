//! Deep unified copy engine for whole-directory copying.

use std::path::Path;

use crate::materialize::{StrategyPolicy, placement_refused};
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

/// High-level copy engine encapsulating filesystem probing, strategy selection,
/// and whole-directory copying.
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
}

impl Default for CopyEngine {
    fn default() -> Self {
        Self::new(StrategyPolicy::Default)
    }
}
