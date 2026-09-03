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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourcePolicy {
    Immutable,

    Any,
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateRefusal {
    pub backend: crate::BackendKind,

    pub reason: String,
}

pub struct SelectionOutcome {
    pub backend: Box<dyn CopyBackend>,

    pub kind: crate::BackendKind,

    pub refusals: Vec<CandidateRefusal>,
}

impl SelectionOutcome {
    pub fn selected_backend(&self) -> crate::BackendKind {
        self.kind
    }

    pub fn refusal_reason(&self) -> Option<&str> {
        self.refusals.last().map(|r| r.reason.as_str())
    }
}

pub fn select_backend_detailed(dir: &Path, policy: SourcePolicy) -> SelectionOutcome {
    select_from_candidates(dir, policy, candidates())
}

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

        assert_eq!(
            all[all.len() - 2].kind(),
            BackendKind::Hardlink,
            "hardlink must be the last fast candidate"
        );
    }

    #[test]
    fn any_policy_never_selects_hardlink() {
        let dir = tempfile::tempdir().expect("tempdir");

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
