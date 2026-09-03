use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ContentId;

const LEDGER_FILE: &str = "verified.tsv";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fingerprint {
    pub size: u64,

    pub mtime: SystemTime,
}

pub struct VerifiedLedger {
    root: PathBuf,
    path: PathBuf,
    entries: BTreeMap<ContentId, Fingerprint>,
    dirty: bool,
}

impl VerifiedLedger {
    pub fn open(root: &Path) -> VerifiedLedger {
        let path = root.join(LEDGER_FILE);
        let entries = fs::read_to_string(&path)
            .map(|text| parse(&text))
            .unwrap_or_default();
        VerifiedLedger {
            root: root.to_path_buf(),
            path,
            entries,
            dirty: false,
        }
    }

    pub fn matches(&self, id: &ContentId, size: u64, mtime: SystemTime) -> bool {
        self.entries
            .get(id)
            .is_some_and(|f| f.size == size && f.mtime == mtime)
    }

    pub fn record(&mut self, id: ContentId, fingerprint: Fingerprint) {
        self.entries.insert(id, fingerprint);
        self.dirty = true;
    }

    pub fn forget(&mut self, id: &ContentId) {
        if self.entries.remove(id).is_some() {
            self.dirty = true;
        }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn save_if_dirty(&mut self) -> io::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        let mut tmp = tempfile::NamedTempFile::new_in(&self.root)?;
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

        let Some(mtime) = UNIX_EPOCH.checked_add(Duration::new(secs, nanos)) else {
            continue;
        };
        out.insert(id, Fingerprint { size, mtime });
    }
    out
}
