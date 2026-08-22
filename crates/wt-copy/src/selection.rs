//! Backend selection: the fastest safe mechanism for a given
//! filesystem.
//!
//! Candidates are ordered fastest-first. The first one that is
//! [`Safety::Safe`] and reports support for the target directory
//! wins; the portable deep copy is the always-available floor.
//! Hardlink joined the list in ticket 07 once copy-on-shared-write
//! protection made it safe: it slots in ahead of deep copy for
//! filesystems without clone support.

use std::path::Path;

use crate::deep_copy::DeepCopyBackend;
use crate::hardlink::HardlinkBackend;
use crate::{CopyBackend, Safety};

#[cfg(target_os = "macos")]
use crate::clonefile::ClonefileBackend;
#[cfg(target_os = "linux")]
use crate::reflink::ReflinkBackend;

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
    out.push(Box::new(ReflinkBackend));
    out.push(Box::new(HardlinkBackend));
    out.push(Box::new(DeepCopyBackend));
    out
}

/// Pick the best available backend for the filesystem holding `dir`:
/// the first safe candidate that supports it, falling back to deep
/// copy. On filesystems without clone support this now lands on the
/// guarded hardlink backend before ever reaching byte copies.
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
    fn selection_is_safe_ends_in_the_fallback_and_offers_hardlink() {
        let dir = tempfile::tempdir().expect("tempdir");
        let picked = select_backend(dir.path());
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
}
