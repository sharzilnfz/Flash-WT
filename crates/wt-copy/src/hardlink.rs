//! Hardlink backend. Ships present but disabled.
//!
//! Plain hardlinks share one inode between trees, so an in-place
//! rewrite in a worktree silently corrupts every other tree holding
//! the same file (the pnpm lesson, spec user story 5). Until ticket
//! 07 adds copy-on-shared-write protection the backend classifies
//! itself [`Safety::UnsafePending`] and refuses to run.

use std::path::Path;

use crate::{ensure_dest_free, BackendKind, CopyBackend, Error, Result, Safety};

#[derive(Debug, Default)]
pub struct HardlinkBackend;

impl CopyBackend for HardlinkBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Hardlink
    }

    fn safety(&self) -> Safety {
        Safety::UnsafePending
    }

    /// Hardlinks work on any POSIX filesystem, within one filesystem.
    fn supports(&self, _dir: &Path) -> bool {
        true
    }

    fn copy_dir(&self, _src: &Path, dest: &Path) -> Result<()> {
        ensure_dest_free(dest)?;
        // The linking walk lands with ticket 07 (hardlink safety).
        Err(Error::UnsafeBackend)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Error;

    #[test]
    fn refuses_to_run_while_unsafe() {
        let backend = HardlinkBackend;
        assert_eq!(backend.safety(), Safety::UnsafePending);
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("src");
        let dest = dir.path().join("dest");
        assert!(matches!(
            backend.copy_dir(&src, &dest),
            Err(Error::UnsafeBackend)
        ));
        assert!(!dest.exists());
    }
}
