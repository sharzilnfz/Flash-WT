//! Ticket 04 acceptance tests. Everything runs in temp directories so
//! no test touches a real project tree.

use std::fs;
use std::time::Instant;

use tempfile::TempDir;
use wt_store::{ContentId, DiskStore, Error, Store};

fn temp_root() -> TempDir {
    TempDir::new().expect("temp dir")
}

/// Overwrite every byte of every file under `root` with `b'X'`. Used
/// to simulate on-disk corruption without knowing the store layout.
fn clobber_all_files(root: &std::path::Path) {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).expect("read_dir") {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                files.push(path);
            }
        }
    }
    assert!(!files.is_empty(), "store should have files on disk");
    for path in files {
        let len = fs::metadata(&path).expect("meta").len() as usize;
        fs::write(&path, vec![b'X'; len.max(1)]).expect("clobber");
    }
}

#[test]
fn put_get_round_trips_byte_identical_content() {
    let dir = temp_root();
    let mut store = DiskStore::open(dir.path()).expect("open");

    let content = "hello, store \0 with embedded nulls and unicode \u{1f600}".as_bytes();
    let id = store.put(content).expect("put");

    assert_eq!(store.get(&id).expect("get"), content);
}

#[test]
fn empty_content_round_trips() {
    let dir = temp_root();
    let mut store = DiskStore::open(dir.path()).expect("open");

    let id = store.put(b"").expect("put empty");
    assert_eq!(store.get(&id).expect("get"), b"");
}

#[test]
fn identical_content_stored_once_on_disk() {
    let dir = temp_root();
    let mut store = DiskStore::open(dir.path()).expect("open");

    let first = store.put(b"duplicate me").expect("put 1");
    let second = store.put(b"duplicate me").expect("put 2");

    assert_eq!(first, second);

    let mut count = 0;
    let mut stack = vec![dir.path().to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in fs::read_dir(&current).expect("read_dir") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                stack.push(path);
            } else {
                count += 1;
            }
        }
    }
    // Exactly one blob plus whatever metadata the store needs, never
    // two copies of the content.
    assert!(
        count <= 2,
        "identical content must occupy disk once, found {count} files"
    );
}

#[test]
fn different_content_gets_different_ids() {
    let dir = temp_root();
    let mut store = DiskStore::open(dir.path()).expect("open");

    let a = store.put(b"a").expect("put a");
    let b = store.put(b"b").expect("put b");
    assert_ne!(a, b);
}

#[test]
fn put_does_not_take_a_reference() {
    let dir = temp_root();
    let mut store = DiskStore::open(dir.path()).expect("open");

    let id = store.put(b"unreferenced").expect("put");
    assert_eq!(store.ref_count(&id).expect("ref_count"), 0);
}

#[test]
fn reference_counts_increment_and_decrement() {
    let dir = temp_root();
    let mut store = DiskStore::open(dir.path()).expect("open");

    let id = store.put(b"counted").expect("put");
    store.add_ref(&id).expect("add_ref 1");
    store.add_ref(&id).expect("add_ref 2");
    assert_eq!(store.ref_count(&id).expect("after adds"), 2);

    store.release_ref(&id).expect("release 1");
    assert_eq!(store.ref_count(&id).expect("after release"), 1);
}

#[test]
fn release_past_zero_is_an_underflow_error() {
    let dir = temp_root();
    let mut store = DiskStore::open(dir.path()).expect("open");

    let id = store.put(b"underflow target").expect("put");
    store.add_ref(&id).expect("add_ref");
    store.release_ref(&id).expect("release to zero");

    match store.release_ref(&id) {
        Err(Error::RefCountUnderflow(past)) => assert_eq!(past, id),
        other => panic!("expected RefCountUnderflow, got {other:?}"),
    }
}

#[test]
fn refs_on_unknown_content_error() {
    let dir = temp_root();
    let mut store = DiskStore::open(dir.path()).expect("open");
    let missing = ContentId([9u8; 32]);

    assert!(matches!(
        store.add_ref(&missing),
        Err(Error::UnknownContent(_))
    ));
    assert!(matches!(
        store.release_ref(&missing),
        Err(Error::RefCountUnderflow(_))
    ));
    assert!(matches!(
        store.ref_count(&missing),
        Err(Error::UnknownContent(_))
    ));
}

#[test]
fn get_unknown_content_errors() {
    let dir = temp_root();
    let store = DiskStore::open(dir.path()).expect("open");
    let missing = ContentId([7u8; 32]);

    assert!(!store.contains(&missing));
    assert!(matches!(store.get(&missing), Err(Error::UnknownContent(_))));
}

#[test]
fn corrupted_blob_returns_error_not_bad_data() {
    let dir = temp_root();
    let mut store = DiskStore::open(dir.path()).expect("open");

    let id = store.put(b"soon to be corrupted").expect("put");
    assert!(store.contains(&id));

    clobber_all_files(dir.path());

    assert!(matches!(store.get(&id), Err(Error::Corrupted(c)) if c == id));
}

#[test]
fn separate_handles_see_each_others_writes() {
    let dir = temp_root();
    let mut a = DiskStore::open(dir.path()).expect("open a");
    let b = DiskStore::open(dir.path()).expect("open b");

    let id = a.put(b"visible across handles").expect("put through a");

    assert!(b.contains(&id));
    assert_eq!(
        b.get(&id).expect("get through b"),
        b"visible across handles"
    );

    let mut c = DiskStore::open(dir.path()).expect("open c");
    c.add_ref(&id).expect("add_ref through c");
    assert_eq!(a.ref_count(&id).expect("seen from a"), 1);
}

#[test]
fn suite_is_fast_and_self_contained() {
    let start = Instant::now();
    for i in 0..25 {
        let dir = temp_root();
        let mut store = DiskStore::open(dir.path()).expect("open");
        let content = format!("iteration {i}").into_bytes();
        let id = store.put(&content).expect("put");
        assert_eq!(store.get(&id).expect("get"), content);
    }
    assert!(
        start.elapsed().as_secs() < 10,
        "store suite must not touch real project trees or be slow"
    );
}

// --- ticket 07: linking verified content out with shared-write protection ---

#[test]
fn link_out_shares_a_read_only_inode() {
    use std::os::unix::fs::MetadataExt;

    let dir = temp_root();
    let mut store = DiskStore::open(dir.path()).expect("open");

    let id = store.put(b"linked into a tree").expect("put");
    let dest = dir.path().join("tree/file.txt");
    fs::create_dir_all(dest.parent().unwrap()).expect("mkdir tree");

    store.link_out(&id, &dest).expect("link_out");

    assert_eq!(fs::read(&dest).expect("read"), b"linked into a tree");
    // One blob plus one linked copy of it behind one inode: the tree
    // file and the store object are the same content, stored once.
    assert_eq!(fs::metadata(&dest).expect("meta").nlink(), 2);
    assert!(
        fs::metadata(&dest).expect("meta").permissions().readonly(),
        "shared inode must be read-only"
    );
    assert!(
        fs::write(&dest, b"poison").is_err(),
        "in-place rewrite of a linked-out blob must fail"
    );
    // The store object shares the inode, so it is protected too.
    assert_eq!(store.get(&id).expect("get"), b"linked into a tree");
}

#[test]
fn link_out_unknown_content_errors_without_creating_dest() {
    let dir = temp_root();
    let store = DiskStore::open(dir.path()).expect("open");
    let missing = ContentId([3u8; 32]);
    let dest = dir.path().join("out/file.txt");

    match store.link_out(&missing, &dest) {
        Err(Error::UnknownContent(id)) => assert_eq!(id, missing),
        other => panic!("expected UnknownContent, got {other:?}"),
    }
    assert!(!dest.exists());
}

#[test]
fn link_out_corrupted_content_errors_without_creating_dest() {
    let dir = temp_root();
    let mut store = DiskStore::open(dir.path()).expect("open");

    let id = store.put(b"soon corrupted").expect("put");
    clobber_all_files(dir.path());
    let dest = dir.path().join("out/file.txt");

    match store.link_out(&id, &dest) {
        Err(Error::Corrupted(c)) => assert_eq!(c, id),
        other => panic!("expected Corrupted, got {other:?}"),
    }
    assert!(!dest.exists(), "corrupt bytes must never land in a tree");
}

// --- ticket 02: the ingest validation cache ---

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use wt_store::{Entry as CacheEntry, ValidationCache};

/// Stat a file and build the cache entry an ingest would record.
fn entry_for(path: &std::path::Path) -> (u64, SystemTime) {
    let meta = fs::metadata(path).expect("stat source file");
    (meta.len(), meta.modified().expect("mtime"))
}

fn write_source(root: &TempDir, text: &str) -> std::path::PathBuf {
    let path = root.path().join("heavy/file.txt");
    fs::create_dir_all(path.parent().unwrap()).expect("mkdir heavy");
    fs::write(&path, text).expect("write source");
    path
}

fn record_for(cache: &mut ValidationCache, store: &mut DiskStore, path: &std::path::Path) -> ContentId {
    let bytes = fs::read(path).expect("read source");
    let id = store.put(&bytes).expect("put");
    let (size, mtime) = entry_for(path);
    cache.record(
        "heavy/file.txt".to_string(),
        CacheEntry { size, mtime, id },
    );
    id
}

#[test]
fn cache_hits_on_unchanged_file() {
    let store_dir = temp_root();
    let src = TempDir::new().expect("tempdir");
    let path = write_source(&src, "version one\n");

    let mut store = DiskStore::open(store_dir.path()).expect("open store");
    let mut cache = ValidationCache::open(store_dir.path());
    assert!(cache.is_empty(), "fresh store has no cache yet");
    let id = record_for(&mut cache, &mut store, &path);
    cache.save().expect("save");

    // Next run, same size and mtime: hit, no read needed.
    let reopened = ValidationCache::open(store_dir.path());
    let (size, mtime) = entry_for(&path);
    assert_eq!(
        reopened.lookup("heavy/file.txt", size, mtime),
        Some(id),
        "unchanged size+mtime must be a hit"
    );
}

#[test]
fn cache_misses_on_mtime_change_with_same_size() {
    let store_dir = temp_root();
    let src = TempDir::new().expect("tempdir");
    let path = write_source(&src, "touch me\n");

    let mut store = DiskStore::open(store_dir.path()).expect("open");
    let mut cache = ValidationCache::open(store_dir.path());
    let id = record_for(&mut cache, &mut store, &path);
    cache.save().expect("save");

    let reopened = ValidationCache::open(store_dir.path());
    let (size, mtime) = entry_for(&path);
    let touched = mtime + Duration::from_secs(1);
    assert_eq!(
        reopened.lookup("heavy/file.txt", size, touched),
        None,
        "a bumped mtime must be re-hashed even at the same size"
    );
    assert_eq!(reopened.lookup("heavy/file.txt", size, mtime), Some(id));
}

#[test]
fn cache_misses_on_size_change_with_same_mtime() {
    let store_dir = temp_root();
    let src = TempDir::new().expect("tempdir");
    let path = write_source(&src, "grow me\n");

    let mut store = DiskStore::open(store_dir.path()).expect("open");
    let mut cache = ValidationCache::open(store_dir.path());
    let id = record_for(&mut cache, &mut store, &path);
    cache.save().expect("save");

    let reopened = ValidationCache::open(store_dir.path());
    let (size, mtime) = entry_for(&path);
    assert_eq!(
        reopened.lookup("heavy/file.txt", size + 1, mtime),
        None,
        "a changed size must be re-hashed even at the same mtime"
    );
    assert_eq!(reopened.lookup("heavy/file.txt", size, mtime), Some(id));
}

#[test]
fn cache_misses_after_content_change() {
    let store_dir = temp_root();
    let src = TempDir::new().expect("tempdir");
    let path = write_source(&src, "before edit\n");

    let mut store = DiskStore::open(store_dir.path()).expect("open");
    let mut cache = ValidationCache::open(store_dir.path());
    let old_id = record_for(&mut cache, &mut store, &path);
    cache.save().expect("save");

    // Edit the content: both size and mtime move in practice.
    fs::write(&path, "after edit, longer\n").expect("edit source");
    let (size, mtime) = entry_for(&path);

    let reopened = ValidationCache::open(store_dir.path());
    assert_eq!(reopened.lookup("heavy/file.txt", size, mtime), None);
    assert_ne!(store.put(b"after edit, longer\n").expect("put"), old_id);
}

#[test]
fn pre_existing_store_without_cache_opens_empty_and_populates() {
    let dir = temp_root();
    let mut store = DiskStore::open(dir.path()).expect("open");
    let src = TempDir::new().expect("tempdir");
    let path = write_source(&src, "first ingest ever\n");

    // A store that predates the cache: no cache file exists yet.
    let mut cache = ValidationCache::open(dir.path());
    assert!(cache.is_empty(), "missing cache must open empty");

    let id = record_for(&mut cache, &mut store, &path);
    cache.save().expect("populate");

    // The next run finds it populated.
    let reopened = ValidationCache::open(dir.path());
    let (size, mtime) = entry_for(&path);
    assert_eq!(reopened.len(), 1);
    assert_eq!(reopened.lookup("heavy/file.txt", size, mtime), Some(id));
}

#[test]
fn deleted_cache_degrades_to_empty_not_error() {
    let dir = temp_root();
    let mut store = DiskStore::open(dir.path()).expect("open");
    let src = TempDir::new().expect("tempdir");
    let path = write_source(&src, "cache me\n");

    let mut cache = ValidationCache::open(dir.path());
    record_for(&mut cache, &mut store, &path);
    cache.save().expect("save");
    fs::remove_file(dir.path().join("ingest-cache.tsv")).expect("delete cache");

    let reopened = ValidationCache::open(dir.path());
    assert!(reopened.is_empty(), "deleted cache must fall back to cold");
    assert_eq!(reopened.lookup("heavy/file.txt", 0, SystemTime::UNIX_EPOCH), None);
}

#[test]
fn corrupt_cache_lines_are_dropped_not_trusted() {
    let dir = temp_root();
    fs::write(
        dir.path().join("ingest-cache.tsv"),
        "not a valid line\n\
         heavy/short\t12\tx\t0\n\
         heavy/badid\t5\t1\t2\tzzzz\n\
         heavy/too\tmany\ttabs\there\tok\textra\n",
    )
    .expect("write garbage cache");

    let cache = ValidationCache::open(dir.path());
    assert!(
        cache.is_empty(),
        "garbage entries must vanish instead of serving wrong ids"
    );
}

#[test]
fn save_is_atomic_enough_to_round_trip() {
    let dir = temp_root();
    let mut cache = ValidationCache::open(dir.path());
    cache.record(
        "heavy/a.txt".into(),
        CacheEntry {
            size: 3,
            mtime: UNIX_EPOCH + Duration::from_secs(1_700_000_000) + Duration::from_nanos(42),
            id: ContentId([7u8; 32]),
        },
    );
    cache.record(
        "heavy/nested/b.txt".into(),
        CacheEntry {
            size: 9,
            mtime: UNIX_EPOCH + Duration::from_secs(1),
            id: ContentId([8u8; 32]),
        },
    );
    cache.save().expect("save");

    let reopened = ValidationCache::open(dir.path());
    assert_eq!(reopened.len(), 2);
    assert_eq!(
        reopened.lookup(
            "heavy/a.txt",
            3,
            UNIX_EPOCH + Duration::from_secs(1_700_000_000) + Duration::from_nanos(42)
        ),
        Some(ContentId([7u8; 32])),
        "sub-second mtime precision must survive the round trip"
    );
}

// --- fast-hydration ticket 05: the verified-blob ledger ---

use sha2::Digest;
use std::fs::FileTimes;
use wt_store::VerifiedLedger;

/// Flip a blob's first byte in place, optionally restoring the file's
/// original mtime exactly afterwards. Size never changes, so with the
/// mtime restored no stat-visible property does either — only a real
/// re-hash could catch the tampering.
fn tamper_blob(store_root: &std::path::Path, id: &ContentId, restore_mtime: bool) {
    let hex = id.to_string();
    let path = store_root.join("objects").join(&hex[..2]).join(&hex[2..]);
    let meta = fs::metadata(&path).expect("stat blob");
    let mtime = meta.modified().expect("blob mtime");
    let mut bytes = fs::read(&path).expect("read blob");
    assert!(!bytes.is_empty(), "tampering an empty blob proves nothing");
    bytes[0] ^= 0xff;
    fs::write(&path, &bytes).expect("tamper");
    if restore_mtime {
        let f = fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("reopen blob");
        f.set_times(FileTimes::new().set_modified(mtime))
            .expect("restore mtime");
    }
}

#[test]
fn ensure_verified_hashes_once_then_trusts_the_fingerprint() {
    let dir = temp_root();
    let mut store = DiskStore::open(dir.path()).expect("open");
    let id = store.put(b"verified once\n").expect("put");
    drop(store); // Drop flushes the ledger to disk.

    // A fresh handle has only the persisted ledger. Tampering that no
    // stat can see must go unnoticed: the fingerprint still matches,
    // so not a single byte is re-read.
    let store = DiskStore::open(dir.path()).expect("reopen");
    tamper_blob(dir.path(), &id, true);
    store
        .ensure_verified(&id)
        .expect("trust path must not re-hash");
}

#[test]
fn ensure_verified_rehashes_when_the_fingerprint_moves() {
    let dir = temp_root();
    let mut store = DiskStore::open(dir.path()).expect("open");
    let id = store.put(b"fingerprint me\n").expect("put");
    drop(store);

    let store = DiskStore::open(dir.path()).expect("reopen");
    tamper_blob(dir.path(), &id, false);
    match store.ensure_verified(&id) {
        Err(Error::Corrupted(c)) => assert_eq!(c, id),
        other => panic!("expected Corrupted, got {other:?}"),
    }
}

#[test]
fn ensure_verified_records_nothing_on_a_mismatch() {
    let dir = temp_root();
    let mut store = DiskStore::open(dir.path()).expect("open");
    let id = store.put(b"never trusted\n").expect("put");
    drop(store);

    let store = DiskStore::open(dir.path()).expect("reopen");
    tamper_blob(dir.path(), &id, false);
    assert!(matches!(
        store.ensure_verified(&id),
        Err(Error::Corrupted(_))
    ));
    drop(store);

    // The failed check recorded nothing, so the next run pays for a
    // full hash again instead of trusting corrupt bytes.
    let store = DiskStore::open(dir.path()).expect("re-reopen");
    assert!(matches!(
        store.ensure_verified(&id),
        Err(Error::Corrupted(_))
    ));
}

#[test]
fn first_touch_of_a_never_verified_blob_verifies() {
    let dir = temp_root();
    // A blob at a well-formed address whose bytes were placed by hand:
    // no put ever ran, so no ledger entry can exist.
    let id = ContentId([0xab_u8; 32]);
    let hex = id.to_string();
    let path = dir.path().join("objects").join(&hex[..2]).join(&hex[2..]);
    fs::create_dir_all(path.parent().unwrap()).expect("mkdir shard");
    fs::write(&path, b"bytes nobody hashed\n").expect("place blob by hand");

    let store = DiskStore::open(dir.path()).expect("open");
    match store.ensure_verified(&id) {
        Err(Error::Corrupted(_)) => {}
        other => panic!("first touch must verify, got {other:?}"),
    }

    // And a genuinely matching hand-placed blob passes and is then
    // trusted on the next call.
    let content = b"honestly addressed\n";
    let good = ContentId(sha2::Sha256::digest(content).into());
    let hex = good.to_string();
    let gpath = dir.path().join("objects").join(&hex[..2]).join(&hex[2..]);
    fs::create_dir_all(gpath.parent().unwrap()).expect("mkdir shard");
    fs::write(&gpath, content).expect("place honest blob");
    store.ensure_verified(&good).expect("first touch verifies");
    tamper_blob(dir.path(), &good, true);
    store
        .ensure_verified(&good)
        .expect("verified once means trusted after");
}

#[test]
fn put_records_a_fingerprint_verified_by_construction() {
    let dir = temp_root();
    let mut store = DiskStore::open(dir.path()).expect("open");
    let id = store.put(b"addressed by hash\n").expect("put");
    drop(store);

    // No ensure_verified ever ran; the ledger entry came from put.
    let store = DiskStore::open(dir.path()).expect("reopen");
    tamper_blob(dir.path(), &id, true);
    store
        .ensure_verified(&id)
        .expect("put's fingerprint must be persisted with the ledger");
}

#[test]
fn delete_drops_the_ledger_entry_best_effort() {
    let dir = temp_root();
    let mut store = DiskStore::open(dir.path()).expect("open");
    let id = store.put(b"swept away\n").expect("put");
    store.delete(&id).expect("delete");
    drop(store);

    // Someone reuses the freed address for different bytes. With the
    // entry gone there is nothing to trust: full verification runs.
    let hex = id.to_string();
    let path = dir.path().join("objects").join(&hex[..2]).join(&hex[2..]);
    fs::create_dir_all(path.parent().unwrap()).expect("mkdir shard");
    fs::write(&path, b"different bytes\n").expect("reuse address");
    let store = DiskStore::open(dir.path()).expect("reopen");
    match store.ensure_verified(&id) {
        Err(Error::Corrupted(_)) => {}
        other => panic!("stale trust must not survive deletion, got {other:?}"),
    }
}

#[test]
fn missing_or_corrupt_ledger_degrades_to_full_verification() {
    let dir = temp_root();
    let mut store = DiskStore::open(dir.path()).expect("open");
    let id = store.put(b"no ledger survives\n").expect("put");
    drop(store);
    tamper_blob(dir.path(), &id, false); // fingerprint now stale

    // Deleted ledger: the next open starts cold...
    fs::remove_file(dir.path().join("verified.tsv")).expect("rm ledger");
    let store = DiskStore::open(dir.path()).expect("reopen");
    assert!(matches!(
        store.ensure_verified(&id),
        Err(Error::Corrupted(_))
    ));

    // ...and a corrupt one parses to entries that match nothing, which
    // is the same thing as starting cold.
    fs::write(
        dir.path().join("verified.tsv"),
        "garbage\nzz\t5\t1\t1\nab\toops\n",
    )
    .expect("corrupt the ledger");
    let store = DiskStore::open(dir.path()).expect("reopen");
    assert!(matches!(
        store.ensure_verified(&id),
        Err(Error::Corrupted(_))
    ));
}

#[test]
fn get_stays_always_hash_regardless_of_the_ledger() {
    let dir = temp_root();
    let mut store = DiskStore::open(dir.path()).expect("open");
    let id = store.put(b"api contract\n").expect("put");
    drop(store);

    let store = DiskStore::open(dir.path()).expect("reopen");
    tamper_blob(dir.path(), &id, true);
    // ensure_verified trusts; get never does. Both contracts hold at
    // the same time on the same store.
    store.ensure_verified(&id).expect("ledger hit");
    assert!(matches!(store.get(&id), Err(Error::Corrupted(_))));
}

#[test]
fn unknown_content_stays_unknown_through_ensure_verified() {
    let dir = temp_root();
    let store = DiskStore::open(dir.path()).expect("open");
    let missing = ContentId([4u8; 32]);
    assert!(matches!(
        store.ensure_verified(&missing),
        Err(Error::UnknownContent(m)) if m == missing
    ));
}

#[test]
fn ledger_round_trips_sub_second_mtimes_through_disk() {
    let dir = temp_root();
    let content = b"precision\n";
    let id = ContentId(sha2::Sha256::digest(content).into());
    {
        let mut store = DiskStore::open(dir.path()).expect("open");
        store.put(content).expect("put");
        // Give the blob an exotic mtime, then record it through a real
        // verification.
        let hex = id.to_string();
        let path = dir.path().join("objects").join(&hex[..2]).join(&hex[2..]);
        let f = fs::OpenOptions::new().write(true).open(&path).unwrap();
        f.set_times(FileTimes::new().set_modified(
            UNIX_EPOCH + Duration::from_secs(1_700_000_000) + Duration::from_nanos(7),
        ))
        .unwrap();
        store.ensure_verified(&id).expect("verify");
    } // Drop persists the ledger.

    // The round-tripped nanosecond mtime still matches on reopen.
    let meta = {
        let hex = id.to_string();
        let path = dir.path().join("objects").join(&hex[..2]).join(&hex[2..]);
        fs::metadata(&path).expect("blob survived")
    };
    let mtime = meta.modified().unwrap();
    let store = DiskStore::open(dir.path()).expect("reopen");
    store
        .ensure_verified(&id)
        .expect("nanosecond precision must survive the save");

    let ledger = VerifiedLedger::open(dir.path());
    assert!(
        ledger.matches(&id, meta.len(), mtime),
        "the persisted entry must match a fresh stat"
    );
    assert!(!ledger.matches(&id, meta.len() + 1, mtime));
    assert!(!ledger.matches(&id, meta.len(), mtime + Duration::from_nanos(1)));
}
