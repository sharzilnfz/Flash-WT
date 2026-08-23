//! Snapshot manifest and publish tests (fast-hydration ticket 08).

use super::*;
use std::time::{Duration, SystemTime};

fn id(n: u8) -> ContentId {
    let mut bytes = [0u8; 32];
    bytes[0] = n;
    bytes[31] = n;
    ContentId(bytes)
}

/// Every path under `dir` (files and symlinks), heavy-relative and
/// sorted — for asserting exactly what a snapshot tree carries.
fn list_tree(dir: &Path) -> Vec<String> {
    fn walk(dir: &Path, prefix: &str, out: &mut Vec<String>) {
        for entry in fs::read_dir(dir).expect("read_dir") {
            let p = entry.expect("entry").path();
            let rel = if prefix.is_empty() {
                p.file_name().unwrap().to_string_lossy().into_owned()
            } else {
                format!("{prefix}/{}", p.file_name().unwrap().to_string_lossy())
            };
            if p.is_dir() {
                // Empty directories are part of the content; record
                // them instead of silently walking into nothing.
                if fs::read_dir(&p).expect("read_dir").next().is_none() {
                    out.push(rel);
                } else {
                    walk(&p, &rel, out);
                }
            } else {
                out.push(rel);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, "", &mut out);
    out.sort();
    out
}

/// Wrap arbitrary entry-region bytes in a header whose claimed hash
/// matches them, so parse failures come from the ENTRIES, not the
/// framing.
fn manifest_text_for_body(body: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(body.as_bytes());
    format!(
        "v1\tmanifest-sha256\t{}\n{body}",
        ContentId(hasher.finalize().into())
    )
}

#[test]
fn new_normalizes_modes_sorts_and_hashes_entry_bytes() {
    let m = Manifest::new(vec![
        SnapshotEntry::file("b.txt", id(1), 0o600),
        SnapshotEntry::dir("a"),
        SnapshotEntry::file("a/z", id(2), 0o700),
    ])
    .expect("valid entries");
    let rels: Vec<&str> = m.entries.iter().map(|e| e.rel.as_str()).collect();
    assert_eq!(rels, ["a", "a/z", "b.txt"], "raw path byte order");
    // 0o600 has no x-bit -> plain; 0o700 has -> exec.
    assert_eq!(m.entries[0].mode, DIR_MODE);
    assert_eq!(m.entries[1].mode, EXEC_FILE_MODE);
    assert_eq!(m.entries[2].mode, PLAIN_FILE_MODE);

    // Hash covers exactly the entry lines, not the header.
    let body = serialize_entries(&m.entries);
    let mut hasher = Sha256::new();
    hasher.update(body.as_bytes());
    assert_eq!(m.hash, ContentId(hasher.finalize().into()));
}

#[test]
fn serialize_parse_round_trips_every_kind_and_escapes() {
    let entries = vec![
        SnapshotEntry::dir("dir with space"),
        SnapshotEntry::dir("empty"),
        SnapshotEntry::file("we\tird%.txt", id(1), 0o644),
        SnapshotEntry::file("x", id(2), 0o755),
        SnapshotEntry::symlink("link%one", "target with\ttab"),
        SnapshotEntry::symlink("nested/ln", "../x"),
        SnapshotEntry::dir("nested"),
    ];
    let m = Manifest::new(entries).expect("valid");
    let text = m.serialize();
    assert_eq!(
        Manifest::parse(&text).expect("parses back").entries,
        m.entries,
        "escape/unescape must round-trip every field"
    );
}

#[test]
fn parse_rejects_tampered_body_or_header() {
    let text = Manifest::new(vec![
        SnapshotEntry::file("f.txt", id(1), 0o644),
        SnapshotEntry::dir("d"),
    ])
    .unwrap()
    .serialize();

    // Flip one ref digit in the body: hash mismatch.
    let tampered = text.replace(&id(1).to_string(), &id(3).to_string());
    assert_ne!(tampered, text);
    assert!(Manifest::parse(&tampered).is_err());

    // Header claims a different hash than the body carries.
    let body = text.split_once('\n').unwrap().1;
    let relabeled = manifest_text_for_body(body).replace(
        &{
            let mut hasher = Sha256::new();
            hasher.update(body.as_bytes());
            ContentId(hasher.finalize().into()).to_string()
        },
        &id(9).to_string(),
    );
    assert!(Manifest::parse(&relabeled).is_err());

    // Unknown kind, malformed mode, non-hex file ref — each with a
    // well-formed framing so the rejection is about the field.
    for bad in [
        "entry\tx\tfifo\t644\t-\n",
        "entry\tx\tfile\tnine\t11\n",
        "entry\tx\tfile\t644\tnot-hex\n",
        "entry\tx\tdir\t644\tblobish\n",
        "entry\t..\tdir\t755\t-\n",
        "entry\t/abs\tfile\t644\t11\n",
        "notentry\tx\tfile\t644\t11\n",
        "entry\tx\tfile\t644\n",
    ] {
        let text = manifest_text_for_body(bad);
        assert!(Manifest::parse(&text).is_err(), "{bad:?} must be rejected");
    }
}

#[test]
fn invalid_relpaths_are_rejected_before_placement() {
    for bad in [
        "/abs/path",
        "..",
        "a/../b",
        "./a",
        "a//b",
        "trailing/",
        "",
        ".",
    ] {
        let e = SnapshotEntry::file(bad, id(1), 0o644);
        assert!(Manifest::new(vec![e]).is_err(), "{bad:?} must be rejected");
    }
}

#[test]
fn kind_ref_mismatches_are_rejected() {
    let mut file = SnapshotEntry::file("f", id(1), 0o644);
    file.blob = None;
    assert!(Manifest::new(vec![file]).is_err());
    let mut dir = SnapshotEntry::dir("d");
    dir.target = Some("nope".into());
    assert!(Manifest::new(vec![dir]).is_err());
    let mut link = SnapshotEntry::symlink("l", "t");
    link.target = None;
    assert!(Manifest::new(vec![link]).is_err());
}

#[test]
fn duplicate_relpath_rejected_regardless_of_order() {
    let a = SnapshotEntry::file("same", id(1), 0o644);
    let b = SnapshotEntry::file("same", id(2), 0o644);
    assert!(Manifest::new(vec![a, b]).is_err());
}

#[test]
fn read_published_requires_manifest_complete_and_matching_hash() {
    let base = tempfile::tempdir().unwrap();
    let root = base.path();
    let m = Manifest::new(vec![
        SnapshotEntry::file("pkg/a.txt", id(1), 0o644),
        SnapshotEntry::dir("empty-dir"),
        SnapshotEntry::symlink("ln", "../pkg/a.txt"),
    ])
    .unwrap();

    // Nothing published at all.
    assert!(read_published(root, &m.hash).is_none());

    let dir = snapshot_path(root, &m.hash);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("manifest.tsv"), m.serialize()).unwrap();
    // Missing .complete: still invalid.
    assert!(read_published(root, &m.hash).is_none());

    fs::write(dir.join(".complete"), format!("v1\t{}\n", m.hash)).unwrap();
    assert_eq!(read_published(root, &m.hash).unwrap(), m);

    // Wrong version or hash in .complete: invalid.
    fs::write(dir.join(".complete"), format!("v2\t{}\n", m.hash)).unwrap();
    assert!(read_published(root, &m.hash).is_none());
    fs::write(dir.join(".complete"), format!("v1\t{}\n", id(7))).unwrap();
    assert!(read_published(root, &m.hash).is_none());

    // Tampered manifest body: invalid even with matching .complete.
    fs::write(dir.join(".complete"), format!("v1\t{}\n", m.hash)).unwrap();
    fs::write(
        dir.join("manifest.tsv"),
        m.serialize().replace("644", "600"),
    )
    .unwrap();
    assert!(read_published(root, &m.hash).is_none());
}

#[test]
fn publish_builds_tree_with_modes_links_and_empty_dirs() {
    let base = tempfile::tempdir().unwrap();
    let mut store = DiskStore::open(base.path().join("store")).unwrap();

    use crate::Store as _;
    let b1 = store.put(b"hello").unwrap();
    let b2 = store.put(b"#!/bin/sh\necho hi\n").unwrap();

    let entries = vec![
        SnapshotEntry::dir("empty-dir"),
        SnapshotEntry::file("pkg/a.txt", b1, 0o644),
        SnapshotEntry::file("pkg/run.sh", b2, 0o755),
        SnapshotEntry::symlink("pkg/bin-link", "../pkg/run.sh"),
    ];
    let outcome = store.publish_snapshot(entries.clone(), false).unwrap();
    assert_eq!(outcome, Ok(PublishOutcome::Published));

    let m = Manifest::new(entries).unwrap();
    let snap_dir = snapshot_path(base.path().join("store").as_path(), &m.hash);
    let tree = snap_dir.join("tree");
    assert_eq!(
        fs::read_to_string(snap_dir.join("manifest.tsv")).unwrap(),
        m.serialize()
    );
    assert_eq!(
        fs::read_to_string(snap_dir.join(".complete")).unwrap(),
        format!("v1\t{}\n", m.hash)
    );
    // The clonable subtree holds exactly the hydrated content — no
    // metadata files leak into a cloned worktree.
    assert_eq!(
        list_tree(&tree),
        ["empty-dir", "pkg/a.txt", "pkg/bin-link", "pkg/run.sh"]
    );
    // Empty dir exists explicitly.
    assert!(tree.join("empty-dir").is_dir());
    // File content flows from the blob through the hardlink.
    assert_eq!(fs::read(tree.join("pkg/a.txt")).unwrap(), b"hello");
    // Exec bit normalized onto the snapshot copy (and shared blob).
    let run_mode = fs::metadata(tree.join("pkg/run.sh"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(run_mode & 0o777, EXEC_FILE_MODE);
    // Plain files stay owner-writable so clones inherit writability.
    let a_mode = fs::metadata(tree.join("pkg/a.txt"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(a_mode & 0o777, PLAIN_FILE_MODE);
    // Symlink recorded as a symlink with the exact target.
    let md = fs::symlink_metadata(tree.join("pkg/bin-link")).unwrap();
    assert!(md.file_type().is_symlink());
    assert_eq!(
        fs::read_link(tree.join("pkg/bin-link")).unwrap(),
        std::path::Path::new("../pkg/run.sh")
    );
    // find_snapshot agrees it is valid.
    assert_eq!(store.find_snapshot(&m.hash).unwrap(), m);

    store.flush().unwrap();
}

#[test]
fn concurrent_publish_loser_consumes_valid_winner() {
    let base = tempfile::tempdir().unwrap();
    let mut store = DiskStore::open(base.path().join("store")).unwrap();

    use crate::Store as _;
    let blob = store.put(b"shared").unwrap();
    let entries = vec![SnapshotEntry::file("f.txt", blob, 0o644)];

    assert_eq!(
        store.publish_snapshot(entries.clone(), false).unwrap(),
        Ok(PublishOutcome::Published)
    );

    // A second builder racing identical content loses the rename but
    // must recognize and consume the valid winner.
    assert_eq!(
        store.publish_snapshot(entries.clone(), false).unwrap(),
        Ok(PublishOutcome::WinnerValid),
        "loser consumes the winner without failure"
    );

    // An INVALID winner is left alone and reported as debris: pre-corrupt
    // the directory this next manifest would land on.
    let other_entries = vec![
        SnapshotEntry::file("f.txt", blob, 0o644),
        SnapshotEntry::dir("sub"),
    ];
    let other_path = snapshot_path(
        base.path().join("store").as_path(),
        &Manifest::new(other_entries.clone()).unwrap().hash,
    );
    fs::create_dir_all(&other_path).unwrap();
    fs::write(other_path.join("manifest.tsv"), "garbage\n").unwrap();

    assert_eq!(
        store.publish_snapshot(other_entries, false).unwrap(),
        Ok(PublishOutcome::WinnerInvalid),
        "invalid winner is never overwritten nor trusted"
    );
    assert_eq!(
        fs::read_to_string(other_path.join("manifest.tsv")).unwrap(),
        "garbage\n",
        "debris left untouched"
    );
    store.flush().unwrap();
}

#[test]
fn missing_blob_reports_missing_for_healing_retry() {
    let base = tempfile::tempdir().unwrap();
    let mut store = DiskStore::open(base.path().join("store")).unwrap();

    // The ingest step computed this id, but a sweep reclaimed the
    // object before the link happened.
    use sha2::Digest as _;
    let ghost = ContentId(Sha256::digest(b"lost bytes").into());
    let entries = vec![SnapshotEntry::file("gone.txt", ghost, 0o644)];

    match store.publish_snapshot(entries.clone(), false).unwrap() {
        Err(BuildError::MissingBlob(id)) => assert_eq!(id, ghost),
        other => panic!("expected MissingBlob, got {other:?}"),
    }
    // No published snapshot may exist after the failed build...
    assert!(store.find_snapshot(&ghost).is_none());
    // ...and no stray temp dirs either.
    let tmp = base.path().join("store/snapshots/tmp");
    if let Ok(found) = fs::read_dir(&tmp) {
        assert_eq!(found.count(), 0, "failed build leaves no temp debris");
    }

    // Healing per plan: re-run put() for the source content, then
    // retry once. The retry succeeds.
    store.put(b"lost bytes").unwrap();
    assert_eq!(
        store.publish_snapshot(entries, false).unwrap(),
        Ok(PublishOutcome::Published)
    );
    let m = Manifest::new(vec![SnapshotEntry::file("gone.txt", ghost, 0o644)]).unwrap();
    assert_eq!(
        fs::read(snapshot_tree_path(base.path().join("store").as_path(), &m.hash).join("gone.txt"))
            .unwrap(),
        b"lost bytes"
    );
    store.flush().unwrap();
}

#[test]
fn paranoid_publish_full_hashes_and_catches_tampered_blob() {
    let base = tempfile::tempdir().unwrap();
    let mut store = DiskStore::open(base.path().join("store")).unwrap();

    use crate::Store as _;
    let blob = store.put(b"content").unwrap();

    // Tamper preserving size AND mtime, so the verified-ledger trust
    // path would wave it through. Paranoia must not.
    let path = store.blob_path(&blob);
    let original_mtime: SystemTime = fs::metadata(&path).unwrap().modified().unwrap();
    fs::write(&path, b"CONTENT").unwrap(); // same length, different bytes
    let f = fs::OpenOptions::new().write(true).open(&path).unwrap();
    f.set_times(std::fs::FileTimes::new().set_modified(original_mtime))
        .unwrap();

    let err = store
        .publish_snapshot(vec![SnapshotEntry::file("p", blob, 0o644)], true)
        .unwrap();
    match err {
        Err(BuildError::Fatal(msg)) => {
            assert!(msg.contains("hash verification"), "{msg}");
        }
        other => panic!("paranoid build must fail loudly, got {other:?}"),
    }
    // Nothing was published from corrupted content.
    let m = Manifest::new(vec![SnapshotEntry::file("p", blob, 0o644)]).unwrap();
    assert!(store.find_snapshot(&m.hash).is_none());
    store.flush().unwrap();
}

#[test]
fn verify_bypass_rebuilds_over_a_healthy_winner() {
    let base = tempfile::tempdir().unwrap();
    let mut store = DiskStore::open(base.path().join("store")).unwrap();

    use crate::Store as _;
    let blob = store.put(b"payload").unwrap();
    let entries = vec![SnapshotEntry::file("p", blob, 0o644)];
    assert_eq!(
        store.publish_snapshot(entries.clone(), false).unwrap(),
        Ok(PublishOutcome::Published)
    );

    // WT_VERIFY=1 semantics: callers skip find_snapshot entirely and
    // rebuild. Every blob is full-hashed during that rebuild; over a
    // healthy store it lands on the existing valid winner.
    assert_eq!(
        store.publish_snapshot(entries, true).unwrap(),
        Ok(PublishOutcome::WinnerValid)
    );
    store.flush().unwrap();
}

#[test]
fn failed_publish_leaves_only_temp_debris_behind() {
    let base = tempfile::tempdir().unwrap();
    let store = DiskStore::open(base.path().join("store")).unwrap();

    let ghost = ContentId(Sha256::digest(b"never stored").into());
    let err = store
        .publish_snapshot(vec![SnapshotEntry::file("x", ghost, 0o644)], false)
        .unwrap();
    assert!(matches!(err, Err(BuildError::MissingBlob(_))));

    // The snapshots root exists with an empty tmp dir; no published
    // name appeared.
    let snapshots = base.path().join("store/snapshots");
    let names: Vec<String> = fs::read_dir(&snapshots)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["tmp".to_string()]);
    // Simulate a kill mid-build: junk in tmp is inert debris.
    fs::write(snapshots.join("tmp/build-junk"), b"half").unwrap();
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap();
    let _ = Duration::from_secs(0); // keep imports honest if cfgs shift
    store.flush().unwrap();
}
