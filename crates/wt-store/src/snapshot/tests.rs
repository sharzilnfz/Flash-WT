//! Snapshot manifest and publish tests (fast-hydration ticket 08).

use super::manifest::{DIR_MODE, EXEC_FILE_MODE, PLAIN_FILE_MODE, serialize_entries};
use super::*;
use sha2::{Digest as _, Sha256};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{Duration, SystemTime};

// Traits/types the old single-file module globbed in from its own
// use-statements.
use crate::{DiskStore, Store as _};

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
    assert_eq!(outcome, PublishOutcome::Published);

    let m = Manifest::new_with_lockfile_and_size(entries, None, 23).unwrap();
    let snap_dir = snapshot_path(base.path().join("store").as_path(), &m.hash);
    let loaded =
        Manifest::parse(&fs::read_to_string(snap_dir.join("manifest.tsv")).unwrap()).unwrap();
    assert_eq!(loaded.hash, m.hash);
    assert_eq!(loaded.entries, m.entries);
    let tree = snap_dir.join("tree");
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
    assert_eq!(store.find_snapshot(&m.hash).unwrap(), loaded);

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
        PublishOutcome::Published
    );

    // A second builder racing identical content loses the rename but
    // must recognize and consume the valid winner.
    assert_eq!(
        store.publish_snapshot(entries.clone(), false).unwrap(),
        PublishOutcome::WinnerValid,
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
        PublishOutcome::WinnerInvalid,
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

    let result = store.publish_snapshot(entries.clone(), false);
    match result {
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
        PublishOutcome::Published
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

    let err = store.publish_snapshot(vec![SnapshotEntry::file("p", blob, 0o644)], true);
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
        PublishOutcome::Published
    );

    // WT_VERIFY=1 semantics: callers skip find_snapshot entirely and
    // rebuild. Every blob is full-hashed during that rebuild; over a
    // healthy store it lands on the existing valid winner.
    assert_eq!(
        store.publish_snapshot(entries, true).unwrap(),
        PublishOutcome::WinnerValid
    );
    store.flush().unwrap();
}

#[test]
fn failed_publish_leaves_only_temp_debris_behind() {
    let base = tempfile::tempdir().unwrap();
    let store = DiskStore::open(base.path().join("store")).unwrap();

    let ghost = ContentId(Sha256::digest(b"never stored").into());
    let err = store.publish_snapshot(vec![SnapshotEntry::file("x", ghost, 0o644)], false);
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

// ---- v2 incremental rebuilds ------------------------------------------

fn incremental_fixture() -> (tempfile::TempDir, DiskStore) {
    let base = tempfile::tempdir().unwrap();
    let store = DiskStore::open(base.path().join("store")).unwrap();
    (base, store)
}

/// A small tree with everything the unit rule can bite on: one
/// package dir with a nested subdir, an empty dir, an exec file, a
/// symlink, and a root-level plain file. Blobs are REALLY stored so
/// placement can verify them.
fn v2_entries(store: &mut DiskStore) -> Vec<SnapshotEntry> {
    use crate::Store as _;
    let a = store.put(b"nested a\n").unwrap();
    let run = store.put(b"#!/bin/sh\necho hi\n").unwrap();
    let root = store.put(b"root.txt v1\n").unwrap();
    vec![
        SnapshotEntry::dir("pkg00"),
        SnapshotEntry::dir("pkg00/nested"),
        SnapshotEntry::dir("empty-dir"),
        SnapshotEntry::symlink("pkg00/bin-link", "../run.sh"),
        SnapshotEntry::file("pkg00/nested/a", a, 0o644),
        SnapshotEntry::file("pkg00/run.sh", run, 0o755),
        SnapshotEntry::file("root.txt", root, 0o644),
    ]
}
/// Fingerprint a hydrated tree for exactness comparisons: every path
/// (files, symlinks, AND dirs — empty dirs matter) as
/// `(rel, kind, mode & 0o7777, bytes or target)`, sorted.
#[cfg(target_os = "macos")]
fn tree_fingerprint(dir: &Path) -> Vec<(String, char, u32, Vec<u8>)> {
    use std::os::unix::fs::PermissionsExt;

    fn walk(dir: &Path, prefix: &str, out: &mut Vec<(String, char, u32, Vec<u8>)>) {
        for entry in fs::read_dir(dir).expect("read_dir") {
            let entry = entry.expect("dir entry");
            let file_type = entry.file_type().expect("file_type");
            let rel = format!("{prefix}{}", entry.file_name().to_string_lossy());
            if file_type.is_symlink() {
                let target = fs::read_link(entry.path()).expect("read_link");
                out.push((
                    rel,
                    'l',
                    0o777,
                    target.as_os_str().as_encoded_bytes().to_vec(),
                ));
            } else if file_type.is_dir() {
                let mode = entry.metadata().expect("stat").permissions().mode() & 0o7777;
                out.push((rel.clone(), 'd', mode, Vec::new()));
                walk(&entry.path(), &format!("{rel}/"), out);
            } else {
                let mode = entry.metadata().expect("stat").permissions().mode() & 0o7777;
                let bytes = fs::read(entry.path()).expect("read");
                out.push((rel, 'f', mode, bytes));
            }
        }
    }

    let mut out = Vec::new();
    walk(dir, "", &mut out);
    out.sort();
    out
}

#[test]
fn incremental_publish_matches_full_build_semantics() {
    let (_base, mut store) = incremental_fixture();

    let old_entries = v2_entries(&mut store);
    let old_manifest = Manifest::new(old_entries.clone()).unwrap();
    assert_eq!(
        store.publish_snapshot(old_entries.clone(), false).unwrap(),
        PublishOutcome::Published
    );

    // Bump ONE root-level file; everything else is unchanged content
    // the whole-tree clone carries over.
    use crate::Store as _;
    let extra = store.put(b"root.txt v2\n").unwrap();
    let mut new_entries = old_entries.clone();
    new_entries.retain(|e| e.rel != "root.txt");
    new_entries.push(SnapshotEntry::file("root.txt", extra, 0o644));
    let new_manifest = Manifest::new(new_entries.clone()).unwrap();
    assert_ne!(new_manifest.hash, old_manifest.hash);

    let result =
        store.publish_snapshot_incremental_with_timing(new_entries, &old_manifest.hash, false);

    #[cfg(target_os = "macos")]
    {
        let receipt = result.unwrap();
        assert_eq!(receipt.outcome, PublishOutcome::Published);
        let timing = receipt.timing;
        // The whole point: the ENTIRE old tree is one clone unit now.
        // Sub-millisecond clones truncate to 0 ms; only counts lie.
        assert_eq!(timing.clone_units, 1);
        assert_eq!(
            timing.linked_files, 1,
            "only the bumped root file is freshly linked"
        );

        // The published result validates and its tree carries EXACTLY
        // the new content: bumped file, untouched package, symlink,
        // empty dir.
        let found = store.find_snapshot(&new_manifest.hash).unwrap();
        assert_eq!(found.entries, new_manifest.entries);
        assert_eq!(found.hash, new_manifest.hash);
        assert_eq!(found.lockfile_hash, new_manifest.lockfile_hash);
        let tree = snapshot_tree_path(store.root(), &new_manifest.hash);
        assert_eq!(
            fs::read_to_string(tree.join("root.txt")).unwrap(),
            "root.txt v2\n"
        );
        assert_eq!(
            fs::read_to_string(tree.join("pkg00/nested/a")).unwrap(),
            "nested a\n"
        );
        assert!(tree.join("empty-dir").is_dir());
        let md = fs::symlink_metadata(tree.join("pkg00/bin-link")).unwrap();
        assert!(md.file_type().is_symlink());
        assert_eq!(
            fs::read_link(tree.join("pkg00/bin-link")).unwrap(),
            Path::new("../run.sh")
        );
        let run_mode = fs::metadata(tree.join("pkg00/run.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(run_mode & 0o777, EXEC_FILE_MODE);
    }
    #[cfg(not(target_os = "macos"))]
    {
        // No recursive clone primitive: the attempt aborts so the
        // caller falls back to a full build. Nothing is published.
        assert!(matches!(result, Err(BuildError::Fatal(_))));
        assert!(store.find_snapshot(&new_manifest.hash).is_none());
    }
    store.flush().unwrap();
}

#[test]
fn deleted_subtree_delta_lands_exactly() {
    let (_base, mut store) = incremental_fixture();

    let old_entries = v2_entries(&mut store);
    let old_manifest = Manifest::new(old_entries.clone()).unwrap();
    assert_eq!(
        store.publish_snapshot(old_entries.clone(), false).unwrap(),
        PublishOutcome::Published
    );

    // New manifest drops pkg00 entirely and keeps only the empty dir
    // plus bumped root file. Deletions are pure unlinks inside the
    // cloned copy; the bumped file is the only relink.
    use crate::Store as _;
    let extra = store.put(b"root.txt v2\n").unwrap();
    let gone_entries = vec![
        SnapshotEntry::dir("empty-dir"),
        SnapshotEntry::file("root.txt", extra, 0o644),
    ];
    let gone_manifest = Manifest::new(gone_entries).unwrap();

    #[cfg(target_os = "macos")]
    {
        assert_eq!(
            store
                .publish_snapshot_incremental(
                    gone_manifest.entries.clone(),
                    &old_manifest.hash,
                    false
                )
                .unwrap(),
            PublishOutcome::Published
        );
        let tree = snapshot_tree_path(store.root(), &gone_manifest.hash);
        assert!(!tree.join("pkg00").exists(), "deleted subtree must vanish");
        assert!(tree.join("empty-dir").is_dir());
        assert_eq!(
            fs::read_to_string(tree.join("root.txt")).unwrap(),
            "root.txt v2\n"
        );
    }
    #[cfg(not(target_os = "macos"))]
    {
        assert!(matches!(
            store
                .publish_snapshot_incremental(
                    gone_manifest.entries.clone(),
                    &old_manifest.hash,
                    false
                )
                .unwrap_err(),
            BuildError::Fatal(_)
        ));
    }
    store.flush().unwrap();
}

#[test]
fn unusable_old_snapshot_aborts_the_incremental_attempt() {
    let (_base, mut store) = incremental_fixture();

    let entries = v2_entries(&mut store);
    let manifest_hash = Manifest::new(entries.clone()).unwrap().hash;

    // A ghost old hash: nothing to clone from. There is NO per-unit
    // fallback anymore — the whole attempt aborts and the CALLER is
    // responsible for the full build.
    let ghost = ContentId([0xAA; 32]);
    let result = store.publish_snapshot_incremental_with_timing(entries, &ghost, false);
    assert!(
        matches!(result, Err(BuildError::Fatal(_))),
        "a refused whole-tree clone must abort the attempt"
    );
    assert!(
        store.find_snapshot(&manifest_hash).is_none(),
        "an aborted attempt publishes nothing"
    );

    // No stray temp dirs either.
    let tmp = store.root().join("snapshots/tmp");
    if let Ok(found) = fs::read_dir(&tmp) {
        assert_eq!(found.count(), 0, "aborted build leaves no temp debris");
    }
    store.flush().unwrap();
}

#[test]
fn old_snapshot_evicted_midflight_yields_to_a_full_build() {
    let (_base, mut store) = incremental_fixture();
    use crate::Store as _;

    let old_entries = v2_entries(&mut store);
    let old_manifest = Manifest::new(old_entries.clone()).unwrap();
    assert_eq!(
        store.publish_snapshot(old_entries.clone(), false).unwrap(),
        PublishOutcome::Published
    );

    // Eviction race: selection already picked this old snapshot, then
    // sweep took its whole directory before the ONE whole-tree clone.
    // That single clone ENOENTs, the attempt aborts with an error, and
    // the CLI's full-build fallback lands the tree instead (the old
    // per-unit-fallback behavior is gone).
    fs::remove_dir_all(snapshot_path(store.root(), &old_manifest.hash)).unwrap();

    let extra = store.put(b"root.txt v2\n").unwrap();
    let mut new_entries = old_entries.clone();
    new_entries.retain(|e| e.rel != "root.txt");
    new_entries.push(SnapshotEntry::file("root.txt", extra, 0o644));
    let new_manifest = Manifest::new(new_entries.clone()).unwrap();

    let result =
        store.publish_snapshot_incremental_with_timing(new_entries, &old_manifest.hash, false);
    assert!(
        matches!(result, Err(BuildError::Fatal(_))),
        "the evicted old tree must abort the incremental attempt"
    );
    assert!(
        store.find_snapshot(&new_manifest.hash).is_none(),
        "nothing published from a vanished source; the full build owns this address now"
    );
    store.flush().unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn paranoid_incremental_rejects_rotted_blob_inside_the_cloned_tree() {
    let (_base, mut store) = incremental_fixture();
    use crate::Store as _;

    let old_entries = v2_entries(&mut store);
    let old_manifest = Manifest::new(old_entries.clone()).unwrap();
    let inner_blob = {
        let entry = old_entries
            .iter()
            .find(|e| e.rel == "pkg00/nested/a")
            .unwrap()
            .clone();
        entry.blob.unwrap()
    };
    assert_eq!(
        store.publish_snapshot(old_entries.clone(), false).unwrap(),
        PublishOutcome::Published
    );

    // Tamper with a blob INSIDE the cloned region, preserving size AND
    // mtime so ledger trust cannot see it. The paranoid pass over the
    // WHOLE staged tree (cloned bulk included) must catch it before
    // the rename.
    let target = store.blob_path(&inner_blob);
    let mtime = fs::metadata(&target).unwrap().modified().unwrap();
    let original = fs::read(&target).unwrap();
    let mut rotted = original.clone();
    rotted[0] = rotted[0].wrapping_add(1);
    fs::write(&target, &rotted).unwrap();
    let f = fs::OpenOptions::new().write(true).open(&target).unwrap();
    f.set_times(std::fs::FileTimes::new().set_modified(mtime))
        .unwrap();

    // Bump the root file too, so the rebuild targets a NEW address:
    // proving failure leaves that address empty is meaningful (the
    // old snapshot still exists at its own).
    let extra = store.put(b"root.txt v2\n").unwrap();
    let mut new_entries = old_entries.clone();
    new_entries.retain(|e| e.rel != "root.txt");
    new_entries.push(SnapshotEntry::file("root.txt", extra, 0o644));
    let new_hash = Manifest::new(new_entries.clone()).unwrap().hash;

    let err = store
        .publish_snapshot_incremental(new_entries, &old_manifest.hash, true)
        .unwrap_err();
    match err {
        BuildError::Fatal(msg) => {
            assert!(msg.contains("paranoid check"), "{msg}");
        }
        other => panic!("paranoid incremental must fail loudly, got {other:?}"),
    }
    // Nothing was published from rotted content.
    assert!(store.find_snapshot(&new_hash).is_none());

    // Restore the bytes: the same call now succeeds and publishes.
    fs::write(&target, &original).unwrap();
    let f = fs::OpenOptions::new().write(true).open(&target).unwrap();
    f.set_times(std::fs::FileTimes::new().set_modified(mtime))
        .unwrap();
    let mut restored = v2_entries(&mut store);
    restored.retain(|e| e.rel != "root.txt");
    restored.push(SnapshotEntry::file("root.txt", extra, 0o644));
    assert_eq!(
        store
            .publish_snapshot_incremental(restored, &old_manifest.hash, true)
            .unwrap(),
        PublishOutcome::Published
    );
    store.flush().unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn delta_matrix_matches_a_full_build_of_the_same_target_exactly() {
    let base = tempfile::tempdir().unwrap();
    // Two independent stores; identical bytes give identical blob ids,
    // so both can manifest the same entries.
    let mut inc_store = DiskStore::open(base.path().join("inc")).unwrap();
    let mut full_store = DiskStore::open(base.path().join("full")).unwrap();

    use crate::Store as _;
    // Generation 1 material, stored in BOTH stores where generation 2
    // still needs it (unchanged pkg/b).
    let f1 = inc_store.put(b"file one\n").unwrap();
    let f2 = inc_store.put(b"file two\n").unwrap();
    let f3 = inc_store.put(b"file three\n").unwrap();
    let deep = inc_store.put(b"deep content\n").unwrap();
    let rootv1 = inc_store.put(b"root v1\n").unwrap();

    let gen1 = vec![
        SnapshotEntry::dir("pkg"),
        SnapshotEntry::dir("pkg/sub"),
        SnapshotEntry::dir("stale-empty"),
        SnapshotEntry::symlink("pkg/ln", "../root.txt"),
        SnapshotEntry::file("pkg/a", f1, 0o644),
        SnapshotEntry::file("pkg/b", f2, 0o644),
        SnapshotEntry::file("pkg/c", f3, 0o755),
        SnapshotEntry::dir("doomed"),
        SnapshotEntry::file("doomed/x", deep, 0o644),
        SnapshotEntry::file("doomed/deeper/y", deep, 0o755),
        SnapshotEntry::file("root.txt", rootv1, 0o644),
        SnapshotEntry::symlink("doomed-link", "pkg/a"),
    ];
    assert_eq!(
        inc_store.publish_snapshot(gen1.clone(), false).unwrap(),
        PublishOutcome::Published
    );

    // Generation 2 exercises EVERY mutation kind in ONE publish:
    // file delete (pkg/a), symlink delete (doomed-link), dir delete
    // with children (doomed), empty-dir delete (stale-empty), file add
    // (added-dir/new), dir add (added-dir), empty-dir add
    // (added-empty), content modify (pkg/c, root.txt), symlink
    // retarget (pkg/ln), mode-only flip on a same-ref file (pkg/b).
    // pkg/sub survives untouched through the bulk clone.
    let fc = inc_store.put(b"file three EDITED\n").unwrap();
    let fnew = inc_store.put(b"brand new\n").unwrap();
    let rootv2 = inc_store.put(b"root v2\n").unwrap();
    let gen2 = vec![
        SnapshotEntry::dir("added-dir"),
        SnapshotEntry::file("added-dir/new", fnew, 0o644),
        SnapshotEntry::dir("added-empty"),
        SnapshotEntry::dir("pkg"),
        SnapshotEntry::dir("pkg/sub"),
        SnapshotEntry::symlink("pkg/ln", "b"),
        SnapshotEntry::file("pkg/b", f2, 0o755),
        SnapshotEntry::file("pkg/c", fc, 0o755),
        SnapshotEntry::file("root.txt", rootv2, 0o644),
    ];

    let gen1_manifest = Manifest::new(gen1).unwrap();
    let gen2_manifest = Manifest::new(gen2.clone()).unwrap();

    // Sanity: the diff really classifies all of these as changed.
    let diff =
        crate::snapdiff::SnapshotDiff::compute(&gen1_manifest.entries, &gen2_manifest.entries);
    assert!(!diff.added.is_empty());
    assert!(!diff.modified.is_empty());
    assert!(!diff.deleted.is_empty());

    let receipt = inc_store
        .publish_snapshot_incremental_with_timing(gen2.clone(), &gen1_manifest.hash, false)
        .unwrap();
    assert_eq!(receipt.outcome, PublishOutcome::Published);
    assert_eq!(receipt.timing.clone_units, 1);

    // Reference: a FULL build of the same target entries in the other
    // store (same blobs there).
    assert_eq!(
        full_store.put(b"file two\n").unwrap(),
        f2,
        "deterministic blob ids make the two stores comparable"
    );
    for bytes in [
        b"file three EDITED\n".as_slice(),
        b"brand new\n".as_slice(),
        b"root v2\n".as_slice(),
    ] {
        full_store.put(bytes).unwrap();
    }
    assert_eq!(
        full_store.publish_snapshot(gen2, false).unwrap(),
        PublishOutcome::Published
    );

    let inc_tree = snapshot_tree_path(inc_store.root(), &gen2_manifest.hash);
    let full_tree = snapshot_tree_path(full_store.root(), &gen2_manifest.hash);
    assert_eq!(
        tree_fingerprint(&inc_tree),
        tree_fingerprint(&full_tree),
        "incremental delta result must be byte/mode/symlink-exact vs the full build"
    );
    inc_store.flush().unwrap();
    full_store.flush().unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn scattered_changes_clone_once_and_relink_only_what_changed() {
    let (_base, mut store) = incremental_fixture();
    use crate::Store as _;

    // A wide tree: 20 packages x 10 files.
    let mut entries = Vec::new();
    for p in 0..20u8 {
        entries.push(SnapshotEntry::dir(format!("pkg{p:02}")));
        for i in 0..10usize {
            let blob = store
                .put(format!("package {p} file {i}\n").as_bytes())
                .unwrap();
            entries.push(SnapshotEntry::file(
                format!("pkg{p:02}/f{i:02}"),
                blob,
                0o644,
            ));
        }
    }
    let old_manifest = Manifest::new(entries.clone()).unwrap();
    assert_eq!(
        store.publish_snapshot(entries, false).unwrap(),
        PublishOutcome::Published
    );

    // Scattered bump: exactly three modified files, three different
    // packages, no adds or deletes.
    let mut new_entries = old_manifest.entries.clone();
    for (p, i) in [(0u8, 0usize), (7, 3), (19, 9)] {
        let rel = format!("pkg{p:02}/f{i:02}");
        new_entries.retain(|e| e.rel != rel);
        let blob = store
            .put(format!("package {p} file {i} EDITED\n").as_bytes())
            .unwrap();
        new_entries.push(SnapshotEntry::file(rel, blob, 0o644));
    }
    let new_manifest = Manifest::new(new_entries).unwrap();

    let receipt = store
        .publish_snapshot_incremental_with_timing(
            new_manifest.entries.clone(),
            &old_manifest.hash,
            false,
        )
        .unwrap();
    assert_eq!(receipt.outcome, PublishOutcome::Published);
    assert_eq!(
        receipt.timing.clone_units, 1,
        "one whole-tree clone regardless of scatter"
    );
    assert_eq!(
        receipt.timing.linked_files, 3,
        "exactly the changed files may be freshly linked"
    );
    let found = store.find_snapshot(&new_manifest.hash).unwrap();
    assert_eq!(found.entries, new_manifest.entries);
    assert_eq!(found.hash, new_manifest.hash);
    store.flush().unwrap();
}

#[test]
fn subtree_partitioned_parallel_snapshot_construction_integrity() {
    let base = tempfile::tempdir().unwrap();
    let mut store = DiskStore::open(base.path().join("store")).unwrap();

    let mut entries = Vec::new();
    let mut ref_contents = std::collections::HashMap::new();

    // Create 30 distinct directories with 10 files each, plus symlinks and empty directories
    for dir_idx in 0..30 {
        let dir_name = format!("pkg_{dir_idx:03}");
        entries.push(SnapshotEntry::dir(&dir_name));

        // Subdirectory
        let sub_dir = format!("{dir_name}/nested");
        entries.push(SnapshotEntry::dir(&sub_dir));

        // Empty directory
        let empty_dir = format!("{dir_name}/empty");
        entries.push(SnapshotEntry::dir(&empty_dir));

        for file_idx in 0..10 {
            let data = format!("content of dir {dir_idx} file {file_idx}\n");
            let blob = store.put(data.as_bytes()).unwrap();
            let rel = format!("{dir_name}/file_{file_idx:02}.txt");
            let mode = if file_idx % 2 == 0 { 0o755 } else { 0o644 };
            entries.push(SnapshotEntry::file(&rel, blob, mode));
            ref_contents.insert(rel, (data.into_bytes(), mode));
        }

        // Nested file
        let nested_data = format!("nested content {dir_idx}\n");
        let nested_blob = store.put(nested_data.as_bytes()).unwrap();
        let nested_rel = format!("{sub_dir}/item.txt");
        entries.push(SnapshotEntry::file(&nested_rel, nested_blob, 0o644));
        ref_contents.insert(nested_rel, (nested_data.into_bytes(), 0o644));

        // Symlink
        let symlink_rel = format!("{dir_name}/link_to_file0");
        entries.push(SnapshotEntry::symlink(&symlink_rel, "file_00.txt"));
    }

    let manifest = Manifest::new(entries.clone()).unwrap();
    let receipt = store.publish_snapshot_with_timing(entries, false).unwrap();
    assert_eq!(receipt.outcome, PublishOutcome::Published);

    // Verify tree integrity
    let tree_path = snapshot_tree_path(store.root(), &manifest.hash);
    for (rel, (expected_bytes, expected_mode)) in ref_contents {
        let path = tree_path.join(&rel);
        assert!(path.is_file(), "missing file: {rel}");
        let actual_bytes = fs::read(&path).unwrap();
        assert_eq!(actual_bytes, expected_bytes, "content mismatch for {rel}");

        let md = fs::metadata(&path).unwrap();
        let norm_mode = if expected_mode & 0o111 != 0 {
            EXEC_FILE_MODE
        } else {
            PLAIN_FILE_MODE
        };
        assert_eq!(
            md.permissions().mode() & 0o777,
            norm_mode,
            "mode mismatch for {rel}"
        );
    }

    for dir_idx in 0..30 {
        let dir_name = format!("pkg_{dir_idx:03}");
        assert!(tree_path.join(&dir_name).is_dir());
        assert!(tree_path.join(format!("{dir_name}/nested")).is_dir());
        assert!(tree_path.join(format!("{dir_name}/empty")).is_dir());

        let sym_path = tree_path.join(format!("{dir_name}/link_to_file0"));
        let md = fs::symlink_metadata(&sym_path).unwrap();
        assert!(md.file_type().is_symlink());
        assert_eq!(fs::read_link(&sym_path).unwrap(), Path::new("file_00.txt"));
    }

    store.flush().unwrap();
}

#[test]
fn parallel_snapshot_construction_missing_blob_fails_safely() {
    let base = tempfile::tempdir().unwrap();
    let mut store = DiskStore::open(base.path().join("store")).unwrap();

    let b1 = store.put(b"blob 1").unwrap();
    let ghost = ContentId(Sha256::digest(b"ghost missing").into());

    let entries = vec![
        SnapshotEntry::dir("sub_a"),
        SnapshotEntry::file("sub_a/a.txt", b1, 0o644),
        SnapshotEntry::dir("sub_b"),
        SnapshotEntry::file("sub_b/b.txt", ghost, 0o644),
    ];

    let err = store.publish_snapshot(entries.clone(), false).unwrap_err();
    match err {
        BuildError::MissingBlob(id) => assert_eq!(id, ghost),
        other => panic!("expected MissingBlob, got {other:?}"),
    }

    // Now heal ghost and publish successfully
    store.put(b"ghost missing").unwrap();
    let receipt = store.publish_snapshot(entries, false).unwrap();
    assert_eq!(receipt, PublishOutcome::Published);
    store.flush().unwrap();
}

#[test]
fn parallel_snapshot_construction_paranoid_fails_on_corruption() {
    let base = tempfile::tempdir().unwrap();
    let mut store = DiskStore::open(base.path().join("store")).unwrap();

    let b1 = store.put(b"valid content").unwrap();
    let b2 = store.put(b"corrupted content").unwrap();

    // Tamper with b2 on disk preserving mtime/size
    let path = store.blob_path(&b2);
    let orig_mtime = fs::metadata(&path).unwrap().modified().unwrap();
    fs::write(&path, b"CORRUPTED CONTENT").unwrap();
    let f = fs::OpenOptions::new().write(true).open(&path).unwrap();
    f.set_times(std::fs::FileTimes::new().set_modified(orig_mtime))
        .unwrap();

    let entries = vec![
        SnapshotEntry::dir("dir1"),
        SnapshotEntry::file("dir1/f1.txt", b1, 0o644),
        SnapshotEntry::dir("dir2"),
        SnapshotEntry::file("dir2/f2.txt", b2, 0o644),
    ];

    let err = store.publish_snapshot(entries, true).unwrap_err();
    match err {
        BuildError::Fatal(msg) => assert!(msg.contains("hash verification"), "{msg}"),
        other => panic!("expected Fatal error on corrupt blob, got {other:?}"),
    }
    store.flush().unwrap();
}

#[test]
fn large_scale_subtree_partitioned_parallel_ingestion_integrity() {
    let base = tempfile::tempdir().unwrap();
    let mut store = DiskStore::open(base.path().join("store")).unwrap();

    let mut entries = Vec::new();
    let shared_blob = store.put(b"common file payload for scale test\n").unwrap();
    let exec_blob = store.put(b"#!/bin/bash\necho executable\n").unwrap();

    // 40 directories, 50 files each = 2,000 files
    for dir_idx in 0..40 {
        let dir_rel = format!("module_{dir_idx:02}");
        entries.push(SnapshotEntry::dir(&dir_rel));

        for file_idx in 0..50 {
            let file_rel = format!("{dir_rel}/file_{file_idx:02}.txt");
            let mode = if file_idx == 0 { 0o755 } else { 0o644 };
            let blob = if file_idx == 0 {
                exec_blob
            } else {
                shared_blob
            };
            entries.push(SnapshotEntry::file(&file_rel, blob, mode));
        }
    }

    let manifest = Manifest::new(entries.clone()).unwrap();
    let receipt = store.publish_snapshot_with_timing(entries, false).unwrap();
    assert_eq!(receipt.outcome, PublishOutcome::Published);

    let tree_path = snapshot_tree_path(store.root(), &manifest.hash);
    for dir_idx in 0..40 {
        let dir_rel = format!("module_{dir_idx:02}");
        assert!(tree_path.join(&dir_rel).is_dir());

        for file_idx in 0..50 {
            let file_rel = format!("{dir_rel}/file_{file_idx:02}.txt");
            let p = tree_path.join(&file_rel);
            assert!(p.is_file());
            let md = fs::metadata(&p).unwrap();
            let expected_mode = if file_idx == 0 {
                EXEC_FILE_MODE
            } else {
                PLAIN_FILE_MODE
            };
            assert_eq!(md.permissions().mode() & 0o777, expected_mode);
        }
    }
    store.flush().unwrap();
}

#[test]
fn manifest_header_lockfile_hash_round_trips_and_validates() {
    let base = tempfile::tempdir().unwrap();
    let mut store = DiskStore::open(base.path().join("store")).unwrap();

    let blob = store.put(b"hello lockfile\n").unwrap();
    let entries = vec![SnapshotEntry::file("dep.txt", blob, 0o644)];
    let lock_hash = id(42);

    let manifest = Manifest::new_with_lockfile(entries.clone(), Some(lock_hash)).unwrap();
    assert_eq!(manifest.lockfile_hash, Some(lock_hash));

    let serialized = manifest.serialize();
    assert!(serialized.starts_with(&format!(
        "v1\tmanifest-sha256\t{}\tlockfile-sha256\t{}\n",
        manifest.hash, lock_hash
    )));

    let parsed = Manifest::parse(&serialized).unwrap();
    assert_eq!(parsed.hash, manifest.hash);
    assert_eq!(parsed.lockfile_hash, Some(lock_hash));
    assert_eq!(parsed.entries, manifest.entries);

    // Publishing with lockfile records the header
    let receipt = store
        .publish_snapshot_with_lockfile_and_timing(entries, Some(lock_hash), false)
        .unwrap();
    assert_eq!(receipt.outcome, PublishOutcome::Published);

    let loaded = store.find_snapshot(&manifest.hash).unwrap();
    assert_eq!(loaded.lockfile_hash, Some(lock_hash));
}

#[test]
fn manifest_header_total_bytes_round_trips_and_records_publish_size() {
    let base = tempfile::tempdir().unwrap();
    let mut store = DiskStore::open(base.path().join("store")).unwrap();

    let b1 = store.put(b"content 1 (13b)").unwrap();
    let b2 = store.put(b"content 2 is longer (21 bytes)").unwrap();
    let entries = vec![
        SnapshotEntry::file("f1.txt", b1, 0o644),
        SnapshotEntry::file("f2.txt", b2, 0o644),
        SnapshotEntry::file("f1_dup.txt", b1, 0o644), // duplicate blob must not double-count
        SnapshotEntry::dir("sub"),
    ];

    let expected_unique_size = 15 + 30; // length of b1 (15 bytes) and b2 (30 bytes)
    let manifest =
        Manifest::new_with_lockfile_and_size(entries.clone(), None, expected_unique_size).unwrap();
    assert_eq!(manifest.total_size, expected_unique_size);

    let serialized = manifest.serialize();
    assert!(serialized.contains(&format!("\ttotal-bytes\t{expected_unique_size}")));

    let parsed = Manifest::parse(&serialized).unwrap();
    assert_eq!(parsed.hash, manifest.hash);
    assert_eq!(parsed.total_size, expected_unique_size);
    assert_eq!(parsed.entries, manifest.entries);

    // Publishing computes unique uncompressed blob size totals automatically
    let receipt = store.publish_snapshot(entries, false).unwrap();
    assert_eq!(receipt, PublishOutcome::Published);

    let loaded = store.find_snapshot(&manifest.hash).unwrap();
    assert_eq!(loaded.total_size, expected_unique_size);
}
