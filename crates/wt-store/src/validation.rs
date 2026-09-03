//! Ingest validation cache (ticket 02, ticket 06), kept beside the store.
//!
//! One file, `<store root>/ingest-cache.tsv`, records for every path
//! the last ingest saw its size, mtime, inode, ctime, and content id.
//! The next ingest skips reading and hashing any file whose size,
//! mtime, inode, and ctime are unchanged, reusing the recorded id directly.
//! If mtime is near now (within 2 seconds), the entry is rehashed to
//! prevent stale alias hits from rapid writes.
//!
//! The cache sits beside `objects/` and `refs/` — never inside the
//! blob layout — so the store format stays untouched and stores that
//! predate this file keep working. A missing, deleted, or corrupt
//! cache degrades to a full re-ingest; it can never change what ends
//! up in a tree, because materialization still verifies every blob's
//! hash through `Store::get`.
//!
//! Format: one entry per line,
//! `<repo-relative path>\t<size>\t<msecs>\t<mnanos>\t<inode>\t<csecs>\t<cnanos>\t<64 hex digits>`,
//! matching the tab-separated ledger style used elsewhere. Older 5-field
//! lines without inode and ctime are supported for backward compatibility.
//! Unparseable lines are dropped rather than trusted; saving rewrites
//! the whole file through temp-file-plus-rename so a crash mid-write leaves
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
    /// Inode number at last ingest.
    pub inode: u64,
    /// Inode change time at last ingest.
    pub ctime: SystemTime,
    /// Content id the bytes hashed to at last ingest.
    pub id: ContentId,
}

/// Ingest-side stat cache: path → (size, mtime, inode, ctime, content id).
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

    /// The stored content id for `rel`, but only if the recorded size,
    /// mtime, inode, and ctime match what the caller just stat'd, and
    /// mtime is not within 2 seconds of the current clock tick.
    /// Anything else is a miss and must go through read-and-hash.
    pub fn lookup(
        &self,
        rel: &str,
        size: u64,
        mtime: SystemTime,
        inode: u64,
        ctime: SystemTime,
    ) -> Option<ContentId> {
        self.lookup_at(rel, size, mtime, inode, ctime, SystemTime::now())
    }

    /// Explicit-time variant of `lookup` used for deterministic testing
    /// of near-now rehashing boundaries.
    pub fn lookup_at(
        &self,
        rel: &str,
        size: u64,
        mtime: SystemTime,
        inode: u64,
        ctime: SystemTime,
        now: SystemTime,
    ) -> Option<ContentId> {
        let entry = self.entries.get(rel)?;
        if entry.size != size || entry.mtime != mtime {
            return None;
        }
        if entry.inode != 0 && entry.inode != inode {
            return None;
        }
        if entry.ctime != UNIX_EPOCH && entry.ctime != ctime {
            return None;
        }
        let is_near_now = match now.duration_since(mtime) {
            Ok(dur) => dur < Duration::from_secs(2),
            Err(e) => e.duration() < Duration::from_secs(2),
        };
        if is_near_now {
            return None;
        }
        Some(entry.id)
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
            let mtime_since = entry
                .mtime
                .duration_since(UNIX_EPOCH)
                .map_err(|e| io::Error::other(format!("uncacheable mtime: {e}")))?;
            let ctime_since = entry
                .ctime
                .duration_since(UNIX_EPOCH)
                .map_err(|e| io::Error::other(format!("uncacheable ctime: {e}")))?;
            writeln!(
                tmp,
                "{rel}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                entry.size,
                mtime_since.as_secs(),
                mtime_since.subsec_nanos(),
                entry.inode,
                ctime_since.as_secs(),
                ctime_since.subsec_nanos(),
                entry.id
            )?;
        }
        tmp.persist(&self.path).map_err(|e| e.error)?;
        self.dirty = false;
        Ok(())
    }
}

/// Parse cache text, dropping any line that does not parse cleanly.
/// Older 5-column formats without inode and ctime are supported for
/// backward compatibility. Corruption shrinks the cache, never misdirects it.
fn parse(text: &str) -> BTreeMap<String, Entry> {
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        match parts.as_slice() {
            [rel, size, msecs, mnanos, inode, csecs, cnanos, hex] => {
                let (Ok(size), Ok(msecs), Ok(mnanos), Ok(inode), Ok(csecs), Ok(cnanos)) = (
                    size.parse::<u64>(),
                    msecs.parse::<u64>(),
                    mnanos.parse::<u32>(),
                    inode.parse::<u64>(),
                    csecs.parse::<u64>(),
                    cnanos.parse::<u32>(),
                ) else {
                    continue;
                };
                let Some(id) = ContentId::from_hex(hex) else {
                    continue;
                };
                let Some(mtime) = UNIX_EPOCH.checked_add(Duration::new(msecs, mnanos)) else {
                    continue;
                };
                let Some(ctime) = UNIX_EPOCH.checked_add(Duration::new(csecs, cnanos)) else {
                    continue;
                };
                out.insert(
                    rel.to_string(),
                    Entry {
                        size,
                        mtime,
                        inode,
                        ctime,
                        id,
                    },
                );
            }
            [rel, size, msecs, mnanos, hex] => {
                let (Ok(size), Ok(msecs), Ok(mnanos)) = (
                    size.parse::<u64>(),
                    msecs.parse::<u64>(),
                    mnanos.parse::<u32>(),
                ) else {
                    continue;
                };
                let Some(id) = ContentId::from_hex(hex) else {
                    continue;
                };
                let Some(mtime) = UNIX_EPOCH.checked_add(Duration::new(msecs, mnanos)) else {
                    continue;
                };
                out.insert(
                    rel.to_string(),
                    Entry {
                        size,
                        mtime,
                        inode: 0,
                        ctime: UNIX_EPOCH,
                        id,
                    },
                );
            }
            _ => continue,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_handles_both_current_and_legacy_formats() {
        let text = "current.txt\t100\t1700000000\t50\t123456\t1700000001\t60\t0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n\
                    legacy.txt\t200\t1700000000\t0\t0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n\
                    corrupt\tnot\tenough\ttabs\n";
        let parsed = parse(text);
        assert_eq!(parsed.len(), 2);

        let cur = &parsed["current.txt"];
        assert_eq!(cur.size, 100);
        assert_eq!(cur.inode, 123456);
        assert_eq!(cur.ctime, UNIX_EPOCH + Duration::new(1700000001, 60));

        let leg = &parsed["legacy.txt"];
        assert_eq!(leg.size, 200);
        assert_eq!(leg.inode, 0);
        assert_eq!(leg.ctime, UNIX_EPOCH);
    }

    #[test]
    fn lookup_at_near_now_rehashes() {
        let mut cache = ValidationCache {
            root: PathBuf::new(),
            path: PathBuf::new(),
            entries: BTreeMap::new(),
            dirty: false,
        };
        let mtime = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let ctime = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let id = ContentId([1u8; 32]);
        cache.record(
            "file.txt".into(),
            Entry {
                size: 50,
                mtime,
                inode: 999,
                ctime,
                id,
            },
        );

        // Case 1: now is 1 second after mtime (near now -> None / rehash).
        let now_near = mtime + Duration::from_secs(1);
        assert_eq!(
            cache.lookup_at("file.txt", 50, mtime, 999, ctime, now_near),
            None
        );

        // Case 2: now is 10 seconds after mtime (not near now -> hit).
        let now_far = mtime + Duration::from_secs(10);
        assert_eq!(
            cache.lookup_at("file.txt", 50, mtime, 999, ctime, now_far),
            Some(id)
        );

        // Case 3: inode moved -> miss.
        assert_eq!(
            cache.lookup_at("file.txt", 50, mtime, 1000, ctime, now_far),
            None
        );

        // Case 4: ctime moved -> miss.
        let ctime_moved = ctime + Duration::from_secs(1);
        assert_eq!(
            cache.lookup_at("file.txt", 50, mtime, 999, ctime_moved, now_far),
            None
        );
    }
}
