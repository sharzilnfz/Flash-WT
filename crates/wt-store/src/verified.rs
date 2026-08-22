//! Verified-blob ledger (fast-hydration ticket 05), kept beside the
//! store.
//!
//! One file, `<store root>/verified.tsv`, records for every blob the
//! moment its hash was last checked: size and mtime at check time.
//! Materialization consults this ledger instead of re-reading and
//! re-hashing every byte on every run — a blob is trusted once
//! verified, and trust expires the moment the fingerprint changes.
//!
//! The ledger sits beside `objects/` and `refs/` — never inside the
//! blob layout — so the store format stays untouched and stores that
//! predate this file keep working. A missing, deleted, or corrupt
//! ledger degrades to full verification of everything; it can never
//! make a run cheaper AND wronger, because a hit requires an exact
//! (size, mtime) match with what the blob's stat just reported.
//!
//! Accepted residual risk: bit rot that preserves both size and mtime
//! between checks goes unnoticed until the next fingerprint change.
//! `WT_VERIFY=1` (handled in the CLI layer) forces the full re-hash of
//! everything for paranoid runs.
//!
//! Format: one entry per line,
//! `<64 hex digits>\t<size>\t<secs>\t<nanos>`, matching the
//! tab-separated ledger style used elsewhere. Unparseable lines are
//! dropped rather than trusted; saving rewrites the whole file through
//! temp-file-plus-rename so a crash mid-write leaves either the
//! previous ledger or the complete new one, never a mixture.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ContentId;

/// The ledger file's name inside the store root.
const LEDGER_FILE: &str = "verified.tsv";

/// What the last verification recorded about one blob.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fingerprint {
    /// Blob length in bytes at last verification.
    pub size: u64,
    /// Last-modified time at last verification.
    pub mtime: SystemTime,
}

pub struct VerifiedLedger {
    path: PathBuf,
    entries: BTreeMap<ContentId, Fingerprint>,
    dirty: bool,
}

impl VerifiedLedger {
    /// Load the ledger living beside the store rooted at `root`. A
    /// missing, unreadable, or corrupt file yields an empty ledger:
    /// the next materialize simply re-verifies everything, which is
    /// always safe because it is exactly what happened before ledgers
    /// existed.
    pub fn open(root: &Path) -> VerifiedLedger {
        let path = root.join(LEDGER_FILE);
        let entries = fs::read_to_string(&path)
            .map(|text| parse(&text))
            .unwrap_or_default();
        VerifiedLedger {
            path,
            entries,
            dirty: false,
        }
    }

    /// True when the recorded fingerprint for `id` matches both the
    /// size and the mtime the caller just stat'd. Anything else is a
    /// miss and must go through read-and-hash.
    pub fn matches(&self, id: &ContentId, size: u64, mtime: SystemTime) -> bool {
        self.entries
            .get(id)
            .is_some_and(|f| f.size == size && f.mtime == mtime)
    }

    /// Record what this verification learned about one blob.
    pub fn record(&mut self, id: ContentId, fingerprint: Fingerprint) {
        self.entries.insert(id, fingerprint);
        self.dirty = true;
    }

    /// Drop the entry for a deleted blob. A no-op when nothing was
    /// recorded.
    pub fn forget(&mut self, id: &ContentId) {
        if self.entries.remove(id).is_some() {
            self.dirty = true;
        }
    }

    /// Number of recorded blobs.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True if nothing is recorded.
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Rewrite the ledger atomically — but only when something changed
    /// since open or the last save. Everything goes to a temp file in
    /// the store root first, then one rename publishes it; a crash
    /// mid-write leaves either the previous ledger or the full new
    /// one, never a mixture.
    pub fn save_if_dirty(&mut self) -> io::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        let parent = self.path.parent().expect("ledger lives beside a root");
        let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
        for (id, fp) in &self.entries {
            let since = fp
                .mtime
                .duration_since(UNIX_EPOCH)
                .map_err(|e| io::Error::other(format!("unledgerable mtime: {e}")))?;
            writeln!(
                tmp,
                "{}\t{}\t{}\t{}",
                id,
                fp.size,
                since.as_secs(),
                since.subsec_nanos()
            )?;
        }
        tmp.persist(&self.path).map_err(|e| e.error)?;
        self.dirty = false;
        Ok(())
    }
}

/// Parse ledger text, dropping any line that does not parse cleanly.
/// Corruption shrinks the ledger, never misdirects it.
fn parse(text: &str) -> BTreeMap<ContentId, Fingerprint> {
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let mut parts = line.splitn(5, '\t');
        let (Some(hex), Some(size), Some(secs), Some(nanos), None) = (
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
        ) else {
            continue;
        };
        let (Some(id), Ok(size), Ok(secs), Ok(nanos)) = (
            ContentId::from_hex(hex),
            size.parse::<u64>(),
            secs.parse::<u64>(),
            nanos.parse::<u32>(),
        ) else {
            continue;
        };
        // Durations beyond the platform range cannot round-trip; skip
        // rather than panic on hostile input.
        let Some(mtime) = UNIX_EPOCH.checked_add(Duration::new(secs, nanos)) else {
            continue;
        };
        out.insert(id, Fingerprint { size, mtime });
    }
    out
}
