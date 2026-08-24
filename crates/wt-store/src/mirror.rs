//! Store-local worktree mirrors (fast-hydration ticket 07).
//!
//! A mirror is the authoritative GC root for one hydrated worktree,
//! kept inside the store (`<root>/worktrees/<key>.tsv`) so a sweep
//! never has to discover repositories scattered across the machine.
//! One atomic write per successful create replaces thousands of
//! per-blob refcount writes as bookkeeping.
//!
//! The key is the SHA-256 hex of the identity string
//!
//! ```text
//! version=1 \0 <canonical worktree path> \0 <canonical gitdir path>
//! ```
//!
//! where both paths went through [`std::fs::canonicalize`] at write
//! time. The canonical paths themselves are stored INSIDE the file as
//! the `v1` header, so readers validate roots from the file alone.
//!
//! Record format (tab-separated, one record per line; `<TAB>` marks
//! a literal tab byte):
//!
//! ```text
//! v1<TAB>worktree<TAB><escaped-worktree-path><TAB><escaped-gitdir-path>
//! file<TAB><64-hex-blob-id>
//! snapshot<TAB><64-hex-manifest-hash>
//! ```
//!
//! Escaping: paths are UTF-8 percent-escaped. The four sequences this
//! module emits are `%25` (%), `%09` (tab), `%0A` (newline), `%0D`
//! (carriage return) — exactly the bytes the TSV framing cannot carry
//! verbatim. Decoding rejects every other `%xx` sequence rather than
//! guessing.
//!
//! Validity rules (mirroring the handoff plan):
//!
//! - A mirror is valid only if it begins with exactly one `v1` header
//!   line.
//! - A final line without a terminal newline is ignored (a torn
//!   append can produce one; the atomic publish never does).
//! - Unknown record types are rejected: the mirror parses as invalid
//!   and is preserved on disk for diagnosis. v1 declares no optional
//!   records.

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use sha2::{Digest, Sha256};

use crate::ContentId;

/// Escape one TSV field: percent-encode the framing bytes.
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

/// Inverse of [`escape`]. Only the four emitted sequences decode;
/// anything else means the file was edited underneath us.
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

/// SHA-256 hex of the canonicalized identity string. `worktree` and
/// `gitdir` must already be canonical ([`std::fs::canonicalize`] at
/// the write site); this function deliberately does not touch the
/// filesystem so tests can derive keys for paths that never existed.
#[must_use]
pub fn worktree_key(worktree: &str, gitdir: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"version=1\0");
    hasher.update(worktree.as_bytes());
    hasher.update(b"\0");
    hasher.update(gitdir.as_bytes());
    // ContentId's Display IS the lowercase hex encoder; no local
    // hand-rolled one needed.
    ContentId(hasher.finalize().into()).to_string()
}

/// The parsed contents of one mirror file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreMirror {
    /// Canonical path of the worktree directory.
    pub worktree: PathBuf,
    /// Canonical path of the worktree's git dir (the directory that
    /// holds the `wt-hydrated.tsv` sidecar).
    pub gitdir: PathBuf,
    /// Every distinct blob the hydration placed, one `file` record
    /// each.
    pub files: BTreeSet<ContentId>,
    /// Published snapshot manifests this worktree hydrates from, one
    /// `snapshot` record each (v2 feature; readers handle the record
    /// type from day one so the format does not fork).
    pub snapshots: BTreeSet<ContentId>,
}

impl StoreMirror {
    /// An empty mirror for the given worktree and its git directory.
    pub fn new(worktree: PathBuf, gitdir: PathBuf) -> StoreMirror {
        StoreMirror {
            worktree,
            gitdir,
            files: BTreeSet::new(),
            snapshots: BTreeSet::new(),
        }
    }

    /// Render the mirror as TSV (see the module-level format docs).
    pub fn serialize(&self) -> String {
        let mut out = String::new();
        out.push_str("v1\tworktree\t");
        out.push_str(&escape(&self.worktree.to_string_lossy()));
        out.push('\t');
        out.push_str(&escape(&self.gitdir.to_string_lossy()));
        out.push('\n');
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

    /// Parse mirror text. See the module header for validity rules.
    pub fn parse(text: &str) -> Result<StoreMirror, String> {
        // Ignore a torn final line: whatever follows the last newline
        // never got its terminal newline and is not a record.
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

/// Final path of the mirror for one canonicalized (worktree, gitdir)
/// pair: `<root>/worktrees/<key>.tsv`.
pub fn mirror_path(root: &Path, worktree: &Path, gitdir: &Path) -> PathBuf {
    let key = worktree_key(&worktree.to_string_lossy(), &gitdir.to_string_lossy());
    root.join("worktrees").join(format!("{key}.tsv"))
}

/// Publish a mirror atomically AND crash-durably: serialized bytes go
/// to a temp file in `<root>/worktrees/tmp/`, are fsynced, then one
/// rename lands them at the final name and the parent directory is
/// fsynced. A mirror is THE GC root for a live worktree — a crash
/// that landed the rename with unwritten data could make a live
/// worktree's root appear empty and let a sweep collect everything it
/// names. With this ordering, any crash leaves either the previous
/// complete mirror or the new complete one at the final name; a kill
/// before the rename leaves at most an anonymous temp file.
pub fn publish(root: &Path, mirror: &StoreMirror) -> io::Result<PathBuf> {
    let dir = root.join("worktrees");
    fs::create_dir_all(dir.join("tmp"))?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir.join("tmp"))?;
    tmp.write_all(mirror.serialize().as_bytes())?;
    let final_path = mirror_path(root, &mirror.worktree, &mirror.gitdir);
    crate::fsutil::durable_write_then_rename(tmp.path(), &final_path)?;
    Ok(final_path)
}

/// Remove the mirror for one (canonicalized) worktree identity, if
/// present. Missing is success: remove must stay rerunnable.
pub fn remove(root: &Path, worktree: &Path, gitdir: &Path) -> io::Result<bool> {
    match fs::remove_file(mirror_path(root, worktree, gitdir)) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// One mirror file found on disk, with its mtime and parse verdict.
#[derive(Debug)]
pub struct ReadMirror {
    /// Where the mirror file lives.
    pub path: PathBuf,
    /// Its modification time, used for grace-window decisions.
    pub modified: SystemTime,
    /// `Err` holds the reason the mirror was rejected; the file is
    /// preserved on disk either way.
    pub mirror: Result<StoreMirror, String>,
}

/// Read every `<root>/worktrees/*.tsv`. Files that fail to stat are
/// skipped; unparsable ones are reported invalid, never silently
/// treated as empty.
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
        // The framing must hold: exactly the records written, no raw
        // tab/newline inside path fields.
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
        text.push_str("file\tdeadbeef"); // torn: no newline, bad hex
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

        // Republishing the same identity replaces the file in place.
        publish(&root, &m).expect("republish");
        assert_eq!(read_all(&root).len(), 1);
    }

    #[test]
    fn interrupted_publish_leaves_no_root_but_maybe_a_temp_file() {
        // Simulate a kill between temp-write and rename: a stray file
        // in worktrees/tmp that never became a .tsv root.
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("store");
        fs::create_dir_all(root.join("worktrees").join("tmp")).expect("mkdir tmp");
        fs::write(root.join("worktrees/tmp/pid-123"), b"half written").expect("write tmp");

        assert!(read_all(&root).is_empty(), "tmp debris is not a root");

        // The next real publish works through the debris.
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
}
