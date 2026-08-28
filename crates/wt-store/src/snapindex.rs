//! Selection index for v2 incremental snapshot rebuilds.
//!
//! One line per (repo root, pattern, heavy directory) remembers which
//! published snapshots recently served that exact heavy directory, so
//! a later create with different content can pick an OLD snapshot to
//! diff against instead of rebuilding from scratch:
//!
//! ```text
//! <escaped-repo-root>\t<escaped-pattern>\t<escaped-heavy-dir>\t<hash,hash,hash>\t<mtime-secs>
//! ```
//!
//! - The three text fields reuse [`crate::mirror`]'s percent escaping,
//!   so paths containing tabs or newlines survive the TSV framing.
//! - The ring holds up to [`MAX_RING`] manifest hashes, NEWEST first.
//!   Three generations is enough for the common bump-and-create loop
//!   without letting the index grow unboundedly.
//! - `mtime-secs` is unix seconds of the last PUBLISH through this
//!   key (hits reorder the ring but do not refresh it).
//!
//! Whole-file load and save; the save publishes atomically (temp file
//! in the same directory, then rename). Parsing is tolerant: any line
//! that does not validate is dropped silently — the index is pure
//! optimization metadata, never worth failing a create over.
//!
//! Durability status: rebuildable and best-effort by design (losing it
//! only degrades v2 incremental rebuilds to full builds); NOT
//! crash-durable — writes are atomic but not fsynced.
//!
//! The module also hosts [`SnapshotLru`], the retention-cap sidecar
//! (product-handoff §7.4): one `<hash>\t<last-use-unix-secs>` line per
//! snapshot, refreshed on publish AND on hit through the free
//! [`record_publish`]/[`record_hit`] entry points. GC reads it to pick
//! which unreferenced snapshots exceed `WT_SNAPSHOT_CAP` and evicts
//! least-recently-used first. Same conventions as the selection
//! index: whole-file load/save with an atomic rename, tolerant parse,
//! torn tail dropped, best-effort everywhere — losing the sidecar
//! only degrades eviction order to directory mtime, never correctness.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ContentId;
use crate::mirror::escape;
use crate::snapshot::{Manifest, read_published};

/// Ring capacity: how many recent manifest hashes one key remembers.
pub const MAX_RING: usize = 3;

/// One key's selection record. `repo_root`, `pattern`, and
/// `heavy_dir` are stored UNESCAPED (the plain text values); escaping
/// happens only at serialization time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionRecord {
    /// Absolute path of the repository the selection was made in.
    pub repo_root: String,
    /// The include-pattern string that selected the heavy directory.
    pub pattern: String,
    /// The heavy directory's path relative to `repo_root`.
    pub heavy_dir: String,
    /// Manifest hashes, newest first.
    pub ring: Vec<ContentId>,
    /// Unix seconds of the last publish under this key.
    pub mtime_secs: u64,
}

impl SelectionRecord {
    fn matches(&self, repo_root: &str, pattern: &str, heavy_dir: &str) -> bool {
        self.repo_root == repo_root && self.pattern == pattern && self.heavy_dir == heavy_dir
    }

    fn serialize(&self) -> String {
        let ring = self
            .ring
            .iter()
            .map(|h| h.to_string())
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "{}\t{}\t{}\t{}\t{}\n",
            escape(&self.repo_root),
            escape(&self.pattern),
            escape(&self.heavy_dir),
            ring,
            self.mtime_secs
        )
    }

    /// Tolerant parse of one line: `None` for anything malformed.
    /// Empty rings are useless records and dropped too.
    fn parse_line(line: &str) -> Option<SelectionRecord> {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() != 5 {
            return None;
        }
        if f[3].is_empty() {
            return None;
        }
        let mut ring = Vec::new();
        for hex in f[3].split(',') {
            let id = ContentId::from_hex(hex)?;
            if !ring.contains(&id) {
                ring.push(id);
            }
        }
        if ring.is_empty() || ring.len() > MAX_RING {
            return None;
        }
        Some(SelectionRecord {
            repo_root: crate::mirror::unescape(f[0]).ok()?,
            pattern: crate::mirror::unescape(f[1]).ok()?,
            heavy_dir: crate::mirror::unescape(f[2]).ok()?,
            ring,
            mtime_secs: f[4].parse().ok()?,
        })
    }
}

/// The whole index: every valid record, in file order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SelectionIndex {
    /// All valid records.
    pub records: Vec<SelectionRecord>,
}

fn index_path(root: &Path) -> PathBuf {
    root.join("snapshots").join("index.tsv")
}

impl SelectionIndex {
    /// Load `<root>/snapshots/index.tsv` and replay `<root>/snapshots/journal.tsv`.
    /// A missing or unreadable file is an empty index; bad lines are dropped silently.
    pub fn load(root: &Path) -> SelectionIndex {
        let mut idx = Self::load_canonical(root);
        idx.apply_journal(root);
        idx
    }

    /// Load only the canonical `<root>/snapshots/index.tsv` without journal replay.
    pub fn load_canonical(root: &Path) -> SelectionIndex {
        let mut idx = SelectionIndex::default();
        let Ok(text) = fs::read_to_string(index_path(root)) else {
            return idx;
        };
        // Same torn-line rule as mirrors and manifests: whatever
        // follows the last newline never got its terminal newline.
        let complete = match text.strip_suffix('\n') {
            Some(body) => body,
            None => match text.rfind('\n') {
                Some(i) => &text[..i],
                None => return idx,
            },
        };
        for line in complete.split('\n').filter(|l| !l.is_empty()) {
            if let Some(rec) = SelectionRecord::parse_line(line) {
                // Last writer wins on duplicate keys: the newest
                // physical line is the most recent state.
                if let Some(existing) = idx
                    .records
                    .iter_mut()
                    .find(|r| r.matches(&rec.repo_root, &rec.pattern, &rec.heavy_dir))
                {
                    *existing = rec;
                } else {
                    idx.records.push(rec);
                }
            }
        }
        idx
    }

    /// Replay entries from `<root>/snapshots/journal.tsv`.
    pub fn apply_journal(&mut self, root: &Path) {
        let Ok(text) = fs::read_to_string(journal_path(root)) else {
            return;
        };
        let complete = match text.strip_suffix('\n') {
            Some(body) => body,
            None => match text.rfind('\n') {
                Some(i) => &text[..i],
                None => return,
            },
        };
        for line in complete.split('\n').filter(|l| !l.is_empty()) {
            if let Some(entry) = parse_journal_line(line) {
                match entry {
                    JournalEntry::Publish {
                        repo_root,
                        pattern,
                        heavy_dir,
                        hash,
                        timestamp,
                    } => {
                        self.record_publish(&repo_root, &pattern, &heavy_dir, &hash, timestamp);
                    }
                    JournalEntry::Hit {
                        repo_root,
                        pattern,
                        heavy_dir,
                        hash,
                        ..
                    } => {
                        self.record_hit(&repo_root, &pattern, &heavy_dir, &hash);
                    }
                    JournalEntry::Touch { .. } => {}
                }
            }
        }
    }

    /// Publish the whole index atomically: temp file inside the
    /// snapshots directory, then one rename onto the final name.
    pub fn save(&self, root: &Path) -> io::Result<()> {
        self.save_durable(root)
    }

    /// Publish the whole index crash-durably: temp file inside the
    /// snapshots directory, fsync, rename onto final name, fsync parent dir.
    pub fn save_durable(&self, root: &Path) -> io::Result<()> {
        let dir = root.join("snapshots");
        fs::create_dir_all(&dir)?;
        let mut tmp = tempfile::NamedTempFile::new_in(&dir)?;
        for rec in &self.records {
            tmp.write_all(rec.serialize().as_bytes())?;
        }
        let target = index_path(root);
        tmp.as_file().sync_all()?;
        tmp.persist(&target).map_err(|e| e.error)?;
        crate::fsutil::sync_parent_dir(&target)
    }

    fn ensure_record(
        &mut self,
        repo_root: &str,
        pattern: &str,
        heavy_dir: &str,
    ) -> &mut SelectionRecord {
        if let Some(existing) = self
            .records
            .iter()
            .position(|r| r.matches(repo_root, pattern, heavy_dir))
        {
            // In bounds by the `position` match above.
            return &mut self.records[existing];
        }
        self.records.push(SelectionRecord {
            repo_root: repo_root.to_string(),
            pattern: pattern.to_string(),
            heavy_dir: heavy_dir.to_string(),
            ring: Vec::new(),
            mtime_secs: 0,
        });
        let last = self.records.len() - 1;
        // In bounds by construction: we just pushed it.
        &mut self.records[last]
    }

    /// Move `hash` to the front of the key's ring (dedup, truncate to
    /// [`MAX_RING`]) and refresh the mtime. Called after every
    /// successful snapshot PUBLISH, full or incremental.
    pub fn record_publish(
        &mut self,
        repo_root: &str,
        pattern: &str,
        heavy_dir: &str,
        hash: &ContentId,
        now_secs: u64,
    ) {
        let rec = self.ensure_record(repo_root, pattern, heavy_dir);
        rec.ring.retain(|h| h != hash);
        rec.ring.insert(0, *hash);
        rec.ring.truncate(MAX_RING);
        rec.mtime_secs = now_secs;
    }

    /// On a snapshot HIT: move-to-front when not already front, leave
    /// everything else alone. A hash unknown to the index (published
    /// before the index existed) joins the ring.
    pub fn record_hit(
        &mut self,
        repo_root: &str,
        pattern: &str,
        heavy_dir: &str,
        hash: &ContentId,
    ) {
        let rec = self.ensure_record(repo_root, pattern, heavy_dir);
        if rec.ring.first() == Some(hash) {
            return;
        }
        rec.ring.retain(|h| h != hash);
        rec.ring.insert(0, *hash);
        rec.ring.truncate(MAX_RING);
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Path of the snapshot WAL journal: `<root>/snapshots/journal.tsv`.
pub fn journal_path(root: &Path) -> PathBuf {
    root.join("snapshots").join("journal.tsv")
}

/// Path of the snapshot LRU sidecar: `<root>/snapshots/lru.tsv`.
pub fn lru_path(root: &Path) -> PathBuf {
    root.join("snapshots").join("lru.tsv")
}

/// Last-use registry for published snapshots (product-handoff §7.4
/// retention cap). One line per snapshot:
///
/// ```text
/// <hash>\t<last-use-unix-secs>
/// ```
///
/// Refreshed on every publish AND every hit that goes through the
/// free [`record_publish`]/[`record_hit`] entry points; GC reads it
/// to order unreferenced snapshots least-recently-used-first when the
/// count exceeds the retention cap. A snapshot missing from the file
/// falls back to its directory mtime (set at publish), so a lost or
/// truncated sidecar degrades ordering, never safety.
///
/// Same conventions as [`SelectionIndex`]: tolerant parse (bad lines
/// dropped silently), torn tail rule, atomic whole-file save, and
/// best-effort semantics — never worth failing a create over.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SnapshotLru {
    /// One `(hash, last-use unix secs)` pair per known snapshot,
    /// in file order.
    pub entries: Vec<(ContentId, u64)>,
}

impl SnapshotLru {
    /// Load `<root>/snapshots/lru.tsv` and replay `<root>/snapshots/journal.tsv`.
    /// Missing/unreadable means empty; malformed lines are dropped silently.
    pub fn load(root: &Path) -> SnapshotLru {
        let mut lru = Self::load_canonical(root);
        lru.apply_journal(root);
        lru
    }

    /// Load only the canonical `<root>/snapshots/lru.tsv` without journal replay.
    pub fn load_canonical(root: &Path) -> SnapshotLru {
        let mut lru = SnapshotLru::default();
        let Ok(text) = fs::read_to_string(lru_path(root)) else {
            return lru;
        };
        let complete = match text.strip_suffix('\n') {
            Some(body) => body,
            None => match text.rfind('\n') {
                Some(i) => &text[..i],
                None => return lru,
            },
        };
        for line in complete.split('\n').filter(|l| !l.is_empty()) {
            if let Some(entry) = Self::parse_line(line) {
                // Last writer wins on duplicates: newest physical line
                // is the most recent state.
                match lru.entries.iter_mut().find(|(h, _)| *h == entry.0) {
                    Some(slot) => slot.1 = entry.1,
                    None => lru.entries.push(entry),
                }
            }
        }
        lru
    }

    /// Replay entries from `<root>/snapshots/journal.tsv`.
    pub fn apply_journal(&mut self, root: &Path) {
        let Ok(text) = fs::read_to_string(journal_path(root)) else {
            return;
        };
        let complete = match text.strip_suffix('\n') {
            Some(body) => body,
            None => match text.rfind('\n') {
                Some(i) => &text[..i],
                None => return,
            },
        };
        for line in complete.split('\n').filter(|l| !l.is_empty()) {
            if let Some(entry) = parse_journal_line(line) {
                match entry {
                    JournalEntry::Publish { hash, timestamp, .. }
                    | JournalEntry::Hit { hash, timestamp, .. }
                    | JournalEntry::Touch { hash, timestamp } => {
                        self.record(&hash, timestamp);
                    }
                }
            }
        }
    }

    /// Publish the whole sidecar atomically: temp file inside the
    /// snapshots directory, then one rename onto the final name.
    pub fn save(&self, root: &Path) -> io::Result<()> {
        self.save_durable(root)
    }

    /// Publish the whole sidecar crash-durably.
    pub fn save_durable(&self, root: &Path) -> io::Result<()> {
        let dir = root.join("snapshots");
        fs::create_dir_all(&dir)?;
        let mut tmp = tempfile::NamedTempFile::new_in(&dir)?;
        for (hash, secs) in &self.entries {
            tmp.write_all(format!("{hash}\t{secs}\n").as_bytes())?;
        }
        let target = lru_path(root);
        tmp.as_file().sync_all()?;
        tmp.persist(&target).map_err(|e| e.error)?;
        crate::fsutil::sync_parent_dir(&target)
    }

    /// Record `secs` as `hash`'s last use, inserting or overwriting.
    /// Last writer wins: callers pass their own clock reading.
    pub fn record(&mut self, hash: &ContentId, secs: u64) {
        match self.entries.iter_mut().find(|(h, _)| h == hash) {
            Some(slot) => slot.1 = secs,
            None => self.entries.push((*hash, secs)),
        }
    }

    /// The recorded last use of `hash`, if any.
    pub fn last_use(&self, hash: &ContentId) -> Option<u64> {
        self.entries
            .iter()
            .find(|(h, _)| h == hash)
            .map(|(_, secs)| *secs)
    }

    fn parse_line(line: &str) -> Option<(ContentId, u64)> {
        let (hex, secs) = line.split_once('\t')?;
        let secs_text = secs.split('\t').next()?;
        Some((ContentId::from_hex(hex)?, secs_text.parse().ok()?))
    }
}

enum JournalEntry {
    Publish {
        repo_root: String,
        pattern: String,
        heavy_dir: String,
        hash: ContentId,
        timestamp: u64,
    },
    Hit {
        repo_root: String,
        pattern: String,
        heavy_dir: String,
        hash: ContentId,
        timestamp: u64,
    },
    Touch {
        hash: ContentId,
        timestamp: u64,
    },
}

fn parse_journal_line(line: &str) -> Option<JournalEntry> {
    let f: Vec<&str> = line.split('\t').collect();
    if f.is_empty() {
        return None;
    }
    match f[0] {
        "publish" => {
            if f.len() != 6 {
                return None;
            }
            Some(JournalEntry::Publish {
                repo_root: crate::mirror::unescape(f[1]).ok()?,
                pattern: crate::mirror::unescape(f[2]).ok()?,
                heavy_dir: crate::mirror::unescape(f[3]).ok()?,
                hash: ContentId::from_hex(f[4])?,
                timestamp: f[5].parse().ok()?,
            })
        }
        "hit" => {
            if f.len() != 6 {
                return None;
            }
            Some(JournalEntry::Hit {
                repo_root: crate::mirror::unescape(f[1]).ok()?,
                pattern: crate::mirror::unescape(f[2]).ok()?,
                heavy_dir: crate::mirror::unescape(f[3]).ok()?,
                hash: ContentId::from_hex(f[4])?,
                timestamp: f[5].parse().ok()?,
            })
        }
        "touch" | "lru" => {
            if f.len() != 3 {
                return None;
            }
            Some(JournalEntry::Touch {
                hash: ContentId::from_hex(f[1])?,
                timestamp: f[2].parse().ok()?,
            })
        }
        _ => None,
    }
}

fn append_journal(root: &Path, line: &str) -> io::Result<()> {
    let dir = root.join("snapshots");
    fs::create_dir_all(&dir)?;
    let path = dir.join("journal.tsv");
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(line.as_bytes())?;
    Ok(())
}

/// Held `flock(2)` on `<root>/snapshots/metadata.lock` during sweep
/// compaction.
pub struct MetadataLock {
    file: fs::File,
}

impl MetadataLock {
    /// Acquire an exclusive lock on `<root>/snapshots/metadata.lock`.
    pub fn acquire(root: &Path) -> io::Result<Self> {
        let dir = root.join("snapshots");
        fs::create_dir_all(&dir)?;
        let path = dir.join("metadata.lock");
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        use std::os::unix::io::AsRawFd;
        let fd = file.as_raw_fd();
        // SAFETY: flock(2) takes only an fd and constants; the fd is
        // valid for as long as `file` is alive.
        if unsafe { libc::flock(fd, libc::LOCK_EX) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { file })
    }
}

impl Drop for MetadataLock {
    fn drop(&mut self) {
        use std::os::unix::io::AsRawFd;
        let fd = self.file.as_raw_fd();
        // SAFETY: fd is valid for the lifetime of self.file.
        unsafe { libc::flock(fd, libc::LOCK_UN) };
    }
}

/// Compact `<root>/snapshots/journal.tsv` into canonical `index.tsv` and `lru.tsv`.
///
/// Acquires an exclusive metadata lock (`<root>/snapshots/metadata.lock`),
/// merges uncompacted journal records with existing canonical index/LRU files,
/// crash-durably saves both files (with fsync), and truncates `journal.tsv`.
pub fn compact_journal(root: &Path) -> io::Result<()> {
    let dir = root.join("snapshots");
    if !dir.exists() {
        return Ok(());
    }
    let _lock = MetadataLock::acquire(root)?;
    let j_path = journal_path(root);
    let mut journal_file = match fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&j_path)
    {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };

    use std::io::{Read, Seek};
    let mut bytes = Vec::new();
    journal_file.read_to_end(&mut bytes)?;
    if bytes.is_empty() {
        return Ok(());
    }

    let last_newline = match bytes.iter().rposition(|&b| b == b'\n') {
        Some(pos) => pos + 1,
        None => return Ok(()),
    };

    let complete_text = match std::str::from_utf8(&bytes[..last_newline]) {
        Ok(s) => s,
        Err(_) => return Ok(()),
    };

    let mut idx = SelectionIndex::load_canonical(root);
    let mut lru = SnapshotLru::load_canonical(root);
    for line in complete_text.split('\n').filter(|l| !l.is_empty()) {
        if let Some(entry) = parse_journal_line(line) {
            match entry {
                JournalEntry::Publish {
                    repo_root,
                    pattern,
                    heavy_dir,
                    hash,
                    timestamp,
                } => {
                    idx.record_publish(&repo_root, &pattern, &heavy_dir, &hash, timestamp);
                    lru.record(&hash, timestamp);
                }
                JournalEntry::Hit {
                    repo_root,
                    pattern,
                    heavy_dir,
                    hash,
                    timestamp,
                } => {
                    idx.record_hit(&repo_root, &pattern, &heavy_dir, &hash);
                    lru.record(&hash, timestamp);
                }
                JournalEntry::Touch { hash, timestamp } => {
                    lru.record(&hash, timestamp);
                }
            }
        }
    }

    idx.save_durable(root)?;
    lru.save_durable(root)?;

    let current_len = journal_file.metadata()?.len() as usize;
    if current_len > last_newline {
        journal_file.seek(io::SeekFrom::Start(last_newline as u64))?;
        let mut remaining = Vec::new();
        journal_file.read_to_end(&mut remaining)?;
        journal_file.seek(io::SeekFrom::Start(0))?;
        journal_file.write_all(&remaining)?;
        journal_file.set_len(remaining.len() as u64)?;
    } else {
        journal_file.seek(io::SeekFrom::Start(0))?;
        journal_file.set_len(0)?;
    }
    journal_file.sync_all()?;
    crate::fsutil::sync_parent_dir(&j_path)?;
    Ok(())
}

/// LRU-only last-use refresh, for callers that skip the v2 selection
/// index. Appends a touch entry to the write-ahead journal using O_APPEND.
pub fn record_snapshot_lru_touch(root: &Path, hash: &ContentId) {
    let now = now_secs();
    let line = format!("touch\t{}\t{}\n", hash, now);
    let _ = append_journal(root, &line);
}

/// After a successful PUBLISH (full or incremental): append a publish entry
/// to the write-ahead journal using atomic POSIX O_APPEND.
pub fn record_publish(
    root: &Path,
    repo_root: &str,
    pattern: &str,
    heavy_dir: &str,
    hash: &ContentId,
) -> io::Result<()> {
    let now = now_secs();
    let line = format!(
        "publish\t{}\t{}\t{}\t{}\t{}\n",
        escape(repo_root),
        escape(pattern),
        escape(heavy_dir),
        hash,
        now
    );
    append_journal(root, &line)
}

/// After a snapshot HIT: append a hit entry to the write-ahead journal
/// using atomic POSIX O_APPEND.
pub fn record_hit(
    root: &Path,
    repo_root: &str,
    pattern: &str,
    heavy_dir: &str,
    hash: &ContentId,
) -> io::Result<()> {
    let now = now_secs();
    let line = format!(
        "hit\t{}\t{}\t{}\t{}\t{}\n",
        escape(repo_root),
        escape(pattern),
        escape(heavy_dir),
        hash,
        now
    );
    append_journal(root, &line)
}

/// Pick the old snapshot to diff against: walk the key's ring newest
/// first; the first candidate that is a VALID published snapshot (dir
/// exists, `.complete` present, manifest parses and validates) wins.
/// `None` means the caller does a plain full build.
pub fn select_old_snapshot(
    root: &Path,
    repo_root: &str,
    pattern: &str,
    heavy_dir: &str,
) -> Option<(ContentId, Manifest)> {
    let idx = SelectionIndex::load(root);
    let rec = idx
        .records
        .iter()
        .find(|r| r.matches(repo_root, pattern, heavy_dir))?;
    for hash in &rec.ring {
        if let Some(manifest) = read_published(root, hash) {
            return Some((*hash, manifest));
        }
    }
    None
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u8) -> ContentId {
        let mut bytes = [0u8; 32];
        bytes[0] = n;
        bytes[31] = n;
        ContentId(bytes)
    }

    const ROOT_A: &str = "/repos/alpha";
    const PAT: &str = "node_modules/";
    const HEAVY: &str = "node_modules";

    #[test]
    fn round_trips_weird_paths_and_multiple_keys() {
        let base = tempfile::tempdir().unwrap();
        let store = base.path();
        let mut idx = SelectionIndex::default();
        idx.record_publish(
            "/repo %40/we\tird",
            "%09pattern\n",
            "heavy%dir",
            &id(1),
            100,
        );
        idx.record_publish(ROOT_A, PAT, HEAVY, &id(2), 200);

        idx.save(store).expect("save");
        let back = SelectionIndex::load(store);
        assert_eq!(back, idx, "escaped round-trip must be lossless");
    }

    #[test]
    fn ring_dedups_moves_to_front_and_truncates_at_three() {
        let mut idx = SelectionIndex::default();
        idx.record_publish(ROOT_A, PAT, HEAVY, &id(1), 1);
        idx.record_publish(ROOT_A, PAT, HEAVY, &id(2), 2);
        idx.record_publish(ROOT_A, PAT, HEAVY, &id(3), 3);
        assert_eq!(
            idx.records[0].ring,
            vec![id(3), id(2), id(1)],
            "newest first"
        );

        // A fourth publish pushes the oldest out.
        idx.record_publish(ROOT_A, PAT, HEAVY, &id(4), 4);
        assert_eq!(idx.records[0].ring, vec![id(4), id(3), id(2)]);

        // Republishing an existing hash moves it up without duplicating.
        idx.record_publish(ROOT_A, PAT, HEAVY, &id(3), 5);
        assert_eq!(idx.records[0].ring, vec![id(3), id(4), id(2)]);
        assert_eq!(idx.records[0].mtime_secs, 5);

        // Hit that is already front: no change at all.
        idx.record_hit(ROOT_A, PAT, HEAVY, &id(3));
        assert_eq!(idx.records[0].ring, vec![id(3), id(4), id(2)]);
        assert_eq!(idx.records[0].mtime_secs, 5, "hit must not refresh mtime");

        // Hit deeper in the ring reorders without touching mtime.
        idx.record_hit(ROOT_A, PAT, HEAVY, &id(2));
        assert_eq!(idx.records[0].ring, vec![id(2), id(3), id(4)]);
        assert_eq!(idx.records[0].mtime_secs, 5);

        // Distinct keys stay independent.
        assert_eq!(idx.records.len(), 1);
        idx.record_publish("/other", PAT, HEAVY, &id(9), 7);
        assert_eq!(idx.records.len(), 2);
        assert_eq!(idx.records[1].ring, vec![id(9)]);
    }

    #[test]
    fn corrupt_lines_are_dropped_silently() {
        let base = tempfile::tempdir().unwrap();
        let dir = base.path().join("snapshots");
        fs::create_dir_all(&dir).unwrap();
        let good = format!("{}\t{}\t{}\t{}\t42\n", ROOT_A, PAT, HEAVY, id(5));
        let text = format!(
            "{good}not enough fields\n{ROOT_A}\tp\t{HEAVY}\tnot-hex,also-bad\t7\n\
             {ROOT_A}\t{PAT}\t{HEAVY}\tbad-hex\t9\ngarbage-without-tabs\n\
             {ROOT_A}%leftover"
        );
        fs::write(dir.join("index.tsv"), text).unwrap();

        let idx = SelectionIndex::load(base.path());
        assert_eq!(idx.records.len(), 1, "only the good line survives");
        assert_eq!(idx.records[0].ring, vec![id(5)]);
        assert_eq!(idx.records[0].mtime_secs, 42);
    }

    #[test]
    fn missing_file_and_torn_tail_are_tolerated() {
        let base = tempfile::tempdir().unwrap();
        assert_eq!(SelectionIndex::load(base.path()), SelectionIndex::default());

        let dir = base.path().join("snapshots");
        fs::create_dir_all(&dir).unwrap();
        let good = format!("{}\t{}\t{}\t{}\t1\n", ROOT_A, PAT, HEAVY, id(6));
        fs::write(dir.join("index.tsv"), format!("{good}{good}")).unwrap();
        // Second copy has no trailing newline: torn tail, dropped once.
        let idx = SelectionIndex::load(base.path());
        assert_eq!(idx.records.len(), 1);
    }

    #[test]
    fn record_publish_surfaces_io_errors_without_panicking_on_hostile_layouts() {
        // journal.tsv exists as a DIRECTORY: append cannot write it.
        // The error must come back as a plain Err for callers to ignore — the index is best-effort.
        let base = tempfile::tempdir().unwrap();
        fs::create_dir_all(base.path().join("snapshots/journal.tsv")).unwrap();
        assert!(super::record_publish(base.path(), ROOT_A, PAT, HEAVY, &id(7)).is_err());
        assert!(super::select_old_snapshot(base.path(), ROOT_A, PAT, HEAVY).is_none());

        // snapshots/ exists as a FILE: even create_dir_all fails.
        let base = tempfile::tempdir().unwrap();
        fs::write(base.path().join("snapshots"), "not a directory").unwrap();
        assert!(super::record_publish(base.path(), ROOT_A, PAT, HEAVY, &id(8)).is_err());
    }

    #[test]
    fn wal_journal_appends_and_reads_live_state_before_compaction() {
        let base = tempfile::tempdir().unwrap();
        let store = base.path();

        // Initially no journal or index files
        assert_eq!(SelectionIndex::load(store), SelectionIndex::default());
        assert_eq!(SnapshotLru::load(store), SnapshotLru::default());

        // Publish to journal
        super::record_publish(store, ROOT_A, PAT, HEAVY, &id(1)).unwrap();
        let j_path = journal_path(store);
        assert!(j_path.exists(), "journal.tsv must be created");

        let j_content = fs::read_to_string(&j_path).unwrap();
        assert!(
            j_content.starts_with("publish\t"),
            "journal must contain publish line"
        );

        // Canonical files should not exist yet before compaction
        assert!(!index_path(store).exists());
        assert!(!lru_path(store).exists());

        // Live state reads must see the journal entry immediately
        let idx = SelectionIndex::load(store);
        assert_eq!(idx.records.len(), 1);
        assert_eq!(idx.records[0].ring, vec![id(1)]);
        assert!(idx.records[0].mtime_secs > 0);

        let lru = SnapshotLru::load(store);
        assert!(lru.last_use(&id(1)).is_some());

        // Hit appends to journal
        super::record_hit(store, ROOT_A, PAT, HEAVY, &id(2)).unwrap();
        let idx2 = SelectionIndex::load(store);
        assert_eq!(idx2.records[0].ring, vec![id(2), id(1)]);

        // Touch appends to journal
        super::record_snapshot_lru_touch(store, &id(3));
        let lru2 = SnapshotLru::load(store);
        assert!(lru2.last_use(&id(3)).is_some());
    }

    #[test]
    fn compaction_merges_journal_into_canonical_files_and_truncates_journal() {
        let base = tempfile::tempdir().unwrap();
        let store = base.path();

        super::record_publish(store, ROOT_A, PAT, HEAVY, &id(1)).unwrap();
        super::record_publish(store, ROOT_A, PAT, HEAVY, &id(2)).unwrap();
        super::record_hit(store, ROOT_A, PAT, HEAVY, &id(1)).unwrap();
        super::record_snapshot_lru_touch(store, &id(3));

        // Compact journal into canonical files
        compact_journal(store).expect("compaction must succeed");

        // Canonical files must exist and journal must be truncated
        assert!(index_path(store).exists());
        assert!(lru_path(store).exists());
        let j_content = fs::read_to_string(journal_path(store)).unwrap();
        assert!(j_content.is_empty(), "journal must be truncated to 0 bytes");

        // Canonical files alone hold the complete state
        let idx = SelectionIndex::load_canonical(store);
        assert_eq!(idx.records.len(), 1);
        assert_eq!(idx.records[0].ring, vec![id(1), id(2)]);
        assert!(idx.records[0].mtime_secs > 0);

        let lru = SnapshotLru::load_canonical(store);
        assert_eq!(lru.entries.len(), 3);
        assert!(lru.last_use(&id(2)).is_some());

        // Subsequent publishes append fresh lines to truncated journal
        super::record_publish(store, ROOT_A, PAT, HEAVY, &id(4)).unwrap();
        let j_after = fs::read_to_string(journal_path(store)).unwrap();
        assert!(!j_after.is_empty());

        let idx_live = SelectionIndex::load(store);
        assert_eq!(idx_live.records[0].ring, vec![id(4), id(1), id(2)]);
    }

    #[test]
    fn journal_torn_tail_and_corrupt_entries_tolerated_during_compaction() {
        let base = tempfile::tempdir().unwrap();
        let store = base.path();
        let dir = store.join("snapshots");
        fs::create_dir_all(&dir).unwrap();

        let good1 = format!("publish\t{ROOT_A}\t{PAT}\t{HEAVY}\t{}\t10\n", id(1));
        let bad = "corrupt\tinvalid\tformat\n";
        let good2 = format!("publish\t{ROOT_A}\t{PAT}\t{HEAVY}\t{}\t20\n", id(2));
        let torn = format!("publish\t{ROOT_A}\t{PAT}\t{HEAVY}\t{}", id(3)); // no trailing newline

        fs::write(journal_path(store), format!("{good1}{bad}{good2}{torn}")).unwrap();

        // Load before compaction
        let idx = SelectionIndex::load(store);
        assert_eq!(idx.records.len(), 1);
        assert_eq!(idx.records[0].ring, vec![id(2), id(1)]);

        // Compact
        compact_journal(store).unwrap();

        // Canonical files have good entries
        let idx_canonical = SelectionIndex::load_canonical(store);
        assert_eq!(idx_canonical.records[0].ring, vec![id(2), id(1)]);

        // The torn tail was retained at the start of journal
        let rem_journal = fs::read_to_string(journal_path(store)).unwrap();
        assert_eq!(rem_journal, torn);
    }

    #[test]
    fn concurrent_journal_appends_and_compaction_stress() {
        use std::sync::Arc;
        use std::thread;

        let base = tempfile::tempdir().unwrap();
        let store = Arc::new(base.path().to_path_buf());

        let num_threads = 8;
        let ops_per_thread = 50;

        let mut handles = Vec::new();
        for t in 0..num_threads {
            let s = Arc::clone(&store);
            handles.push(thread::spawn(move || {
                for i in 0..ops_per_thread {
                    let h = id(((t * ops_per_thread + i) % 250 + 1) as u8);
                    let key = format!("/repo/{t}");
                    if i % 3 == 0 {
                        super::record_publish(&s, &key, "pat/", "heavy", &h).unwrap();
                    } else if i % 3 == 1 {
                        super::record_hit(&s, &key, "pat/", "heavy", &h).unwrap();
                    } else {
                        super::record_snapshot_lru_touch(&s, &h);
                    }
                    if i % 15 == 0 {
                        let _ = compact_journal(&s);
                    }
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        // Final compaction
        compact_journal(&store).unwrap();

        let lru = SnapshotLru::load_canonical(&store);
        assert!(!lru.entries.is_empty(), "LRU should have recorded entries");

        let idx = SelectionIndex::load_canonical(&store);
        assert_eq!(idx.records.len(), num_threads, "All thread keys must be present in index");
    }

    #[test]
    fn select_walks_ring_newest_first_and_skips_invalid_candidates() {
        use crate::Store as _;
        use crate::snapshot::{Manifest, SnapshotEntry};

        let base = tempfile::tempdir().unwrap();
        let mut store = crate::DiskStore::open(base.path().join("store")).unwrap();

        let blob = store.put(b"content").unwrap();
        let entries = vec![SnapshotEntry::file("f.txt", blob, 0o644)];
        let m_old = Manifest::new(entries).unwrap();
        assert_eq!(
            store
                .publish_snapshot(m_old.entries.clone(), false)
                .unwrap(),
            crate::PublishOutcome::Published
        );
        super::record_publish(store.root(), ROOT_A, PAT, HEAVY, &m_old.hash).unwrap();

        // A second, NEWER publish; then the index points at both, new
        // first. Selection must return the newest valid one.
        let blob2 = store.put(b"content two").unwrap();
        let entries2 = vec![
            SnapshotEntry::file("f.txt", blob2, 0o644),
            SnapshotEntry::dir("d"),
        ];
        let m_new = Manifest::new(entries2).unwrap();
        assert_eq!(
            store
                .publish_snapshot(m_new.entries.clone(), false)
                .unwrap(),
            crate::PublishOutcome::Published
        );
        super::record_publish(store.root(), ROOT_A, PAT, HEAVY, &m_new.hash).unwrap();

        let (picked, manifest) =
            select_old_snapshot(store.root(), ROOT_A, PAT, HEAVY).expect("a usable candidate");
        assert_eq!(picked, m_new.hash);
        assert_eq!(manifest, m_new);

        // Corrupt the newest: selection skips it and falls back to the
        // older ring entry.
        fs::write(
            crate::snapshot::snapshot_path(store.root(), &m_new.hash).join("manifest.tsv"),
            "garbage\n",
        )
        .unwrap();
        let (picked, manifest) =
            select_old_snapshot(store.root(), ROOT_A, PAT, HEAVY).expect("older still usable");
        assert_eq!(picked, m_old.hash);
        assert_eq!(manifest, m_old);

        // Corrupt everything: None -> caller builds from scratch.
        fs::write(
            crate::snapshot::snapshot_path(store.root(), &m_old.hash).join("manifest.tsv"),
            "",
        )
        .unwrap();
        assert!(select_old_snapshot(store.root(), ROOT_A, PAT, HEAVY).is_none());
        assert!(select_old_snapshot(store.root(), "/unknown", PAT, HEAVY).is_none());
        store.flush().unwrap();
    }

    #[test]
    fn lru_sidecar_round_trips_and_tolerates_garbage() {
        let base = tempfile::tempdir().unwrap();
        let store = base.path();

        // Missing file: empty registry.
        assert_eq!(SnapshotLru::load(store), SnapshotLru::default());

        let mut lru = SnapshotLru::default();
        lru.record(&id(1), 100);
        lru.record(&id(2), 200);
        lru.record(&id(1), 150); // upsert, no duplicate line
        lru.save(store).unwrap();
        let back = SnapshotLru::load(store);
        assert_eq!(back, lru);
        assert_eq!(back.entries.len(), 2);
        assert_eq!(back.last_use(&id(1)), Some(150));
        assert_eq!(back.last_use(&id(2)), Some(200));
        assert_eq!(back.last_use(&id(3)), None);

        // Garbage lines dropped; torn tail (no trailing newline)
        // dropped too; duplicate keys resolved newest-line-wins.
        let good = format!("{}\t42\n", id(5));
        fs::write(
            lru_path(store),
            format!(
                "not-hex\t7\n{good}{}\tnot-a-number\n{}extra\t9\n{good}",
                id(6),
                id(7)
            ),
        )
        .unwrap();
        let lru = SnapshotLru::load(store);
        assert_eq!(lru.entries, vec![(id(5), 42)]);
    }

    #[test]
    fn publish_and_hit_refresh_the_lru_sidecar() {
        // The free entry points are what the CLI calls on publishes
        // and hits: each must leave a fresh last-use stamp behind.
        let base = tempfile::tempdir().unwrap();
        let store = base.path();
        super::record_publish(store, ROOT_A, PAT, HEAVY, &id(9)).unwrap();
        let stamp = SnapshotLru::load(store).last_use(&id(9));
        assert!(
            stamp.is_some_and(|s| s >= now_secs() - 2),
            "publish must stamp the LRU sidecar ({stamp:?})"
        );

        // A hit re-stamps: still exactly one entry for the hash.
        super::record_hit(store, ROOT_A, PAT, HEAVY, &id(9)).unwrap();
        let lru = SnapshotLru::load(store);
        assert_eq!(lru.entries.len(), 1);
        assert_eq!(lru.entries[0].0, id(9));
        assert!(
            lru.entries[0].1 >= now_secs() - 2,
            "hit must leave a fresh stamp"
        );
    }

    #[test]
    fn lru_touch_survives_hostile_layouts_silently() {
        // snapshots/ exists as a FILE: the LRU touch is best-effort
        // and must never panic; the index save surfaces its own
        // error as before.
        let base = tempfile::tempdir().unwrap();
        fs::write(base.path().join("snapshots"), "not a directory").unwrap();
        assert!(super::record_publish(base.path(), ROOT_A, PAT, HEAVY, &id(9)).is_err());
        let _ = super::record_hit(base.path(), ROOT_A, PAT, HEAVY, &id(9));
    }
}
