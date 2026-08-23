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

#[test]
fn incremental_publish_matches_full_build_semantics() {
    let (_base, mut store) = incremental_fixture();
    use crate::Store as _;

    let old_entries = v2_entries(&mut store);
    let old_manifest = Manifest::new(old_entries.clone()).unwrap();
    assert_eq!(
        store.publish_snapshot(old_entries.clone(), false).unwrap(),
        Ok(PublishOutcome::Published)
    );

    // Bump ONE root-level file; pkg00 stays fully unchanged -> the
    // single maximal unit.
    let extra = store.put(b"root.txt v2\n").unwrap();
    let mut new_entries = old_entries.clone();
    new_entries.retain(|e| e.rel != "root.txt");
    new_entries.push(SnapshotEntry::file("root.txt", extra, 0o644));
    let new_manifest = Manifest::new(new_entries.clone()).unwrap();
    assert_ne!(new_manifest.hash, old_manifest.hash);
    let units = crate::snapdiff::SnapshotDiff::unchanged_units(
        &old_manifest.entries,
        &new_manifest.entries,
    );
    // Both top-level dirs survive untouched; each is a unit.
    assert_eq!(units, vec!["empty-dir".to_string(), "pkg00".to_string()]);

    let mut timing = SnapshotBuildTiming::default();
    let outcome = store
        .publish_snapshot_incremental(
            new_entries,
            &old_manifest.hash,
            &units,
            false,
            Some(&mut timing),
        )
        .unwrap();
    assert_eq!(outcome, Ok(PublishOutcome::Published));
    // The whole point: the package unit cloned on APFS; elsewhere
    // every clone refuses and falls back to per-entry links.
    if cfg!(target_os = "macos") {
        // Sub-millisecond clones truncate to 0 ms; only counts lie.
        assert_eq!(timing.clone_units, 2);
    } else {
        assert_eq!(timing.clone_units, 0);
    }

    // The published result validates and its tree carries EXACTLY the
    // new content: bumped file, untouched package, symlink, empty dir.
    assert_eq!(
        store.find_snapshot(&new_manifest.hash).unwrap(),
        new_manifest
    );
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
    store.flush().unwrap();
}

#[test]
fn deleted_subtree_and_unitless_rebuild_still_land_exactly() {
    let (_base, mut store) = incremental_fixture();

    let old_entries = v2_entries(&mut store);
    let old_manifest = Manifest::new(old_entries.clone()).unwrap();
    assert_eq!(
        store.publish_snapshot(old_entries.clone(), false).unwrap(),
        Ok(PublishOutcome::Published)
    );

    // New manifest drops pkg00 entirely and keeps only the empty dir
    // plus bumped root file. No units at all; deletions need nothing.
    use crate::Store as _;
    let extra = store.put(b"root.txt v2\n").unwrap();
    let gone_entries = vec![
        SnapshotEntry::dir("empty-dir"),
        SnapshotEntry::file("root.txt", extra, 0o644),
    ];
    let gone_manifest = Manifest::new(gone_entries.clone()).unwrap();
    assert_eq!(
        store
            .publish_snapshot_incremental(gone_entries, &old_manifest.hash, &[], false, None)
            .unwrap(),
        Ok(PublishOutcome::Published)
    );
    let tree = snapshot_tree_path(store.root(), &gone_manifest.hash);
    assert!(!tree.join("pkg00").exists(), "deleted subtree must vanish");
    assert!(tree.join("empty-dir").is_dir());
    store.flush().unwrap();
}

#[test]
fn failed_unit_clone_falls_back_to_per_entry_placement() {
    let (_base, mut store) = incremental_fixture();

    let entries = v2_entries(&mut store);
    let manifest_hash = Manifest::new(entries.clone()).unwrap().hash;

    // A ghost old hash: no snapshot carries these units, so EVERY
    // clone attempt fails and every unit falls back to hardlinks —
    // yet the rebuild must still land exactly right.
    let ghost = ContentId([0xAA; 32]);
    let mut timing = SnapshotBuildTiming::default();
    assert_eq!(
        store
            .publish_snapshot_incremental(
                entries,
                &ghost,
                &["pkg00".to_string()],
                false,
                Some(&mut timing)
            )
            .unwrap(),
        Ok(PublishOutcome::Published)
    );
    assert_eq!(timing.clone_units, 0, "nothing could have cloned");

    let tree = snapshot_tree_path(store.root(), &manifest_hash);
    assert!(tree.join("empty-dir").is_dir());
    let md = fs::symlink_metadata(tree.join("pkg00/bin-link")).unwrap();
    assert!(md.file_type().is_symlink());
    assert_eq!(
        fs::read_link(tree.join("pkg00/bin-link")).unwrap(),
        Path::new("../run.sh")
    );
    store.flush().unwrap();
}

#[test]
fn paranoid_incremental_rejects_rotted_blob_inside_a_cloned_unit() {
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
        Ok(PublishOutcome::Published)
    );

    // Tamper with a blob INSIDE the unit, preserving size AND mtime so
    // ledger trust cannot see it. The paranoid pass over cloned files
    // must catch it before the rename.
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
        .publish_snapshot_incremental(
            new_entries,
            &old_manifest.hash,
            &["pkg00".to_string(), "empty-dir".to_string()],
            true,
            None,
        )
        .unwrap();
    match err {
        Err(BuildError::Fatal(msg)) => {
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
    let restored = v2_entries(&mut store);
    let mut restored = restored;
    restored.retain(|e| e.rel != "root.txt");
    restored.push(SnapshotEntry::file("root.txt", extra, 0o644));
    assert_eq!(
        store
            .publish_snapshot_incremental(
                restored,
                &old_manifest.hash,
                &["pkg00".to_string(), "empty-dir".to_string()],
                true,
                None
            )
            .unwrap(),
        Ok(PublishOutcome::Published)
    );
    store.flush().unwrap();
}
