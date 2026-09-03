use sha2::{Digest, Sha256};

use crate::ContentId;
use crate::mirror::unescape;

pub(super) const EXEC_FILE_MODE: u32 = 0o755;

pub(super) const PLAIN_FILE_MODE: u32 = 0o644;

pub(super) const DIR_MODE: u32 = 0o755;

const SYMLINK_MODE: u32 = 0o777;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EntryKind {
    File,

    Symlink,

    Dir,
}

impl EntryKind {
    fn as_str(self) -> &'static str {
        match self {
            EntryKind::File => "file",
            EntryKind::Symlink => "symlink",
            EntryKind::Dir => "dir",
        }
    }

    fn parse(text: &str) -> Option<EntryKind> {
        match text {
            "file" => Some(EntryKind::File),
            "symlink" => Some(EntryKind::Symlink),
            "dir" => Some(EntryKind::Dir),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotEntry {
    pub rel: String,

    pub kind: EntryKind,

    pub mode: u32,

    pub blob: Option<ContentId>,

    pub target: Option<String>,
}

impl SnapshotEntry {
    pub fn file(rel: impl Into<String>, blob: ContentId, raw_mode: u32) -> SnapshotEntry {
        SnapshotEntry {
            rel: rel.into(),
            kind: EntryKind::File,
            mode: normalized_file_mode(raw_mode),
            blob: Some(blob),
            target: None,
        }
    }

    pub fn dir(rel: impl Into<String>) -> SnapshotEntry {
        SnapshotEntry {
            rel: rel.into(),
            kind: EntryKind::Dir,
            mode: DIR_MODE,
            blob: None,
            target: None,
        }
    }

    pub fn symlink(rel: impl Into<String>, target: impl Into<String>) -> SnapshotEntry {
        SnapshotEntry {
            rel: rel.into(),
            kind: EntryKind::Symlink,
            mode: SYMLINK_MODE,
            blob: None,
            target: Some(target.into()),
        }
    }
}

fn normalized_file_mode(raw_mode: u32) -> u32 {
    if raw_mode & 0o111 != 0 {
        EXEC_FILE_MODE
    } else {
        PLAIN_FILE_MODE
    }
}

fn validate_rel(rel: &str) -> Result<(), String> {
    if rel.is_empty() {
        return Err("relpath is empty".into());
    }
    if rel.starts_with('/') {
        return Err(format!("relpath {rel:?} is absolute"));
    }
    for comp in rel.split('/') {
        match comp {
            "" => return Err(format!("relpath {rel:?} has an empty component")),
            "." | ".." => return Err(format!("relpath {rel:?} contains {comp:?}")),
            _ => {}
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub entries: Vec<SnapshotEntry>,

    pub hash: ContentId,

    pub lockfile_hash: Option<ContentId>,

    pub total_size: u64,
}

impl Manifest {
    pub fn new(entries: Vec<SnapshotEntry>) -> Result<Manifest, String> {
        Self::new_with_lockfile_and_size(entries, None, 0)
    }

    pub fn new_with_lockfile(
        entries: Vec<SnapshotEntry>,
        lockfile_hash: Option<ContentId>,
    ) -> Result<Manifest, String> {
        Self::new_with_lockfile_and_size(entries, lockfile_hash, 0)
    }

    pub fn new_with_lockfile_and_size(
        mut entries: Vec<SnapshotEntry>,
        lockfile_hash: Option<ContentId>,
        total_size: u64,
    ) -> Result<Manifest, String> {
        for e in &entries {
            validate_rel(&e.rel)?;
            match e.kind {
                EntryKind::File => {
                    if e.blob.is_none() || e.target.is_some() {
                        return Err(format!("file entry {} lacks a blob ref", e.rel));
                    }
                }
                EntryKind::Symlink => {
                    if e.target.is_none() || e.blob.is_some() {
                        return Err(format!("symlink entry {} lacks a target", e.rel));
                    }
                }
                EntryKind::Dir => {
                    if e.blob.is_some() || e.target.is_some() {
                        return Err(format!("dir entry {} carries a ref", e.rel));
                    }
                }
            }
        }

        entries.sort_by(|a, b| {
            a.rel
                .as_bytes()
                .cmp(b.rel.as_bytes())
                .then_with(|| a.kind.cmp(&b.kind))
        });

        if let Some(pair) = entries.windows(2).find(|w| w[0].rel == w[1].rel) {
            return Err(format!("duplicate relpath {:?}", pair[0].rel));
        }
        let body = serialize_entries(&entries);
        let mut hasher = Sha256::new();
        hasher.update(body.as_bytes());
        Ok(Manifest {
            entries,
            hash: ContentId(hasher.finalize().into()),
            lockfile_hash,
            total_size,
        })
    }

    #[must_use]
    pub fn serialize(&self) -> String {
        let mut out = String::new();
        out.push_str("v1\tmanifest-sha256\t");
        out.push_str(&self.hash.to_string());
        if let Some(lh) = &self.lockfile_hash {
            out.push_str("\tlockfile-sha256\t");
            out.push_str(&lh.to_string());
        }
        if self.total_size > 0 {
            out.push_str("\ttotal-bytes\t");
            out.push_str(&self.total_size.to_string());
        }
        out.push('\n');
        out.push_str(&serialize_entries(&self.entries));
        out
    }

    pub fn parse(text: &str) -> Result<Manifest, String> {
        let end = text
            .rfind('\n')
            .ok_or("manifest has no complete header line")?;
        let complete = &text[..=end];
        let (header, body) = complete
            .split_once('\n')
            .ok_or("manifest has no header line")?;
        let fields: Vec<&str> = header.split('\t').collect();
        if fields.len() < 3 || fields[0] != "v1" || fields[1] != "manifest-sha256" {
            return Err(format!("bad manifest header {header:?}"));
        }
        let claimed = ContentId::from_hex(fields[2])
            .ok_or_else(|| format!("malformed manifest hash {:?}", fields[2]))?;
        let mut lockfile_hash = None;
        let mut total_size = 0u64;
        let mut i = 3;
        while i < fields.len() {
            match fields[i] {
                "lockfile-sha256" => {
                    if i + 1 < fields.len() {
                        lockfile_hash =
                            Some(ContentId::from_hex(fields[i + 1]).ok_or_else(|| {
                                format!("malformed lockfile hash {:?}", fields[i + 1])
                            })?);
                        i += 2;
                    } else {
                        return Err(format!(
                            "missing value for lockfile-sha256 in header {header:?}"
                        ));
                    }
                }
                "total-bytes" | "size-bytes" => {
                    if i + 1 < fields.len() {
                        total_size = fields[i + 1]
                            .parse::<u64>()
                            .map_err(|_| format!("malformed total size {:?}", fields[i + 1]))?;
                        i += 2;
                    } else {
                        return Err(format!(
                            "missing value for total-bytes in header {header:?}"
                        ));
                    }
                }
                _ => {
                    i += 1;
                }
            }
        }
        let mut hasher = Sha256::new();
        hasher.update(body.as_bytes());
        let actual = ContentId(hasher.finalize().into());
        if claimed != actual {
            return Err(format!(
                "manifest hash mismatch: header says {claimed}, body hashes to {actual}"
            ));
        }
        let mut entries = Vec::new();

        let entry_text = body.trim_end_matches('\n');
        for line in entry_text.split('\n').filter(|l| !l.is_empty()) {
            let f: Vec<&str> = line.split('\t').collect();
            match f.as_slice() {
                ["entry", rel_esc, kind, mode, ref_esc] => {
                    let kind =
                        EntryKind::parse(kind).ok_or_else(|| format!("unknown kind {kind:?}"))?;
                    let mode_text = *mode;
                    let mode = u32::from_str_radix(mode_text, 8)
                        .map_err(|_| format!("malformed octal mode {mode_text:?}"))?;
                    let rel = unescape(rel_esc)?;
                    validate_rel(&rel)?;
                    entries.push(match kind {
                        EntryKind::File => {
                            let hex = unescape(ref_esc)?;
                            let blob = ContentId::from_hex(&hex)
                                .ok_or_else(|| format!("file entry has non-hex ref {hex:?}"))?;
                            SnapshotEntry {
                                rel,
                                kind,
                                mode,
                                blob: Some(blob),
                                target: None,
                            }
                        }
                        EntryKind::Symlink => SnapshotEntry {
                            rel,
                            kind,
                            mode,
                            blob: None,
                            target: Some(unescape(ref_esc)?),
                        },
                        EntryKind::Dir => {
                            if *ref_esc != "-" {
                                return Err(format!("dir entry {rel:?} has a non-dash ref"));
                            }
                            SnapshotEntry {
                                rel,
                                kind,
                                mode,
                                blob: None,
                                target: None,
                            }
                        }
                    });
                }
                _ => return Err(format!("malformed manifest entry line {line:?}")),
            }
        }

        entries.sort_by(|a, b| {
            a.rel
                .as_bytes()
                .cmp(b.rel.as_bytes())
                .then_with(|| a.kind.cmp(&b.kind))
        });
        if let Some(pair) = entries.windows(2).find(|w| w[0].rel == w[1].rel) {
            return Err(format!("duplicate relpath {:?}", pair[0].rel));
        }
        Ok(Manifest {
            entries,
            hash: claimed,
            lockfile_hash,
            total_size,
        })
    }
}

pub(super) fn serialize_entries(entries: &[SnapshotEntry]) -> String {
    let mut out = String::new();
    for e in entries {
        out.push_str("entry\t");
        out.push_str(&crate::mirror::escape(&e.rel));
        out.push('\t');
        out.push_str(e.kind.as_str());
        out.push('\t');
        out.push_str(&format!("{:o}", e.mode));
        out.push('\t');
        match (e.blob, &e.target) {
            (Some(blob), _) => out.push_str(&blob.to_string()),
            (None, Some(target)) => out.push_str(&crate::mirror::escape(target)),
            (None, None) => out.push('-'),
        }
        out.push('\n');
    }
    out
}
