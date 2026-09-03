use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ContentId;

const CACHE_FILE: &str = "ingest-cache.tsv";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub size: u64,

    pub mtime: SystemTime,

    pub inode: u64,

    pub ctime: SystemTime,

    pub id: ContentId,
}

pub struct ValidationCache {
    root: PathBuf,
    path: PathBuf,
    entries: BTreeMap<String, Entry>,

    dirty: bool,
}

impl ValidationCache {
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

    pub fn record(&mut self, rel: String, entry: Entry) {
        self.entries.insert(rel, entry);
        self.dirty = true;
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

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

        let now_near = mtime + Duration::from_secs(1);
        assert_eq!(
            cache.lookup_at("file.txt", 50, mtime, 999, ctime, now_near),
            None
        );

        let now_far = mtime + Duration::from_secs(10);
        assert_eq!(
            cache.lookup_at("file.txt", 50, mtime, 999, ctime, now_far),
            Some(id)
        );

        assert_eq!(
            cache.lookup_at("file.txt", 50, mtime, 1000, ctime, now_far),
            None
        );

        let ctime_moved = ctime + Duration::from_secs(1);
        assert_eq!(
            cache.lookup_at("file.txt", 50, mtime, 999, ctime_moved, now_far),
            None
        );
    }
}
