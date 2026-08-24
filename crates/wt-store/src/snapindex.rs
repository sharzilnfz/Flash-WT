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
    /// Load `<root>/snapshots/index.tsv`. A missing or unreadable
    /// file is an empty index; bad lines are dropped silently.
    pub fn load(root: &Path) -> SelectionIndex {
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

    /// Publish the whole index atomically: temp file inside the
    /// snapshots directory, then one rename onto the final name.
    pub fn save(&self, root: &Path) -> io::Result<()> {
        let dir = root.join("snapshots");
        fs::create_dir_all(&dir)?;
        let mut tmp = tempfile::NamedTempFile::new_in(&dir)?;
        for rec in &self.records {
            tmp.write_all(rec.serialize().as_bytes())?;
        }
        tmp.persist(index_path(root)).map_err(|e| e.error)?;
        Ok(())
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

/// After a successful PUBLISH (full or incremental): load, move the
/// published hash to the front, refresh the mtime, save atomically.
pub fn record_publish(
    root: &Path,
    repo_root: &str,
    pattern: &str,
    heavy_dir: &str,
    hash: &ContentId,
) -> io::Result<()> {
    let mut idx = SelectionIndex::load(root);
    idx.record_publish(repo_root, pattern, heavy_dir, hash, now_secs());
    idx.save(root)
}

/// After a snapshot HIT: load, move-to-front if not already there,
/// save atomically. The mtime is untouched.
pub fn record_hit(
    root: &Path,
    repo_root: &str,
    pattern: &str,
    heavy_dir: &str,
    hash: &ContentId,
) -> io::Result<()> {
    let mut idx = SelectionIndex::load(root);
    idx.record_hit(repo_root, pattern, heavy_dir, hash);
    idx.save(root)
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
        // index.tsv exists as a DIRECTORY: load cannot read it and
        // save cannot rename over it. The error must come back as a
        // plain Err for callers to ignore — the index is best-effort.
        let base = tempfile::tempdir().unwrap();
        fs::create_dir_all(base.path().join("snapshots/index.tsv")).unwrap();
        assert!(super::record_publish(base.path(), ROOT_A, PAT, HEAVY, &id(7)).is_err());
        assert!(super::select_old_snapshot(base.path(), ROOT_A, PAT, HEAVY).is_none());

        // snapshots/ exists as a FILE: even create_dir_all fails.
        let base = tempfile::tempdir().unwrap();
        fs::write(base.path().join("snapshots"), "not a directory").unwrap();
        assert!(super::record_publish(base.path(), ROOT_A, PAT, HEAVY, &id(8)).is_err());
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
        {
            let mut idx = crate::snapindex::SelectionIndex::load(store.root());
            idx.record_publish(ROOT_A, PAT, HEAVY, &m_new.hash, now_secs());
            idx.save(store.root()).unwrap();
        }

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
}
