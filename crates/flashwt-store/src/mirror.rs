use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use sha2::{Digest, Sha256};

use crate::ContentId;

#[must_use]
pub fn escape(field: &str) -> String {
    let mut out = String::with_capacity(field.len());
    for ch in field.chars() {
        match ch {
            '%' => out.push_str("%25"),
            '\t' => out.push_str("%09"),
            '\n' => out.push_str("%0A"),
            '\r' => out.push_str("%0D"),
            c => out.push(c),
        }
    }
    out
}

pub fn unescape(field: &str) -> Result<String, String> {
    let mut out = String::with_capacity(field.len());
    let mut chars = field.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let hi = chars.next().ok_or("field ends with a bare %")?;
        let lo = chars.next().ok_or("truncated % escape")?;
        let byte = u8::from_str_radix(&format!("{hi}{lo}"), 16)
            .map_err(|_| format!("malformed % escape %{hi}{lo}"))?;
        match byte {
            0x09 => out.push('\t'),
            0x0A => out.push('\n'),
            0x0D => out.push('\r'),
            0x25 => out.push('%'),
            other => return Err(format!("unexpected % escape %{hi}{lo} ({other:#04x})")),
        }
    }
    Ok(out)
}

#[must_use]
pub fn worktree_key(worktree: &str, gitdir: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"version=1\0");
    hasher.update(worktree.as_bytes());
    hasher.update(b"\0");
    hasher.update(gitdir.as_bytes());

    ContentId(hasher.finalize().into()).to_string()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreMirror {
    pub worktree: PathBuf,

    pub gitdir: PathBuf,

    pub base_branch: Option<String>,

    pub base_commit: Option<String>,

    pub files: BTreeSet<ContentId>,

    pub snapshots: BTreeSet<ContentId>,
}

impl StoreMirror {
    pub fn new(worktree: PathBuf, gitdir: PathBuf) -> StoreMirror {
        StoreMirror {
            worktree,
            gitdir,
            base_branch: None,
            base_commit: None,
            files: BTreeSet::new(),
            snapshots: BTreeSet::new(),
        }
    }

    pub fn serialize(&self) -> String {
        let mut out = String::new();
        out.push_str("v1\tworktree\t");
        out.push_str(&escape(&self.worktree.to_string_lossy()));
        out.push('\t');
        out.push_str(&escape(&self.gitdir.to_string_lossy()));
        out.push('\n');
        if let Some(ref base) = self.base_branch {
            out.push_str("base\t");
            out.push_str(&escape(base));
            if let Some(ref commit) = self.base_commit {
                out.push('\t');
                out.push_str(commit);
            }
            out.push('\n');
        }
        for id in &self.files {
            out.push_str("file\t");
            out.push_str(&id.to_string());
            out.push('\n');
        }
        for id in &self.snapshots {
            out.push_str("snapshot\t");
            out.push_str(&id.to_string());
            out.push('\n');
        }
        out
    }

    pub fn parse(text: &str) -> Result<StoreMirror, String> {
        let complete = match text.strip_suffix('\n') {
            Some(body) => body,
            None => match text.rfind('\n') {
                Some(i) => &text[..i],
                None => return Err("no complete records (header line is torn)".into()),
            },
        };
        let mut lines = complete.split('\n');
        let header = lines.next().ok_or("empty mirror")?;
        let fields: Vec<&str> = header.split('\t').collect();
        if fields.len() != 4 || fields[0] != "v1" || fields[1] != "worktree" {
            return Err(format!("bad v1 header line {header:?}"));
        }
        let worktree = unescape(fields[2])?;
        if worktree.is_empty() {
            return Err("empty worktree path in header".into());
        }
        let gitdir = unescape(fields[3])?;
        if gitdir.is_empty() {
            return Err("empty gitdir path in header".into());
        }
        let mut mirror = StoreMirror::new(PathBuf::from(worktree), PathBuf::from(gitdir));
        for line in lines {
            let fields: Vec<&str> = line.split('\t').collect();
            match fields.as_slice() {
                ["base", symbolic, commit] => {
                    if mirror.base_branch.is_some() {
                        return Err("duplicate base record".into());
                    }
                    mirror.base_branch = Some(unescape(symbolic)?);
                    mirror.base_commit = Some((*commit).to_string());
                }
                ["base", symbolic] => {
                    if mirror.base_branch.is_some() {
                        return Err("duplicate base record".into());
                    }
                    mirror.base_branch = Some(unescape(symbolic)?);
                    mirror.base_commit = None;
                }
                ["file", hex] | ["snapshot", hex] => {
                    let id = ContentId::from_hex(hex)
                        .ok_or_else(|| format!("malformed 64-hex id {hex:?}"))?;
                    if fields[0] == "file" {
                        if !mirror.files.insert(id) {
                            return Err(format!("duplicate file record {hex}"));
                        }
                    } else if !mirror.snapshots.insert(id) {
                        return Err(format!("duplicate snapshot record {hex}"));
                    }
                }
                [kind, ..] => {
                    return Err(format!("unknown record type {kind:?}"));
                }
                [] => return Err("empty record line".into()),
            }
        }
        Ok(mirror)
    }
}

pub fn mirror_path(root: &Path, worktree: &Path, gitdir: &Path) -> PathBuf {
    let key = worktree_key(&worktree.to_string_lossy(), &gitdir.to_string_lossy());
    root.join("worktrees").join(format!("{key}.tsv"))
}

pub fn publish(root: &Path, mirror: &StoreMirror) -> io::Result<PathBuf> {
    let dir = root.join("worktrees");
    fs::create_dir_all(dir.join("tmp"))?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir.join("tmp"))?;
    tmp.write_all(mirror.serialize().as_bytes())?;
    let final_path = mirror_path(root, &mirror.worktree, &mirror.gitdir);
    crate::fsutil::durable_write_then_rename(tmp.path(), &final_path)?;
    Ok(final_path)
}

pub fn remove(root: &Path, worktree: &Path, gitdir: &Path) -> io::Result<bool> {
    match fs::remove_file(mirror_path(root, worktree, gitdir)) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

#[derive(Debug)]
pub struct ReadMirror {
    pub path: PathBuf,

    pub modified: SystemTime,

    pub mirror: Result<StoreMirror, String>,
}

pub fn read_all(root: &Path) -> Vec<ReadMirror> {
    let mut out = Vec::new();
    let dir = root.join("worktrees");
    let Ok(entries) = fs::read_dir(&dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension() != Some(std::ffi::OsStr::new("tsv")) {
            continue;
        }
        let (modified, text) = match (fs::metadata(&path), fs::read_to_string(&path)) {
            (Ok(meta), Ok(text)) => (meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), text),
            _ => continue,
        };
        let mirror = StoreMirror::parse(&text);
        out.push(ReadMirror {
            path,
            modified,
            mirror,
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    fn mirror_with_weird_paths() -> StoreMirror {
        let mut m = StoreMirror::new(
            PathBuf::from("/tmp/a b/c\u{1f600}\tdir"),
            PathBuf::from("/tmp/x%20y/.git\nworktrees\nw"),
        );
        m.files.insert(ContentId([1u8; 32]));
        m.files.insert(ContentId([2u8; 32]));
        m.snapshots.insert(ContentId([3u8; 32]));
        m
    }

    #[test]
    fn escape_round_trips_tabs_newlines_percent_and_unicode() {
        let weird = "space and\ttab\nnewline\rret %25 literal %zz \u{1f600}";
        let escaped = escape(weird);
        assert!(!escaped.contains('\t'));
        assert!(!escaped.contains('\n'));
        assert_eq!(unescape(&escaped).unwrap(), weird);
    }

    #[test]
    fn unescape_rejects_unknown_escapes() {
        assert!(unescape("%41").is_err(), "%41 was never emitted");
        assert!(unescape("tail %").is_err());
        assert!(unescape("plain").is_ok());
    }

    #[test]
    fn mirror_serialization_round_trips_weird_paths() {
        let m = mirror_with_weird_paths();
        let text = m.serialize();

        assert_eq!(text.lines().count(), 4);
        assert_eq!(StoreMirror::parse(&text).unwrap(), m);
    }

    #[test]
    fn key_is_stable_and_identity_sensitive() {
        let a = worktree_key("/w/one", "/r/.git/worktrees/one");
        assert_eq!(a, worktree_key("/w/one", "/r/.git/worktrees/one"));
        assert_ne!(a, worktree_key("/w/two", "/r/.git/worktrees/one"));
        assert_ne!(a, worktree_key("/w/one", "/r/.git/worktrees/two"));
    }

    #[test]
    fn torn_final_line_is_ignored_not_fatal() {
        let m = mirror_with_weird_paths();
        let mut text = m.serialize();
        text.push_str("file\tdeadbeef");
        let parsed = StoreMirror::parse(&text).unwrap();
        assert_eq!(parsed, m, "the torn record must vanish, the rest hold");
    }

    #[test]
    fn missing_header_rejects_the_mirror() {
        assert!(StoreMirror::parse("file\t00\n").is_err());
        assert!(StoreMirror::parse("").is_err());
        assert!(StoreMirror::parse("garbage\n").is_err());
    }

    #[test]
    fn duplicate_header_rejects_the_mirror() {
        let m = mirror_with_weird_paths();
        let text = m.serialize();
        let header = text.lines().next().unwrap();
        assert!(StoreMirror::parse(&format!("{text}{header}\n")).is_err());
    }

    #[test]
    fn unknown_record_type_rejects_the_mirror() {
        let m = mirror_with_weird_paths();
        let mut text = m.serialize();
        text.push_str("future\twhatever\n");
        assert!(
            StoreMirror::parse(&text).is_err(),
            "v1 declares no optional records"
        );
    }

    #[test]
    fn malformed_id_rejects_the_mirror() {
        let m = mirror_with_weird_paths();
        let mut text = m.serialize();
        text.push_str("file\tnot-hex\n");
        assert!(StoreMirror::parse(&text).is_err());
    }

    #[test]
    fn publish_is_atomic_and_read_back() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("store");
        fs::create_dir_all(&root).expect("mkdir store");
        let m = mirror_with_weird_paths();

        let path = publish(&root, &m).expect("publish");
        assert_eq!(
            path,
            mirror_path(&root, &m.worktree, &m.gitdir),
            "published at the derived key"
        );
        let text = fs::read_to_string(&path).expect("read mirror");
        assert_eq!(StoreMirror::parse(&text).unwrap(), m);

        let found = read_all(&root);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].mirror.as_ref().unwrap(), &m);

        publish(&root, &m).expect("republish");
        assert_eq!(read_all(&root).len(), 1);
    }

    #[test]
    fn interrupted_publish_leaves_no_root_but_maybe_a_temp_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("store");
        fs::create_dir_all(root.join("worktrees").join("tmp")).expect("mkdir tmp");
        fs::write(root.join("worktrees/tmp/pid-123"), b"half written").expect("write tmp");

        assert!(read_all(&root).is_empty(), "tmp debris is not a root");

        let m = mirror_with_weird_paths();
        publish(&root, &m).expect("publish after simulated crash");
        assert_eq!(read_all(&root).len(), 1);
    }

    #[test]
    fn remove_is_idempotent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("store");
        fs::create_dir_all(&root).expect("mkdir");
        let m = mirror_with_weird_paths();
        publish(&root, &m).expect("publish");
        assert!(remove(&root, &m.worktree, &m.gitdir).expect("remove"));
        assert!(!remove(&root, &m.worktree, &m.gitdir).expect("remove again"));
        assert!(read_all(&root).is_empty());
    }

    #[test]
    fn base_branch_record_round_trips_and_escapes() {
        let mut m = StoreMirror::new(
            PathBuf::from("/worktree/feature-1"),
            PathBuf::from("/repo/.git/worktrees/feature-1"),
        );
        m.base_branch = Some("feat/special\tbranch\n%name".into());
        m.base_commit = Some("0123456789abcdef0123456789abcdef01234567".into());
        m.files.insert(ContentId([42u8; 32]));

        let text = m.serialize();
        assert!(text.contains(
            "base\tfeat/special%09branch%0A%25name\t0123456789abcdef0123456789abcdef01234567\n"
        ));
        let parsed = StoreMirror::parse(&text).expect("parse");
        assert_eq!(parsed, m);
    }

    #[test]
    fn duplicate_base_record_rejects_the_mirror() {
        let text =
            "v1\tworktree\t/w/one\t/r/.git/worktrees/one\nbase\tmain\t1111\nbase\tmain\t2222\n";
        assert!(StoreMirror::parse(text).is_err());
    }
}
