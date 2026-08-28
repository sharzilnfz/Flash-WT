//! Backend selection: the fastest safe mechanism for a given
//! filesystem.
//!
//! Candidates are ordered fastest-first. The first one that is
//! [`Safety::Safe`] and reports support for the target directory
//! wins; the portable deep copy is the always-available floor.
//! Hardlink joined the list in ticket 07 once copy-on-shared-write
//! protection made it safe: it slots in ahead of deep copy for
//! filesystems without clone support — but only when the caller can
//! promise the source tree is immutable (see [`SourcePolicy`]).

use std::path::Path;

use crate::deep_copy::DeepCopyBackend;
use crate::hardlink::HardlinkBackend;
use crate::{CopyBackend, Safety};

#[cfg(target_os = "macos")]
use crate::clonefile::ClonefileBackend;
#[cfg(target_os = "linux")]
use crate::copy_file_range::CopyFileRangeBackend;
#[cfg(target_os = "linux")]
use crate::reflink::ReflinkBackend;

/// What the caller promises about the source tree while the copy
/// runs.
///
/// Hardlink strips write bits from the shared inode, and permissions
/// live on the inode: the source path loses its write bits too (the
/// pnpm lesson). That trade is acceptable only for sources that will
/// never be edited — content-addressed store blobs and snapshot
/// trees. Callers who cannot make that promise must pass
/// [`SourcePolicy::Any`], and selection skips hardlink entirely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourcePolicy {
    /// Source files never change during or after the copy.
    /// Hydration from the Store passes this: blobs and snapshot
    /// trees are content-addressed and never mutate.
    Immutable,
    /// The source may be a live checkout someone is editing.
    /// Hydration from anything outside the Store passes this;
    /// hardlink is skipped and selection falls through to deep copy.
    Any,
}

/// Every backend this platform could offer, ordered fastest-first,
/// ending with the portable fallback.
// Platform-conditional pushes read clearer than a vec![] built with
// the same cfg gates, so silence the lint outright.
#[allow(clippy::vec_init_then_push)]
pub fn candidates() -> Vec<Box<dyn CopyBackend>> {
    #[allow(unused_mut)]
    let mut out: Vec<Box<dyn CopyBackend>> = Vec::new();
    #[cfg(target_os = "macos")]
    out.push(Box::new(ClonefileBackend));
    #[cfg(target_os = "linux")]
    {
        out.push(Box::new(ReflinkBackend));
        out.push(Box::new(CopyFileRangeBackend));
    }
    out.push(Box::new(HardlinkBackend));
    out.push(Box::new(DeepCopyBackend));
    out
}

/// Pick the best available backend for the filesystem holding `dir`:
/// the first safe candidate that supports it, falling back to deep
/// copy. On filesystems without clone support this lands on the
/// guarded hardlink backend — but only under
/// [`SourcePolicy::Immutable`]; under [`SourcePolicy::Any`] hardlink
/// is excluded up front, so no code path can point it at a source the
/// caller did not declare immutable.
///
/// The deep-copy floor is constructed directly rather than reached by
/// `expect`: selection must not be able to panic.
pub fn select_backend(dir: &Path, policy: SourcePolicy) -> Box<dyn CopyBackend> {
    let fallback: Box<dyn CopyBackend> = Box::new(DeepCopyBackend);
    candidates()
        .into_iter()
        .filter(|b| match policy {
            SourcePolicy::Immutable => true,
            SourcePolicy::Any => b.kind() != crate::BackendKind::Hardlink,
        })
        .find(|b| b.safety() == Safety::Safe && b.supports(dir))
        .unwrap_or(fallback)
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::BackendKind;

    #[test]
    fn selection_is_safe_ends_in_the_fallback_and_offers_hardlink() {
        let dir = tempfile::tempdir().expect("tempdir");
        let picked = select_backend(dir.path(), SourcePolicy::Immutable);
        assert_eq!(picked.safety(), Safety::Safe);

        let all = candidates();
        assert!(all.iter().any(|b| b.kind() == BackendKind::DeepCopy));
        assert_eq!(all.last().unwrap().kind(), BackendKind::DeepCopy);
        // Hardlink is the automatic fallback on filesystems without
        // clone support: it sits directly ahead of deep copy.
        assert_eq!(
            all[all.len() - 2].kind(),
            BackendKind::Hardlink,
            "hardlink must be the last fast candidate"
        );
    }

    #[test]
    fn any_policy_never_selects_hardlink() {
        let dir = tempfile::tempdir().expect("tempdir");
        // On this tempdir, whatever Immutable picks, Any must pick
        // something that is not hardlink — even when hardlink would
        // have been the fastest safe candidate.
        if candidates()
            .iter()
            .any(|b| b.kind() == BackendKind::Hardlink && b.supports(dir.path()))
        {
            let picked = select_backend(dir.path(), SourcePolicy::Any);
            assert_ne!(
                picked.kind(),
                BackendKind::Hardlink,
                "Any policy must exclude hardlink"
            );
        }
        // And the floor still stands: Any always yields a backend.
        let picked = select_backend(Path::new("/definitely/not/here"), SourcePolicy::Any);
        assert_eq!(picked.kind(), BackendKind::DeepCopy);
    }
}
