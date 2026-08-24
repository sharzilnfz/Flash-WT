//! On-disk content-addressed store (ticket 04), the real
//! implementation of [`Store`] from ADR-0001.
//!
//! Layout, entirely inside the root the constructor is given:
//!
//! - `<root>/objects/<2 hex>/<62 hex>` — content blobs named by their
//!   SHA-256 digest. Identical bytes therefore occupy disk once.
//! - `<root>/refs/<64 hex>` — decimal reference count per blob.
//! - `<root>/verified.tsv` — fast-hydration ticket 05's ledger of
//!   blob fingerprints (size, mtime) recorded when each hash was last
//!   checked; see [`crate::verified`].
//!
//! All state lives on disk, so separate handles on the same root see
//! each other's writes without shared memory. Blobs are written to a
//! temp file in the same directory and renamed into place, so a crash
//! never leaves a half-written blob at its final address.

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use crate::verified::VerifiedLedger;
use crate::{ContentId, Error, Result, Store};

pub struct DiskStore {
    root: PathBuf,
    /// Fast-hydration ticket 05: fingerprints of blobs whose hash has
    /// been checked at least once. Updates go through a mutex so the
    /// store handle stays shareable behind `&self` (materialization
    /// only borrows it); the ledger is persisted once per run.
    ledger: Mutex<VerifiedLedger>,
}

/// Held `flock(2)` on the `refs/` directory for the lifetime of the
/// guard; released on drop.
struct RefsLock {
    _file: fs::File,
}

impl Drop for RefsLock {
    fn drop(&mut self) {
        // SAFETY: the fd is valid for as long as `_file` is alive,
        // i.e. through the end of this drop.
        unsafe { libc::flock(self._file.as_raw_fd(), libc::LOCK_UN) };
    }
}

/// What one sweep pass observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Swept {
    /// Entries the sweep looked at.
    pub examined: u64,
    /// Entries it deleted.
    pub reclaimed: u64,
}

impl DiskStore {
    /// Create or open a store rooted at `root`. The directory (and its
    /// `objects`/`refs` subdirectories) is created if missing.
    pub fn open(root: impl Into<PathBuf>) -> io::Result<DiskStore> {
        let root = root.into();
        fs::create_dir_all(root.join("objects"))?;
        fs::create_dir_all(root.join("refs"))?;
        let ledger = Mutex::new(VerifiedLedger::open(&root));
        Ok(DiskStore { root, ledger })
    }

    /// Lock the ledger, recovering from a poisoned one: the entries
    /// inside stay valid no matter which thread panicked mid-update.
    fn ledger(&self) -> MutexGuard<'_, VerifiedLedger> {
        self.ledger.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Take the exclusive advisory lock (`flock(2)`) over refcount
    /// updates by locking the `refs/` DIRECTORY itself. Every
    /// read-modify-write of a refcount runs under this lock so
    /// concurrent processes cannot lose increments (which would let
    /// legacy sweeps collect live content) or decrements (which strand
    /// blobs forever). Locks are per open file description, so
    /// separate handles (threads or processes) genuinely contend.
    ///
    /// Deliberately locks the directory rather than a `.lock` file
    /// inside it: same exclusion semantics with no artifact left in
    /// the store layout.
    fn lock_refs(&self) -> io::Result<RefsLock> {
        let dir = fs::File::open(self.root.join("refs"))?;
        // SAFETY: flock(2) takes only an fd and constants; the fd is
        // valid for as long as `dir` is alive.
        if unsafe { libc::flock(dir.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(RefsLock { _file: dir })
    }

    /// Persist any pending verified-ledger updates. Called once at the
    /// end of a run (also from `Drop`, best-effort); a failure here
    /// only costs verification speed next run, never correctness.
    pub fn flush(&self) -> io::Result<()> {
        self.ledger().save_if_dirty()
    }

    /// Fast-hydration ticket 05: prove the blob at `id` is good,
    /// paying for a read-and-hash only when there is no choice.
    ///
    /// A matching (size, mtime) fingerprint in the verified ledger —
    /// recorded when this blob's hash was last checked — returns
    /// without reading a single byte. Otherwise the blob is STREAMED
    /// through a fixed-size window into an incremental SHA-256 (see
    /// [`Self::verify_digest`]): mismatch fails loudly with
    /// [`Error::Corrupted`] and records nothing; match records the
    /// fingerprint. A missing blob stays [`Error::UnknownContent`].
    pub fn ensure_verified(&self, id: &ContentId) -> Result<()> {
        let path = self.object_path(id);
        let meta = match fs::metadata(&path) {
            Ok(meta) => meta,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(Error::UnknownContent(*id))
            }
            Err(e) => return Err(e.into()),
        };
        let size = meta.len();
        let mtime = meta.modified()?;
        if self.ledger().matches(id, size, mtime) {
            return Ok(());
        }
        self.verify_digest(id)?;
        // Fingerprint AFTER the successful check. Chmod elsewhere does
        // not touch mtime, so what was stat'd is what was hashed.
        self.ledger()
            .record(*id, crate::verified::Fingerprint { size, mtime });
        Ok(())
    }

    /// Prove the blob at `id` hashes to its own address by streaming
    /// it through [`Self::verify_file`]: constant memory regardless
    /// of blob size, so paranoid verification of huge content can
    /// never look like a mysterious OOM kill.
    ///
    /// Unlike [`Self::ensure_verified`] this ignores the verified
    /// ledger entirely — it always reads and hashes. Missing blobs are
    /// [`Error::UnknownContent`]; mismatches are [`Error::Corrupted`].
    pub fn verify_digest(&self, id: &ContentId) -> Result<()> {
        Self::verify_file(&self.object_path(id), id)
    }

    /// Stream-hash the file at `path` in 1 MiB chunks and compare
    /// against `expected`. Never holds more than one chunk plus the
    /// hash state in memory. Shared by [`Self::verify_digest`],
    /// placement, and the snapshot paranoid pass.
    pub(crate) fn verify_file(path: &Path, expected: &ContentId) -> Result<()> {
        let file = match fs::File::open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(Error::UnknownContent(*expected))
            }
            Err(e) => return Err(e.into()),
        };
        let mut reader = io::BufReader::with_capacity(1024 * 1024, file);
        let mut hasher = Sha256::new();
        io::copy(&mut reader, &mut hasher)?;
        if ContentId(hasher.finalize().into()) != *expected {
            return Err(Error::Corrupted(*expected));
        }
        Ok(())
    }

    /// The root this store was opened with. The ingest validation
    /// cache (ticket 02) lives beside `objects/` and `refs/` here,
    /// which is why the store format itself never changes.
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn object_path(&self, id: &ContentId) -> PathBuf {
        let hex = id.to_string();
        self.root.join("objects").join(&hex[..2]).join(&hex[2..])
    }

    /// Path of the blob stored at `id`, whether or not it exists.
    ///
    /// Exposed for file-shaped materializers (fast-hydration ticket
    /// 03): CoW clones and hardlinks need to hand the kernel the blob
    /// itself, not a buffered copy of its bytes. Callers verify via
    /// [`Store::get`] before trusting anything they read here.
    pub fn blob_path(&self, id: &ContentId) -> PathBuf {
        self.object_path(id)
    }

    pub(crate) fn ref_path(&self, id: &ContentId) -> PathBuf {
        self.root.join("refs").join(id.to_string())
    }

    pub(crate) fn read_ref_count(&self, id: &ContentId) -> Result<u64> {
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
    /// Every content id currently stored, in stable order. Malformed
    /// names under `objects/` are skipped rather than failing the
    /// enumeration — the sweep must tolerate a store that was touched
    /// by an interrupted run.
    pub(crate) fn ids(&self) -> Result<Vec<ContentId>> {
        let mut out = Vec::new();
        let shards = fs::read_dir(self.root.join("objects")).map_err(Error::Io)?;
        for shard in shards.flatten() {
            if !shard.file_type().map_err(Error::Io)?.is_dir() {
                continue;
            }
            let blobs = fs::read_dir(shard.path()).map_err(Error::Io)?;
            for blob in blobs.flatten() {
                // Rebuild "<shard><name>" and parse it back to bytes.
                let hex = format!(
                    "{}{}",
                    shard.file_name().to_string_lossy(),
                    blob.file_name().to_string_lossy()
                );
                if let Some(id) = ContentId::from_hex(&hex) {
                    out.push(id);
                }
            }
        }
        out.sort();
        out.dedup();
        Ok(out)
    }

    /// Remove an entry outright: ref file first, then the object. If
    /// this is interrupted between the two, what remains is an
    /// unreferenced object — a state the sweep already understands and
    /// will reclaim on its next run.
    ///
    /// Ticket 05: the verified-ledger entry goes with it (best-effort;
    /// a stale entry could only ever cost a re-verification, but there
    /// is no reason to keep it).
    pub fn delete(&mut self, id: &ContentId) -> Result<()> {
        match fs::remove_file(self.ref_path(id)) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        fs::remove_file(self.object_path(id))?;
        // Best-effort cleanup of an emptied shard directory.
        if let Some(shard) = self.object_path(id).parent() {
            let _ = fs::remove_dir(shard);
        }
        self.ledger().forget(id);
        Ok(())
    }

    /// Age-based garbage collection (ticket 06): delete every entry
    /// whose reference count is zero and whose object file was last
    /// modified before `now - max_age`. Referenced entries survive any
    /// threshold; the age floor protects entries that are mid-write or
    /// awaiting their first reference (hydration puts content before
    /// it claims references on it).
    ///
    /// Deletion order (ref file first) means a kill at any point
    /// leaves only states the next sweep can finish: either both files
    /// present, or an unreferenced object with no ref file.
    pub fn sweep(&mut self, max_age: Duration) -> Result<Swept> {
        let cutoff = SystemTime::now()
            .checked_sub(max_age)
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let ids = self.ids()?;
        let examined = ids.len() as u64;
        let mut reclaimed = 0u64;
        for id in ids {
            match fs::metadata(self.ref_path(&id)) {
                Ok(_) => {
                    if self.read_ref_count(&id)? > 0 {
                        continue;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
            let modified = fs::metadata(self.object_path(&id))
                .map_err(Error::Io)?
                .modified()
                .map_err(Error::Io)?;
            if modified > cutoff {
                continue;
            }
            self.delete(&id)?;
            reclaimed += 1;
        }
        Ok(Swept {
            examined,
            reclaimed,
        })
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
        // Streaming verify: same guarantee as Store::get's read-and-
        // hash, without ever holding the whole blob in memory.
        self.verify_digest(id)?;
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
            // Durability: fsync the blob's bytes BEFORE the rename.
            // A crash after the rename without this could leave the
            // final address naming empty/truncated content — and a
            // content-addressed store cannot tell that from a valid
            // empty file. The parent-dir fsync after persist makes
            // the new directory entry itself durable too.
            tmp.as_file().sync_all()?;
            // Rename is atomic; a second handle putting the same
            // content races harmlessly because both write identical
            // bytes to the same final name.
            tmp.persist(&path).map_err(|e| Error::Io(e.error))?;
            crate::fsutil::sync_parent_dir(&path)?;
            // Temp files carry restrictive modes; normalize the blob
            // to plain 0644 (ticket 05). CoW clones then inherit
            // normal writable permissions, so the placement path needs
            // no per-file chmod. Blobs written by older versions keep
            // their 0600 mode until re-ingested — their clones are
            // owner-rw-only, which is acceptable and cheaper than a
            // chmod walk over every hydrated file.
            //
            // The fingerprint is recorded here too: the address IS the
            // hash of the bytes just written through an atomic rename,
            // so a fresh blob is verified by construction.
            let perms = fs::Permissions::from_mode(0o644);
            let _ = fs::set_permissions(&path, perms);
            if let Ok(meta) = fs::metadata(&path) {
                if let Ok(mtime) = meta.modified() {
                    self.ledger().record(
                        id,
                        crate::verified::Fingerprint {
                            size: meta.len(),
                            mtime,
                        },
                    );
                }
            }
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
        // The flock makes the read-modify-write below atomic across
        // processes; a lost increment here could let a legacy sweep
        // collect live content.
        let _lock = self.lock_refs()?;
        // Fast-hydration ticket 05: claim_references calls this once
        // per distinct blob, so skip the redundant existence stat when
        // a readable ref file already proves the blob was put before.
        let current = match fs::metadata(self.ref_path(id)) {
            Ok(_) => self.read_ref_count(id)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                if !self.contains(id) {
                    return Err(Error::UnknownContent(*id));
                }
                0
            }
            Err(e) => return Err(e.into()),
        };
        self.write_ref_count(id, current + 1)
    }

    fn release_ref(&mut self, id: &ContentId) -> Result<()> {
        let _lock = self.lock_refs()?;
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

impl Drop for DiskStore {
    /// Best-effort persistence of the verified ledger. Failures cost
    /// verification speed on the next run (the next run re-hashes what
    /// it cannot trust), never correctness.
    fn drop(&mut self) {
        let _ = self.ledger().save_if_dirty();
    }
}
