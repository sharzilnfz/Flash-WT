use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ContentId;
use crate::mirror::escape;
use crate::snapshot::{Manifest, read_published};

pub const MAX_RING: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionRecord {
    pub repo_root: String,

    pub pattern: String,

    pub heavy_dir: String,

    pub ring: Vec<ContentId>,

    pub mtime_secs: u64,
}

impl SelectionRecord {
    pub fn matches(&self, repo_root: &str, pattern: &str, heavy_dir: &str) -> bool {
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SelectionIndex {
    pub records: Vec<SelectionRecord>,
}

fn index_path(root: &Path) -> PathBuf {
    root.join("snapshots").join("index.tsv")
}

impl SelectionIndex {
    pub fn load(root: &Path) -> SelectionIndex {
        let mut idx = Self::load_canonical(root);
        idx.apply_journal(root);
        idx
    }

    pub fn load_canonical(root: &Path) -> SelectionIndex {
        let mut idx = SelectionIndex::default();
        let Ok(text) = fs::read_to_string(index_path(root)) else {
            return idx;
        };

        let complete = match text.strip_suffix('\n') {
            Some(body) => body,
            None => match text.rfind('\n') {
                Some(i) => &text[..i],
                None => return idx,
            },
        };
        for line in complete.split('\n').filter(|l| !l.is_empty()) {
            if let Some(rec) = SelectionRecord::parse_line(line) {
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

        &mut self.records[last]
    }

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

pub fn journal_path(root: &Path) -> PathBuf {
    root.join("snapshots").join("journal.tsv")
}

pub fn lru_path(root: &Path) -> PathBuf {
    root.join("snapshots").join("lru.tsv")
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SnapshotLru {
    pub entries: Vec<(ContentId, u64)>,
}

impl SnapshotLru {
    pub fn load(root: &Path) -> SnapshotLru {
        let mut lru = Self::load_canonical(root);
        lru.apply_journal(root);
        lru
    }

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
                match lru.entries.iter_mut().find(|(h, _)| *h == entry.0) {
                    Some(slot) => slot.1 = entry.1,
                    None => lru.entries.push(entry),
                }
            }
        }
        lru
    }

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
                        hash, timestamp, ..
                    }
                    | JournalEntry::Hit {
                        hash, timestamp, ..
                    }
                    | JournalEntry::Touch { hash, timestamp } => {
                        self.record(&hash, timestamp);
                    }
                }
            }
        }
    }

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

    pub fn record(&mut self, hash: &ContentId, secs: u64) {
        match self.entries.iter_mut().find(|(h, _)| h == hash) {
            Some(slot) => slot.1 = secs,
            None => self.entries.push((*hash, secs)),
        }
    }

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

pub type MetadataLock = crate::fsutil::FlockGuard;

pub fn compact_journal(root: &Path) -> io::Result<()> {
    let dir = root.join("snapshots");
    if !dir.exists() {
        return Ok(());
    }
    let _lock = crate::fsutil::FlockGuard::lock_file_exclusive(&dir.join("metadata.lock"))?;
    let j_path = journal_path(root);
    let mut journal_file = match fs::OpenOptions::new().read(true).write(true).open(&j_path) {
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

pub fn record_snapshot_lru_touch(root: &Path, hash: &ContentId) {
    let now = now_secs();
    let line = format!("touch\t{}\t{}\n", hash, now);
    let _ = append_journal(root, &line);
}

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

        idx.save_durable(store).expect("save");
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

        idx.record_publish(ROOT_A, PAT, HEAVY, &id(4), 4);
        assert_eq!(idx.records[0].ring, vec![id(4), id(3), id(2)]);

        idx.record_publish(ROOT_A, PAT, HEAVY, &id(3), 5);
        assert_eq!(idx.records[0].ring, vec![id(3), id(4), id(2)]);
        assert_eq!(idx.records[0].mtime_secs, 5);

        idx.record_hit(ROOT_A, PAT, HEAVY, &id(3));
        assert_eq!(idx.records[0].ring, vec![id(3), id(4), id(2)]);
        assert_eq!(idx.records[0].mtime_secs, 5, "hit must not refresh mtime");

        idx.record_hit(ROOT_A, PAT, HEAVY, &id(2));
        assert_eq!(idx.records[0].ring, vec![id(2), id(3), id(4)]);
        assert_eq!(idx.records[0].mtime_secs, 5);

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

        let idx = SelectionIndex::load(base.path());
        assert_eq!(idx.records.len(), 1);
    }

    #[test]
    fn record_publish_surfaces_io_errors_without_panicking_on_hostile_layouts() {
        let base = tempfile::tempdir().unwrap();
        fs::create_dir_all(base.path().join("snapshots/journal.tsv")).unwrap();
        assert!(super::record_publish(base.path(), ROOT_A, PAT, HEAVY, &id(7)).is_err());
        assert!(super::select_old_snapshot(base.path(), ROOT_A, PAT, HEAVY).is_none());

        let base = tempfile::tempdir().unwrap();
        fs::write(base.path().join("snapshots"), "not a directory").unwrap();
        assert!(super::record_publish(base.path(), ROOT_A, PAT, HEAVY, &id(8)).is_err());
    }

    #[test]
    fn wal_journal_appends_and_reads_live_state_before_compaction() {
        let base = tempfile::tempdir().unwrap();
        let store = base.path();

        assert_eq!(SelectionIndex::load(store), SelectionIndex::default());
        assert_eq!(SnapshotLru::load(store), SnapshotLru::default());

        super::record_publish(store, ROOT_A, PAT, HEAVY, &id(1)).unwrap();
        let j_path = journal_path(store);
        assert!(j_path.exists(), "journal.tsv must be created");

        let j_content = fs::read_to_string(&j_path).unwrap();
        assert!(
            j_content.starts_with("publish\t"),
            "journal must contain publish line"
        );

        assert!(!index_path(store).exists());
        assert!(!lru_path(store).exists());

        let idx = SelectionIndex::load(store);
        assert_eq!(idx.records.len(), 1);
        assert_eq!(idx.records[0].ring, vec![id(1)]);
        assert!(idx.records[0].mtime_secs > 0);

        let lru = SnapshotLru::load(store);
        assert!(lru.last_use(&id(1)).is_some());

        super::record_hit(store, ROOT_A, PAT, HEAVY, &id(2)).unwrap();
        let idx2 = SelectionIndex::load(store);
        assert_eq!(idx2.records[0].ring, vec![id(2), id(1)]);

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

        compact_journal(store).expect("compaction must succeed");

        assert!(index_path(store).exists());
        assert!(lru_path(store).exists());
        let j_content = fs::read_to_string(journal_path(store)).unwrap();
        assert!(j_content.is_empty(), "journal must be truncated to 0 bytes");

        let idx = SelectionIndex::load_canonical(store);
        assert_eq!(idx.records.len(), 1);
        assert_eq!(idx.records[0].ring, vec![id(1), id(2)]);
        assert!(idx.records[0].mtime_secs > 0);

        let lru = SnapshotLru::load_canonical(store);
        assert_eq!(lru.entries.len(), 3);
        assert!(lru.last_use(&id(2)).is_some());

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
        let torn = format!("publish\t{ROOT_A}\t{PAT}\t{HEAVY}\t{}", id(3));

        fs::write(journal_path(store), format!("{good1}{bad}{good2}{torn}")).unwrap();

        let idx = SelectionIndex::load(store);
        assert_eq!(idx.records.len(), 1);
        assert_eq!(idx.records[0].ring, vec![id(2), id(1)]);

        compact_journal(store).unwrap();

        let idx_canonical = SelectionIndex::load_canonical(store);
        assert_eq!(idx_canonical.records[0].ring, vec![id(2), id(1)]);

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

        compact_journal(&store).unwrap();

        let lru = SnapshotLru::load_canonical(&store);
        assert!(!lru.entries.is_empty(), "LRU should have recorded entries");

        let idx = SelectionIndex::load_canonical(&store);
        assert_eq!(
            idx.records.len(),
            num_threads,
            "All thread keys must be present in index"
        );
    }

    #[test]
    fn select_walks_ring_newest_first_and_skips_invalid_candidates() {
        use crate::snapshot::{Manifest, PublishOptions, SnapshotEntry};

        let base = tempfile::tempdir().unwrap();
        let mut store = crate::DiskStore::open(base.path().join("store")).unwrap();

        let blob = store.put(b"content").unwrap();
        let entries = vec![SnapshotEntry::file("f.txt", blob, 0o644)];
        let m_old = Manifest::new(entries).unwrap();
        assert_eq!(
            store
                .publish_snapshot(m_old.entries.clone(), PublishOptions::default())
                .unwrap()
                .outcome,
            crate::PublishOutcome::Published
        );
        super::record_publish(store.root(), ROOT_A, PAT, HEAVY, &m_old.hash).unwrap();

        let blob2 = store.put(b"content two").unwrap();
        let entries2 = vec![
            SnapshotEntry::file("f.txt", blob2, 0o644),
            SnapshotEntry::dir("d"),
        ];
        let m_new = Manifest::new(entries2).unwrap();
        assert_eq!(
            store
                .publish_snapshot(m_new.entries.clone(), PublishOptions::default())
                .unwrap()
                .outcome,
            crate::PublishOutcome::Published
        );
        super::record_publish(store.root(), ROOT_A, PAT, HEAVY, &m_new.hash).unwrap();

        let (picked, manifest) =
            select_old_snapshot(store.root(), ROOT_A, PAT, HEAVY).expect("a usable candidate");
        assert_eq!(picked, m_new.hash);
        assert_eq!(manifest.hash, m_new.hash);
        assert_eq!(manifest.entries, m_new.entries);

        fs::write(
            crate::snapshot::snapshot_path(store.root(), &m_new.hash).join("manifest.tsv"),
            "garbage\n",
        )
        .unwrap();
        let (picked, manifest) =
            select_old_snapshot(store.root(), ROOT_A, PAT, HEAVY).expect("older still usable");
        assert_eq!(picked, m_old.hash);
        assert_eq!(manifest.hash, m_old.hash);
        assert_eq!(manifest.entries, m_old.entries);

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

        assert_eq!(SnapshotLru::load(store), SnapshotLru::default());

        let mut lru = SnapshotLru::default();
        lru.record(&id(1), 100);
        lru.record(&id(2), 200);
        lru.record(&id(1), 150);
        lru.save_durable(store).unwrap();
        let back = SnapshotLru::load(store);
        assert_eq!(back, lru);
        assert_eq!(back.entries.len(), 2);
        assert_eq!(back.last_use(&id(1)), Some(150));
        assert_eq!(back.last_use(&id(2)), Some(200));
        assert_eq!(back.last_use(&id(3)), None);

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
        let base = tempfile::tempdir().unwrap();
        let store = base.path();
        super::record_publish(store, ROOT_A, PAT, HEAVY, &id(9)).unwrap();
        let stamp = SnapshotLru::load(store).last_use(&id(9));
        assert!(
            stamp.is_some_and(|s| s >= now_secs() - 2),
            "publish must stamp the LRU sidecar ({stamp:?})"
        );

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
        let base = tempfile::tempdir().unwrap();
        fs::write(base.path().join("snapshots"), "not a directory").unwrap();
        assert!(super::record_publish(base.path(), ROOT_A, PAT, HEAVY, &id(9)).is_err());
        let _ = super::record_hit(base.path(), ROOT_A, PAT, HEAVY, &id(9));
    }
}
