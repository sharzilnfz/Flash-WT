//! Backend selection: the fastest mechanism for a given
//! filesystem.
//!
//! Candidates are ordered fastest-first. The first one that
//! reports support for the target directory
//! wins; the portable deep copy is the always-available floor.
//! Hardlink joined the list in ticket 07 once copy-on-shared-write
//! protection made it safe: it slots in ahead of deep copy for
//! filesystems without clone support — but only when the caller can
//! promise the source tree is immutable (see [`SourcePolicy`]).

use std::path::Path;

use crate::CopyBackend;
use crate::deep_copy::DeepCopyBackend;
use crate::hardlink::HardlinkBackend;

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

/// A candidate backend that was evaluated and skipped or refused during selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateRefusal {
    /// The backend that was evaluated.
    pub backend: crate::BackendKind,
    /// Why this backend was refused or skipped for the target directory.
    pub reason: String,
}

/// The result of backend selection, recording the chosen backend, its kind,
/// and refusal reasons for any faster candidates that were rejected.
pub struct SelectionOutcome {
    /// The instantiated copy backend.
    pub backend: Box<dyn CopyBackend>,
    /// The kind of the selected backend.
    pub kind: crate::BackendKind,
    /// Refusal reasons for candidates that were skipped or unsupported.
    pub refusals: Vec<CandidateRefusal>,
}

impl SelectionOutcome {
    /// The backend kind selected.
    pub fn selected_backend(&self) -> crate::BackendKind {
        self.kind
    }

    /// The refusal reason for why faster acceleration was refused, if any.
    pub fn refusal_reason(&self) -> Option<&str> {
        self.refusals.last().map(|r| r.reason.as_str())
    }
}

/// Detailed backend selection: evaluates candidate backends in priority order,
/// recording refusal reasons for each skipped candidate until a supported backend is found.
pub fn select_backend_detailed(dir: &Path, policy: SourcePolicy) -> SelectionOutcome {
    select_from_candidates(dir, policy, candidates())
}

/// Select a backend from an explicit candidate list, recording refusal reasons for each skipped candidate.
pub fn select_from_candidates(
    dir: &Path,
    policy: SourcePolicy,
    candidates: Vec<Box<dyn CopyBackend>>,
) -> SelectionOutcome {
    let mut refusals = Vec::new();
    let fs_name = crate::sys::filesystem_name(dir);

    for b in candidates {
        if b.kind() == crate::BackendKind::Hardlink && policy == SourcePolicy::Any {
            refusals.push(CandidateRefusal {
                backend: crate::BackendKind::Hardlink,
                reason: "hardlink refused: source policy Any prohibits sharing mutable inodes"
                    .to_string(),
            });
            continue;
        }
        if b.supports(dir) {
            let kind = b.kind();
            return SelectionOutcome {
                backend: b,
                kind,
                refusals,
            };
        }
        let reason = match b.kind() {
            crate::BackendKind::Clonefile => {
                format!("filesystem {fs_name} does not support APFS clonefile")
            }
            crate::BackendKind::Reflink => {
                format!("filesystem {fs_name} does not support FICLONE reflink")
            }
            crate::BackendKind::CopyFileRange => {
                format!("filesystem {fs_name} does not support copy_file_range page splicing")
            }
            crate::BackendKind::Hardlink => {
                format!("filesystem {fs_name} does not support hardlinks")
            }
            crate::BackendKind::DeepCopy => "deep copy unavailable".to_string(),
        };
        refusals.push(CandidateRefusal {
            backend: b.kind(),
            reason,
        });
    }

    SelectionOutcome {
        backend: Box::new(DeepCopyBackend),
        kind: crate::BackendKind::DeepCopy,
        refusals,
    }
}

/// Pick the best available backend for the filesystem holding `dir`:
/// the first candidate that supports it, falling back to deep
/// copy. On filesystems without clone support this lands on the
/// guarded hardlink backend — but only under
/// [`SourcePolicy::Immutable`]; under [`SourcePolicy::Any`] hardlink
/// is excluded up front, so no code path can point it at a source the
/// caller did not declare immutable.
///
/// The deep-copy floor is constructed directly rather than reached by
/// `expect`: selection must not be able to panic.
pub fn select_backend(dir: &Path, policy: SourcePolicy) -> Box<dyn CopyBackend> {
    select_backend_detailed(dir, policy).backend
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::BackendKind;

    #[test]
    fn selection_ends_in_the_fallback_and_offers_hardlink() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _picked = select_backend(dir.path(), SourcePolicy::Immutable);

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
        assert!(picked.supports(Path::new("/definitely/not/here")));
    }

    #[test]
    fn detailed_selection_reports_refusal_reasons() {
        let dir = tempfile::tempdir().expect("tempdir");
        let outcome = select_from_candidates(
            dir.path(),
            SourcePolicy::Any,
            vec![Box::new(HardlinkBackend), Box::new(DeepCopyBackend)],
        );
        assert_eq!(outcome.selected_backend(), BackendKind::DeepCopy);
        assert!(outcome.refusal_reason().is_some());
        assert!(
            outcome
                .refusal_reason()
                .unwrap()
                .contains("source policy Any")
        );
        assert_eq!(outcome.refusals.len(), 1);
        assert_eq!(outcome.refusals[0].backend, BackendKind::Hardlink);

        let detailed = select_backend_detailed(dir.path(), SourcePolicy::Immutable);
        assert!(!detailed.selected_backend().as_str().is_empty());
    }
}
