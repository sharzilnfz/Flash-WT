//! Backend selection: the fastest safe mechanism for a given
//! filesystem.
//!
//! Candidates are ordered fastest-first. The first one that is
//! [`Safety::Safe`] and reports support for the target directory
//! wins; the portable deep copy is the always-available floor.
//! Hardlink is deliberately absent from the list until ticket 07
//! clears its shared-write hazard.

use std::path::Path;

use crate::deep_copy::DeepCopyBackend;
use crate::{CopyBackend, Safety};

#[cfg(target_os = "macos")]
use crate::clonefile::ClonefileBackend;
#[cfg(target_os = "linux")]
use crate::reflink::ReflinkBackend;

/// Every backend this platform could offer, ordered fastest-first,
/// ending with the portable fallback. Includes nothing disabled by
/// safety — callers wanting the hardlink backend must construct it
/// explicitly (ticket 07 decides when that is allowed).
// Platform-conditional pushes read clearer than a vec![] built with
// the same cfg gates, so silence the lint outright.
#[allow(clippy::vec_init_then_push)]
pub fn candidates() -> Vec<Box<dyn CopyBackend>> {
    #[allow(unused_mut)]
    let mut out: Vec<Box<dyn CopyBackend>> = Vec::new();
    #[cfg(target_os = "macos")]
    out.push(Box::new(ClonefileBackend));
    #[cfg(target_os = "linux")]
    out.push(Box::new(ReflinkBackend));
    out.push(Box::new(DeepCopyBackend));
    out
}

/// Pick the best available backend for the filesystem holding `dir`:
/// the first safe candidate that supports it, falling back to deep
/// copy. Never returns the hardlink backend until ticket 07.
pub fn select_backend(dir: &Path) -> Box<dyn CopyBackend> {
    candidates()
        .into_iter()
        .find(|b| b.safety() == Safety::Safe && b.supports(dir))
        .expect("deep copy supports every filesystem")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BackendKind;

    #[test]
    fn selection_is_safe_and_ends_in_the_fallback() {
        let dir = tempfile::tempdir().expect("tempdir");
        let picked = select_backend(dir.path());
        assert_eq!(picked.safety(), Safety::Safe);
        assert_ne!(picked.kind(), BackendKind::Hardlink);

        let all = candidates();
        assert!(all.iter().any(|b| b.kind() == BackendKind::DeepCopy));
        assert!(all.iter().all(|b| b.kind() != BackendKind::Hardlink));
        assert_eq!(all.last().unwrap().kind(), BackendKind::DeepCopy);
    }
}
