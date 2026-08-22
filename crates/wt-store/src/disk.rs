//! On-disk content-addressed store (ticket 04), the real
//! implementation of [`Store`] from ADR-0001.
//!
//! Layout, entirely inside the root the constructor is given:
//!
//! - `<root>/objects/<2 hex>/<62 hex>` — content blobs named by their
//!   SHA-256 digest. Identical bytes therefore occupy disk once.
//! - `<root>/refs/<64 hex>` — decimal reference count per blob.
//!
//! All state lives on disk, so separate handles on the same root see
//! each other's writes without shared memory. Blobs are written to a
//! temp file in the same directory and renamed into place, so a crash
//! never leaves a half-written blob at its final address.

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::{ContentId, Error, Result, Store};

pub struct DiskStore {
    root: PathBuf,
}

impl DiskStore {
    /// Create or open a store rooted at `root`. The directory (and its
    /// `objects`/`refs` subdirectories) is created if missing.
    pub fn open(root: impl Into<PathBuf>) -> io::Result<DiskStore> {
        let root = root.into();
        fs::create_dir_all(root.join("objects"))?;
        fs::create_dir_all(root.join("refs"))?;
        Ok(DiskStore { root })
    }

    fn object_path(&self, id: &ContentId) -> PathBuf {
        let hex = id.to_string();
        self.root.join("objects").join(&hex[..2]).join(&hex[2..])
    }

    fn ref_path(&self, id: &ContentId) -> PathBuf {
        self.root.join("refs").join(id.to_string())
    }

    fn read_ref_count(&self, id: &ContentId) -> Result<u64> {
        let text = fs::read_to_string(self.ref_path(id))?;
        text.trim()
            .parse::<u64>()
            .map_err(|e| Error::Io(io::Error::other(format!("bad ref count file: {e}"))))
    }

    fn write_ref_count(&self, id: &ContentId, count: u64) -> Result<()> {
        let path = self.ref_path(id);
        let mut tmp = tempfile::NamedTempFile::new_in(path.parent().expect("refs dir"))?;
        writeln!(tmp, "{count}")?;
        tmp.persist(&path).map_err(|e| Error::Io(e.error))?;
        Ok(())
    }
    /// Hard-link a verified blob out to `dest`, which must not exist.
    ///
    /// Ticket 07: the hash is verified before anything lands in a
    /// tree, then the write bits are stripped from the shared inode —
    /// an in-place rewrite of the linked copy would otherwise corrupt
    /// every other tree and the store itself (the pnpm lesson).
    /// Replacement-style writes (rename-over, unlink plus recreate)
    /// break the share and stay private, so writers still work; they
    /// just get their own copy. The store object becomes read-only
    /// along with the link: permissions live on the inode.
    pub fn link_out(&self, id: &ContentId, dest: &Path) -> Result<()> {
        self.get(id)?;
        fs::hard_link(self.object_path(id), dest)?;
        let mut perms = fs::metadata(dest)?.permissions();
        perms.set_mode(perms.mode() & !0o222);
        Ok(fs::set_permissions(dest, perms)?)
    }
}

impl Store for DiskStore {
    fn put(&mut self, content: &[u8]) -> Result<ContentId> {
        let id = ContentId(Sha256::digest(content).into());
        let path = self.object_path(&id);
        if !path.exists() {
            fs::create_dir_all(path.parent().expect("object parent dir"))?;
            let mut tmp = tempfile::NamedTempFile::new_in(path.parent().expect("object dir"))?;
            tmp.write_all(content)?;
            // Rename is atomic; a second handle putting the same
            // content races harmlessly because both write identical
            // bytes to the same final name.
            tmp.persist(&path).map_err(|e| Error::Io(e.error))?;
        }
        Ok(id)
    }

    fn get(&self, id: &ContentId) -> Result<Vec<u8>> {
        let path = self.object_path(id);
        if !path.exists() {
            return Err(Error::UnknownContent(*id));
        }
        let content = fs::read(&path)?;
        if ContentId(Sha256::digest(&content).into()) != *id {
            return Err(Error::Corrupted(*id));
        }
        Ok(content)
    }

    fn contains(&self, id: &ContentId) -> bool {
        self.object_path(id).exists()
    }

    fn add_ref(&mut self, id: &ContentId) -> Result<()> {
        if !self.contains(id) {
            return Err(Error::UnknownContent(*id));
        }
        let current = match fs::metadata(self.ref_path(id)) {
            Ok(_) => self.read_ref_count(id)?,
            Err(_) => 0,
        };
        self.write_ref_count(id, current + 1)
    }

    fn release_ref(&mut self, id: &ContentId) -> Result<()> {
        if !self.contains(id) {
            return Err(Error::RefCountUnderflow(*id));
        }
        let current = match fs::metadata(self.ref_path(id)) {
            Ok(_) => self.read_ref_count(id)?,
            Err(_) => 0,
        };
        if current == 0 {
            return Err(Error::RefCountUnderflow(*id));
        }
        self.write_ref_count(id, current - 1)
    }

    fn ref_count(&self, id: &ContentId) -> Result<u64> {
        if !self.contains(id) {
            return Err(Error::UnknownContent(*id));
        }
        match fs::metadata(self.ref_path(id)) {
            Ok(_) => self.read_ref_count(id),
            Err(_) => Ok(0),
        }
    }
}
