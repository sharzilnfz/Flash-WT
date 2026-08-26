//! Store scrubbing: the repair path for the trust model's documented
//! residual risk (fast-hydration ticket 05, product-handoff known
//! limitations). The verified-blob ledger trusts a blob while its
//! (size, mtime) fingerprint stays unchanged, so a bit flip that
//! preserves both slips past every warm run. A scrub pass closes that
//! gap by re-hashing EVERY blob against its own content address —
//! the one check the ledger exists to avoid paying, spent deliberately.
//!
//! A corrupt blob cannot be repaired: its true content is gone. It is
//! deleted outright (with its refcount file and verified-ledger
//! entry) because blobs are rebuildable cache data — the next ingest
//! from any surviving checkout re-stores them — while anything still
//! referencing the address must fail LOUDLY (`Error::UnknownContent`)
//! rather than serve bad bytes. `--dry-run` reports without touching.
//!
//! Concurrency: the hash pass holds no lock — blobs are immutable by
//! convention and read via atomic opens, so a concurrent process
//  either sees a complete blob or none. Deletion takes the same
//! exclusive `flock(2)` on the `refs/` directory that every refcount
//! read-modify-write takes, so a concurrent `add_ref`/`release_ref`
//! cannot interleave with the corrupt entry's ref-file removal. Object
//! removal itself is a single `unlink(2)`: a concurrent reader racing
//! the delete gets the file or `NotFound`, never torn bytes.

use std::fs;
use std::io;
use std::os::unix::io::AsRawFd;
use std::path::Path;

use crate::{ContentId, DiskStore, Error, Result};

/// Held `flock(2)` on the store's `refs/` directory for the lifetime
/// of the guard; released on drop. Same exclusion semantics as the
/// refcount lock in [`crate::disk`] — locks are per open file
/// description, so separate processes genuinely contend — reimplemented
/// here rather than exported, keeping the disk module's surface
/// untouched.
struct RefsDirLock {
    _file: fs::File,
}

impl Drop for RefsDirLock {
    fn drop(&mut self) {
        // SAFETY: the fd is valid for as long as `_file` is alive,
        // i.e. through the end of this drop.
        unsafe { libc::flock(self._file.as_raw_fd(), libc::LOCK_UN) };
    }
}

/// Exclusive-lock the `refs/` directory, serializing this pass's
/// refcount-affecting deletions against every other wt process.
fn lock_refs(root: &Path) -> io::Result<RefsDirLock> {
    let dir = fs::File::open(root.join("refs"))?;
    // SAFETY: flock(2) takes only an fd and constants; the fd is
    // valid for as long as `dir` is alive.
    if unsafe { libc::flock(dir.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(RefsDirLock { _file: dir })
}

/// What one scrub pass observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScrubReport {
    /// Blobs the pass streamed through SHA-256.
    pub scanned: u64,
    /// Blobs whose bytes no longer match their content address,
    /// sorted (the enumeration order).
    pub corrupt: Vec<ContentId>,
    /// Corrupt blobs actually deleted; always zero for a dry run.
    pub deleted: u64,
}

impl DiskStore {
    /// Re-hash every blob in the store against its own address.
    ///
    /// With `dry_run` set, corrupt blobs are only reported. Otherwise
    /// each corrupt blob is deleted outright — refcount file, object,
    /// and verified-ledger entry together (via [`DiskStore::delete`])
    /// — so future hydrates fail loudly with [`Error::UnknownContent`]
    /// instead of trusting the entry or serving corrupted bytes.
    ///
    /// Blobs that vanish mid-pass (collected by a concurrent sweep)
    /// are neither corrupt nor deletable here and are simply skipped;
    /// whatever removed them already did the ledger cleanup.
    pub fn scrub(&mut self, dry_run: bool) -> Result<ScrubReport> {
        let ids = self.ids()?;
        let mut corrupt = Vec::new();
        for id in &ids {
            match Self::verify_file(&self.object_path(id), id) {
                Ok(()) => {}
                Err(Error::Corrupted(_)) => corrupt.push(*id),
                Err(Error::UnknownContent(_)) => {}
                Err(e) => return Err(e),
            }
        }

        let mut deleted = 0u64;
        if !dry_run && !corrupt.is_empty() {
            let _lock = lock_refs(self.root())?;
            for id in &corrupt {
                self.delete(id)?;
                deleted += 1;
            }
        }
        // Persist the ledger forgets now rather than at drop, so an
        // interrupted run does not leave trust behind for deleted
        // blobs (harmless — a missing blob never verifies — but free
        // to get right here).
        self.flush()?;

        Ok(ScrubReport {
            scanned: ids.len() as u64,
            corrupt,
            deleted,
        })
    }
}
