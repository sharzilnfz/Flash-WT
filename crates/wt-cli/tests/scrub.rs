// Tests assert with unwrap/expect by design: a panic IS the failure
// signal under test, so the workspace restriction lints stay off here.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! `wt scrub`: full re-hash repair pass over the store, asserted
//! end to end through the CLI seam.
//!
//! The trust model (verified-blob ledger) trusts blobs while size and
//! mtime stay unchanged, so the tampering here deliberately preserves
//! both — exactly the corruption only a scrub can see. The deleted-
//! blob half asserts through the library's typed errors: whatever
//! references a scrubbed-away address must fail loudly, never panic
//! and never serve bytes.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::{Fixture, list_files};
use wt_store::{ContentId, DiskStore, Store};

/// Flip the first byte of one stored blob, then restore its original
/// mtime: size never changes, so no stat-visible property does, and
/// only a re-hash could catch the tampering.
fn tamper_blob(store: &Path) -> PathBuf {
    let blob = list_files(&store.join("objects"))
        .into_iter()
        .find(|p| p.is_file())
        .expect("store has objects");
    let mtime = fs::metadata(&blob).unwrap().modified().unwrap();
    let mut bytes = fs::read(&blob).unwrap();
    bytes[0] ^= 0xff;
    // Disk-level corruption does not respect read-only inodes.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&blob).unwrap().permissions();
        perms.set_mode(perms.mode() | 0o200);
        fs::set_permissions(&blob, perms).unwrap();
    }
    fs::write(&blob, &bytes).unwrap();
    let f = fs::OpenOptions::new()
        .write(true)
        .open(&blob)
        .expect("reopen blob");
    f.set_times(fs::FileTimes::new().set_modified(mtime))
        .expect("restore mtime");
    blob
}

/// The content id named by an object path (`objects/<2 hex>/<62 hex>`).
fn blob_id(blob: &Path) -> ContentId {
    let shard = blob.parent().unwrap().file_name().unwrap();
    let hex = format!(
        "{}{}",
        shard.to_string_lossy(),
        blob.file_name().unwrap().to_string_lossy()
    );
    ContentId::from_hex(&hex).expect("object path parses as a content id")
}

fn wt(fx: &Fixture, args: &[&str], store: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_wt"))
        .args(args)
        .env("WT_STORE", store)
        .env("WT_SNAPSHOTS", "1")
        .env_remove("WT_HARDLINK")
        .env_remove("WT_NO_HARDLINK")
        .env_remove("WT_VERIFY")
        .current_dir(&fx.repo)
        .output()
        .expect("run wt binary")
}

fn fixture(files: usize) -> (Fixture, std::path::PathBuf) {
    let fx = Fixture::heavy_repo(files);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();
    let base = tempfile::tempdir().unwrap();
    let store = base.path().join("store");
    (fx, store)
}

#[test]
fn dry_run_reports_corruption_but_touches_nothing() {
    let (fx, store) = fixture(20);

    let first = wt(&fx, &["create", "one"], &store);
    assert!(
        first.status.success(),
        "create failed: {}",
        String::from_utf8_lossy(&first.stderr)
    );

    let blob = tamper_blob(&store);

    let out = wt(&fx, &["scrub", "--dry-run"], &store);
    assert!(
        out.status.success(),
        "scrub --dry-run failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("corrupt 1") && stdout.contains("would delete 1"),
        "dry run must report the corrupt blob:\n{stdout}"
    );
    assert!(blob.is_file(), "a dry run must not delete anything");

    // Trust model untouched: the ledger entry survived the dry run,
    // so a warm create skips re-verification entirely and succeeds.
    let second = wt(&fx, &["create", "two"], &store);
    assert!(
        second.status.success(),
        "warm create must still succeed after a dry-run scrub: {}",
        String::from_utf8_lossy(&second.stderr)
    );
}

#[test]
fn scrub_deletes_corrupt_blob_and_references_fail_cleanly() {
    let (fx, store) = fixture(20);

    let first = wt(&fx, &["create", "one"], &store);
    assert!(
        first.status.success(),
        "create failed: {}",
        String::from_utf8_lossy(&first.stderr)
    );

    let blob = tamper_blob(&store);
    let id = blob_id(&blob);

    let out = wt(&fx, &["scrub"], &store);
    assert!(
        out.status.success(),
        "scrub failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("corrupt 1") && stdout.contains("deleted 1"),
        "scrub must report what it found and removed:\n{stdout}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(&id.to_string()),
        "the corrupt blob must be named on stderr:\n{stderr}"
    );
    assert!(!blob.exists(), "the corrupt blob must be gone");

    // The ledger must not keep trusting the deleted address.
    let ledger = fs::read_to_string(store.join("verified.tsv")).unwrap();
    assert!(
        !ledger.contains(&id.to_string()),
        "verified-ledger entry for the deleted blob must be gone:\n{ledger}"
    );

    // Anything referencing the address now fails with a typed error,
    // not a panic and never bad bytes.
    let disk = DiskStore::open(&store).unwrap();
    assert!(matches!(
        Store::get(&disk, &id),
        Err(wt_store::Error::UnknownContent(_))
    ));
    assert!(matches!(
        disk.ensure_verified(&id),
        Err(wt_store::Error::UnknownContent(_))
    ));

    // With the corrupt entry gone, a rerun is clean.
    let again = wt(&fx, &["scrub"], &store);
    assert!(again.status.success());
    assert!(
        String::from_utf8_lossy(&again.stdout).contains("corrupt 0"),
        "rerun on the repaired store must find nothing:\n{}",
        String::from_utf8_lossy(&again.stdout)
    );
}

#[test]
fn scrub_on_healthy_store_deletes_nothing() {
    let (fx, store) = fixture(20);

    let first = wt(&fx, &["create", "one"], &store);
    assert!(
        first.status.success(),
        "create failed: {}",
        String::from_utf8_lossy(&first.stderr)
    );

    let before = list_files(&store.join("objects"));

    let out = wt(&fx, &["scrub"], &store);
    assert!(
        out.status.success(),
        "scrub failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let scanned = before.len();
    assert!(
        stdout.contains(&format!("scanned {scanned}"))
            && stdout.contains("corrupt 0")
            && stdout.contains("deleted 0"),
        "healthy store must scan everything and delete nothing:\n{stdout}"
    );
    assert_eq!(
        list_files(&store.join("objects")),
        before,
        "a healthy store must come through scrub byte-for-byte intact"
    );
}

#[test]
fn parallel_sharded_scrubbing_detects_multiple_blob_corruptions_and_emits_json() {
    let (fx, store) = fixture(100);

    let first = wt(&fx, &["create", "one"], &store);
    assert!(
        first.status.success(),
        "create failed: {}",
        String::from_utf8_lossy(&first.stderr)
    );

    let all_blobs = list_files(&store.join("objects"))
        .into_iter()
        .filter(|p| p.is_file())
        .collect::<Vec<_>>();
    assert!(all_blobs.len() >= 50, "expected at least 50 blobs");

    // Tamper 3 blobs across shards
    let mut tampered_ids = Vec::new();
    for &idx in &[0, all_blobs.len() / 2, all_blobs.len() - 1] {
        let blob = &all_blobs[idx];
        let id = blob_id(blob);
        tampered_ids.push(id.to_string());
        let mtime = fs::metadata(blob).unwrap().modified().unwrap();
        let mut bytes = fs::read(blob).unwrap();
        bytes[0] ^= 0xff;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(blob).unwrap().permissions();
            perms.set_mode(perms.mode() | 0o200);
            fs::set_permissions(blob, perms).unwrap();
        }
        fs::write(blob, &bytes).unwrap();
        let f = fs::OpenOptions::new().write(true).open(blob).unwrap();
        f.set_times(fs::FileTimes::new().set_modified(mtime))
            .unwrap();
    }

    // Dry run with --json
    let dry_out = wt(&fx, &["scrub", "--dry-run", "--json"], &store);
    assert!(dry_out.status.success());
    let dry_json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&dry_out.stdout).trim()).unwrap();
    assert_eq!(dry_json["status"], "ok");
    assert_eq!(dry_json["data"]["dry_run"], true);
    assert_eq!(dry_json["data"]["corrupt"].as_array().unwrap().len(), 3);
    assert_eq!(dry_json["data"]["deleted"], 0);

    let diagnostics = dry_json["diagnostics"].as_array().unwrap();
    assert!(diagnostics.iter().any(|d| d["code"] == "CORRUPT_BLOB"));

    // Real run with --json
    let real_out = wt(&fx, &["scrub", "--json"], &store);
    assert!(real_out.status.success());
    let real_json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&real_out.stdout).trim()).unwrap();
    assert_eq!(real_json["status"], "ok");
    assert_eq!(real_json["data"]["deleted"], 3);

    // Subsequent scrub finds 0 corrupt
    let clean_out = wt(&fx, &["scrub", "--json"], &store);
    assert!(clean_out.status.success());
    let clean_json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&clean_out.stdout).trim()).unwrap();
    assert_eq!(clean_json["data"]["corrupt"].as_array().unwrap().len(), 0);
}

fn publish_test_snapshot(store: &Path) -> PathBuf {
    use wt_store::SnapshotEntry;
    let mut ds = DiskStore::open(store).unwrap();
    let b1 = ds.put(b"snap test content 1").unwrap();
    let b2 = ds.put(b"snap test content 2").unwrap();
    let entries = vec![
        SnapshotEntry::file("f1.txt", b1, 0o644),
        SnapshotEntry::file("sub/f2.txt", b2, 0o644),
    ];
    let manifest = wt_store::Manifest::new(entries.clone()).unwrap();
    ds.publish_snapshot(entries, false).unwrap();
    store.join("snapshots").join(manifest.hash.to_string())
}

#[test]
fn scrub_detects_and_purges_snapshot_with_missing_complete_marker() {
    let (fx, store) = fixture(10);
    let snap = publish_test_snapshot(&store);
    assert!(snap.is_dir(), "expected published snapshot directory");

    let complete_file = snap.join(".complete");
    if complete_file.exists() {
        fs::remove_file(&complete_file).unwrap();
    }

    // Dry run
    let dry_out = wt(&fx, &["scrub", "--dry-run", "--json"], &store);
    assert!(dry_out.status.success());
    let dry_json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&dry_out.stdout).trim()).unwrap();
    let corrupt_snaps = dry_json["data"]["corrupt_snapshots"].as_array().unwrap();
    assert!(!corrupt_snaps.is_empty());
    assert_eq!(dry_json["data"]["snapshot_dirs_deleted"], 0);
    assert!(snap.exists());

    let diagnostics = dry_json["diagnostics"].as_array().unwrap();
    assert!(diagnostics.iter().any(|d| d["code"] == "CORRUPT_SNAPSHOT"));

    // Real run purges the broken snapshot directory
    let real_out = wt(&fx, &["scrub", "--json"], &store);
    assert!(real_out.status.success());
    let real_json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&real_out.stdout).trim()).unwrap();
    assert!(real_json["data"]["snapshot_dirs_deleted"].as_u64().unwrap() >= 1);
    assert!(!snap.exists());
}

#[test]
fn scrub_detects_and_purges_snapshot_with_unparseable_manifest() {
    let (fx, store) = fixture(10);
    let snap = publish_test_snapshot(&store);
    assert!(snap.is_dir());

    fs::write(snap.join("manifest.tsv"), "not a valid manifest header\n").unwrap();

    let real_out = wt(&fx, &["scrub", "--json"], &store);
    assert!(real_out.status.success());
    let real_json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&real_out.stdout).trim()).unwrap();
    assert!(real_json["data"]["snapshot_dirs_deleted"].as_u64().unwrap() >= 1);
    assert!(!snap.exists());
}

#[test]
fn scrub_detects_and_purges_snapshot_with_corrupted_file_tree() {
    let (fx, store) = fixture(10);
    let snap = publish_test_snapshot(&store);
    assert!(snap.is_dir());

    let tree_files = list_files(&snap.join("tree"))
        .into_iter()
        .filter(|p| p.is_file())
        .collect::<Vec<_>>();
    assert!(!tree_files.is_empty());

    // Corrupt one file in tree
    fs::write(&tree_files[0], b"corrupted file content in snapshot tree").unwrap();

    let real_out = wt(&fx, &["scrub", "--json"], &store);
    assert!(real_out.status.success());
    let real_json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&real_out.stdout).trim()).unwrap();
    assert!(real_json["data"]["snapshot_dirs_deleted"].as_u64().unwrap() >= 1);
    assert!(!snap.exists());
}
