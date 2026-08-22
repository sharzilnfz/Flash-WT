//! Portable byte-by-byte fallback. Slow, but every filesystem
//! supports it — the floor the selection layer can always stand on.

use std::fs;
use std::path::Path;

use crate::copy_tree::copy_tree;
use crate::{ensure_dest_free, BackendKind, CopyBackend, Error, Result};

#[derive(Debug, Default)]
pub struct DeepCopyBackend;

impl CopyBackend for DeepCopyBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::DeepCopy
    }

    /// Always true: byte copies work everywhere.
    fn supports(&self, _dir: &Path) -> bool {
        true
    }

    fn copy_dir(&self, src: &Path, dest: &Path) -> Result<()> {
        ensure_dest_free(dest)?;
        let mut copy_file = |from: &Path, to: &Path| fs::copy(from, to).map(|_| ());
        copy_tree(src, dest, &mut copy_file).map_err(Error::Io)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Error;

    #[test]
    fn copies_nested_tree_and_rejects_existing_dest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("src");
        fs::create_dir_all(src.join("a/b")).expect("mkdir");
        fs::write(src.join("a/b/f.txt"), "deep bytes\n").expect("write");

        let dest = dir.path().join("dest");
        DeepCopyBackend.copy_dir(&src, &dest).expect("copy_dir");
        assert_eq!(
            fs::read_to_string(dest.join("a/b/f.txt")).expect("read"),
            "deep bytes\n"
        );

        assert!(matches!(
            DeepCopyBackend.copy_dir(&src, &dest),
            Err(Error::DestinationExists)
        ));
    }
}
