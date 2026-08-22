//! Placeholder implementation of [`CopyBackend`], replaced by the
//! real backends in ticket 03. Exists so downstream crates compile
//! against the trait before backends land.

use std::path::Path;

use crate::{BackendKind, CopyBackend, Error, Result, Safety};

#[derive(Debug, Default)]
pub struct StubBackend;

impl CopyBackend for StubBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::DeepCopy
    }

    fn safety(&self) -> Safety {
        Safety::Safe
    }

    fn supports(&self, _dir: &Path) -> bool {
        false
    }

    fn copy_dir(&self, _src: &Path, _dest: &Path) -> Result<()> {
        Err(Error::Unsupported)
    }
}
