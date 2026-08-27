//! Crash-durable write primitives (durability ticket).
//!
//! Every durability claim elsewhere in the store rests on
//! write-temp-then-rename — but a rename alone is NOT durability:
//! on power loss an ext4/APFS filesystem can land the rename while
//! the file's data blocks were never written, leaving a valid-looking
//! name over empty or truncated content. The fix is ordering with
//! explicit flushes:
//!
//! 1. write the payload and `fsync` the FILE (data hits the device),
//! 2. rename,
//! 3. `fsync` the parent DIRECTORY (the new name hits the device).
//!
//! Use these for everything that acts as truth: mirrors (GC roots),
//! blobs, the gc-mode marker, snapshot metadata. Files that are pure
//! rebuildable caches (verified ledger, selection index, ingest
//! validation cache) deliberately stay best-effort atomic — see their
//! doc comments.
//!
//! Cost model: each helper pays two to three fsyncs per call. That is
//! acceptable because these run once per create/publish, not per
//! blob-sized unit of work.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

/// Write `bytes` to `path` crash-durably: create/truncate, write,
/// `sync_all` the file, then `fsync` its parent directory so the
/// (possibly new) directory entry itself survives power loss.
pub(crate) fn durable_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = fs::File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    sync_parent_dir(path)
}

/// Finish a temp-file-then-final-name publish crash-durably: `fsync`
/// the already-written temp file, rename it onto `final_path`, then
/// `fsync` the final parent directory. After this returns, a crash
/// can no longer leave `final_path` naming truncated or empty
/// content — the worst pre-existing state was the complete temp file.
pub(crate) fn durable_write_then_rename(tmp_path: &Path, final_path: &Path) -> io::Result<()> {
    let file = fs::File::open(tmp_path)?;
    file.sync_all()?;
    drop(file);
    fs::rename(tmp_path, final_path)?;
    sync_parent_dir(final_path)
}

/// `fsync` the directory that contains `path`. Required after a
/// rename/create for the NAME change itself to be durable.
pub(crate) fn sync_parent_dir(path: &Path) -> io::Result<()> {
    let dir = fs::File::open(path.parent().unwrap_or_else(|| Path::new(".")))?;
    dir.sync_all()
}
