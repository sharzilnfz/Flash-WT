//! Ingest validation cache (ticket 02), kept beside the store.
//!
//! One file, `<store root>/ingest-cache.tsv`, records for every path
//! the last ingest saw its size, mtime, and content id. The next
//! ingest skips reading and hashing any file whose size AND mtime are
//! unchanged, reusing the recorded id directly.
//!
//! The cache sits beside `objects/` and `refs/` — never inside the
//! blob layout — so the store format stays untouched and stores that
//! predate this file keep working. A missing, deleted, or corrupt
//! cache degrades to a full re-ingest; it can never change what ends
//! up in a tree, because materialization still verifies every blob's
//! hash through `Store::get`.
//!
//! Format: one entry per line,
//! `<repo-relative path>\t<size>\t<secs>\t<nanos>\t<64 hex digits>`,
//! matching the tab-separated ledger style used elsewhere. Unparseable
//! lines are dropped rather than trusted; saving rewrites the whole
//! file through temp-file-plus-rename so a crash mid-write leaves
//! either the previous cache or the complete new one, never a mixture.
//!
//! Durability status: rebuildable and best-effort by design (losing it
//! only costs a full re-read-and-hash on the next ingest); NOT
//! crash-durable — writes are atomic but not fsynced.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ContentId;

/// The cache file's name inside the store root.
const CACHE_FILE: &str = "ingest-cache.tsv";

/// What the last ingest recorded about one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    /// File length in bytes at last ingest.
    pub size: u64,
    /// Last-modified time at last ingest.
    pub mtime: SystemTime,
    /// Content id the bytes hashed to at last ingest.
    pub id: ContentId,
}

/// Ingest-side stat cache: path → (size, mtime, content id).
pub struct ValidationCache {
    /// Store root the cache file lives in.
    root: PathBuf,
    path: PathBuf,
    entries: BTreeMap<String, Entry>,
    /// Set by `record`, cleared by a successful `save`: a warm ingest
    /// that changed nothing must not rewrite a 40k-line TSV for
    /// nothing.
    dirty: bool,
}

impl ValidationCache {
    /// Load the cache living beside the store rooted at `root`. A
    /// missing, unreadable, or corrupt file yields an empty cache:
    /// the next ingest simply re-reads everything, which is always
    /// safe because it is exactly what happened before caches existed.
    pub fn open(root: &Path) -> ValidationCache {
        let path = root.join(CACHE_FILE);
        let entries = fs::read_to_string(&path)
            .map(|text| parse(&text))
            .unwrap_or_default();
        ValidationCache {
            root: root.to_path_buf(),
            path,
            entries,
            dirty: false,
        }
    }

    /// The stored content id for `rel`, but only if both the recorded
    /// size and the recorded mtime match what the caller just stat'd.
    /// Anything else is a miss and must go through read-and-hash.
    pub fn lookup(&self, rel: &str, size: u64, mtime: SystemTime) -> Option<ContentId> {
        let entry = self.entries.get(rel)?;
        (entry.size == size && entry.mtime == mtime).then_some(entry.id)
    }

    /// Record what this ingest learned about one file.
    pub fn record(&mut self, rel: String, entry: Entry) {
        self.entries.insert(rel, entry);
        self.dirty = true;
    }

    /// Number of recorded paths.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True if nothing is recorded.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Rewrite the cache atomically — but only when something was
    /// recorded since open or the last save. A no-op save (nothing
    /// changed) is a cheap `Ok(())`: warm ingests of an unchanged tree
    /// must not pay a full temp-file-plus-rename rewrite of the whole
    /// TSV. Everything goes to a temp file in
    /// the store root first, then one rename publishes it. A crash
    /// mid-write leaves either the previous cache or the full new one;
    /// whatever survives parses as a whole.
    pub fn save(&mut self) -> io::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        let mut tmp = tempfile::NamedTempFile::new_in(&self.root)?;
        for (rel, entry) in &self.entries {
            let since = entry
                .mtime
                .duration_since(UNIX_EPOCH)
                .map_err(|e| io::Error::other(format!("uncacheable mtime: {e}")))?;
            writeln!(
                tmp,
                "{rel}\t{}\t{}\t{}\t{}",
                entry.size,
                since.as_secs(),
                since.subsec_nanos(),
                entry.id
            )?;
        }
        tmp.persist(&self.path).map_err(|e| e.error)?;
        self.dirty = false;
        Ok(())
    }
}

/// Parse cache text, dropping any line that does not parse cleanly.
/// Corruption shrinks the cache, never misdirects it.
fn parse(text: &str) -> BTreeMap<String, Entry> {
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let mut parts = line.splitn(6, '\t');
        let (Some(rel), Some(size), Some(secs), Some(nanos), Some(hex), None) = (
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
        ) else {
            continue;
        };
        let (Ok(size), Ok(secs), Ok(nanos)) = (
            size.parse::<u64>(),
            secs.parse::<u64>(),
            nanos.parse::<u32>(),
        ) else {
            continue;
        };
        let Some(id) = ContentId::from_hex(hex) else {
            continue;
        };
        // Durations beyond the platform range cannot round-trip; skip
        // rather than panic on hostile input.
        let Some(mtime) = UNIX_EPOCH.checked_add(Duration::new(secs, nanos)) else {
            continue;
        };
        out.insert(rel.to_string(), Entry { size, mtime, id });
    }
    out
}
