// Tests assert with unwrap/expect by design: a panic IS the failure
// signal under test, so the workspace restriction lints stay off here.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Ticket 04 acceptance tests. Everything runs in temp directories so
//! no test touches a real project tree.

use std::fs;
use std::time::Instant;

use tempfile::TempDir;
use wt_store::{ContentId, DiskStore, Error};

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

fn set_file_mtime(path: &std::path::Path, time: SystemTime) {
    use std::os::unix::ffi::OsStrExt;
    let dur = time.duration_since(UNIX_EPOCH).expect("time since epoch");
    let ts = libc::timespec {
        tv_sec: dur.as_secs() as _,
        tv_nsec: dur.subsec_nanos() as _,
    };
    let times = [ts, ts];
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).expect("cstring");
    let ret = unsafe { libc::utimensat(libc::AT_FDCWD, c_path.as_ptr(), times.as_ptr(), 0) };
    assert_eq!(ret, 0, "utimensat failed");
}

/// Stat a file and build the cache entry an ingest would record.
fn entry_for(path: &std::path::Path) -> (u64, SystemTime, u64, SystemTime) {
    use std::os::unix::fs::MetadataExt;
    let meta = fs::metadata(path).expect("stat source file");
    let ctime_secs = meta.ctime().max(0) as u64;
    let ctime_nanos = meta.ctime_nsec().clamp(0, 999_999_999) as u32;
    let ctime = UNIX_EPOCH + Duration::new(ctime_secs, ctime_nanos);
    (
        meta.len(),
        meta.modified().expect("mtime"),
        meta.ino(),
        ctime,
    )
}

fn write_source(root: &TempDir, text: &str) -> std::path::PathBuf {
    let path = root.path().join("heavy/file.txt");
    fs::create_dir_all(path.parent().unwrap()).expect("mkdir heavy");
    fs::write(&path, text).expect("write source");
    // Backdate mtime so it is not near now, simulating normal repo files on disk.
    set_file_mtime(&path, UNIX_EPOCH + Duration::from_secs(1_700_000_000));
    path
}

fn record_for(
    cache: &mut ValidationCache,
    store: &mut DiskStore,
    path: &std::path::Path,
) -> ContentId {
    let bytes = fs::read(path).expect("read source");
    let id = store.put(&bytes).expect("put");
    let (size, mtime, inode, ctime) = entry_for(path);
    cache.record(
        "heavy/file.txt".to_string(),
        CacheEntry {
            size,
            mtime,
            inode,
            ctime,
            id,
        },
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

    // Next run, same size, mtime, inode, and ctime: hit, no read needed.
    let reopened = ValidationCache::open(store_dir.path());
    let (size, mtime, inode, ctime) = entry_for(&path);
    assert_eq!(
        reopened.lookup("heavy/file.txt", size, mtime, inode, ctime),
        Some(id),
        "unchanged size+mtime+inode+ctime must be a hit"
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
    let (size, mtime, inode, ctime) = entry_for(&path);
    let touched = mtime + Duration::from_secs(1);
    assert_eq!(
        reopened.lookup("heavy/file.txt", size, touched, inode, ctime),
        None,
        "a bumped mtime must be re-hashed even at the same size"
    );
    assert_eq!(
        reopened.lookup("heavy/file.txt", size, mtime, inode, ctime),
        Some(id)
    );
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
    let (size, mtime, inode, ctime) = entry_for(&path);
    assert_eq!(
        reopened.lookup("heavy/file.txt", size + 1, mtime, inode, ctime),
        None,
        "a changed size must be re-hashed even at the same mtime"
    );
    assert_eq!(
        reopened.lookup("heavy/file.txt", size, mtime, inode, ctime),
        Some(id)
    );
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
    let (size, mtime, inode, ctime) = entry_for(&path);

    let reopened = ValidationCache::open(store_dir.path());
    assert_eq!(
        reopened.lookup("heavy/file.txt", size, mtime, inode, ctime),
        None
    );
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
    let (size, mtime, inode, ctime) = entry_for(&path);
    assert_eq!(reopened.len(), 1);
    assert_eq!(
        reopened.lookup("heavy/file.txt", size, mtime, inode, ctime),
        Some(id)
    );
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
    assert_eq!(
        reopened.lookup(
            "heavy/file.txt",
            0,
            SystemTime::UNIX_EPOCH,
            0,
            SystemTime::UNIX_EPOCH
        ),
        None
    );
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
            inode: 12345,
            ctime: UNIX_EPOCH + Duration::from_secs(1_700_000_001),
            id: ContentId([7u8; 32]),
        },
    );
    cache.record(
        "heavy/nested/b.txt".into(),
        CacheEntry {
            size: 9,
            mtime: UNIX_EPOCH + Duration::from_secs(1),
            inode: 67890,
            ctime: UNIX_EPOCH + Duration::from_secs(2),
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
            UNIX_EPOCH + Duration::from_secs(1_700_000_000) + Duration::from_nanos(42),
            12345,
            UNIX_EPOCH + Duration::from_secs(1_700_000_001),
        ),
        Some(ContentId([7u8; 32])),
        "sub-second mtime precision and inode/ctime must survive the round trip"
    );
}

// --- deep ingestion: tree ingestion through the store interface ---

use wt_store::IngestOptions;

fn no_excludes() -> IngestOptions<'static> {
    IngestOptions {
        snapshots: false,
        exclude: &|_| false,
    }
}

/// Build `heavy/` under `root` with two files, a nested directory,
/// and a relative symlink.
fn write_ingest_tree(root: &std::path::Path) {
    fs::create_dir_all(root.join("heavy/pkg")).expect("mkdir heavy/pkg");
    fs::write(root.join("heavy/a.txt"), "alpha\n").expect("write a");
    fs::write(root.join("heavy/pkg/b.txt"), "beta\n").expect("write b");
    #[cfg(unix)]
    std::os::unix::fs::symlink("a.txt", root.join("heavy/link.txt")).expect("symlink");
}

#[test]
fn ingest_tree_walks_stores_and_summarizes_a_tree() {
    let store_dir = temp_root();
    let src = TempDir::new().expect("tempdir");
    write_ingest_tree(src.path());

    let mut store = DiskStore::open(store_dir.path()).expect("open store");
    let ingested = store
        .ingest_tree(src.path(), &src.path().join("heavy"), &no_excludes())
        .expect("ingest");

    assert_eq!(
        ingested.files.keys().collect::<Vec<_>>(),
        ["heavy/a.txt", "heavy/pkg/b.txt"]
    );
    assert!(ingested.dirs.contains(&"heavy".to_string()));
    assert!(ingested.dirs.contains(&"heavy/pkg".to_string()));
    assert_eq!(ingested.file_sizes["heavy/a.txt"], 6);
    assert_eq!(
        ingested.symlinks.get("heavy/link.txt").map(String::as_str),
        Some("a.txt"),
        "symlink targets are recorded raw, never followed"
    );

    // Every recorded address really holds the file's bytes.
    for (rel, id) in &ingested.files {
        let expected = match rel.as_str() {
            "heavy/a.txt" => "alpha\n",
            "heavy/pkg/b.txt" => "beta\n",
            other => panic!("unexpected ingested path {other}"),
        };
        assert_eq!(store.get(id).expect("get"), expected.as_bytes());
    }
}

#[test]
fn ingest_tree_records_directory_permission_bits() {
    use std::os::unix::fs::PermissionsExt;

    let store_dir = temp_root();
    let src = TempDir::new().expect("tempdir");
    write_ingest_tree(src.path());
    fs::set_permissions(
        src.path().join("heavy/pkg"),
        fs::Permissions::from_mode(0o750),
    )
    .expect("chmod");

    let mut store = DiskStore::open(store_dir.path()).expect("open store");
    let ingested = store
        .ingest_tree(src.path(), &src.path().join("heavy"), &no_excludes())
        .expect("ingest");

    assert_eq!(ingested.dir_modes["heavy/pkg"] & 0o7777, 0o750);
    assert_eq!(ingested.modes["heavy/a.txt"] & 0o7777, 0o644);
}

#[test]
fn ingest_tree_rehashes_changed_content_and_feeds_the_cache() {
    let store_dir = temp_root();
    let src = TempDir::new().expect("tempdir");
    write_ingest_tree(src.path());

    let mut store = DiskStore::open(store_dir.path()).expect("open store");
    let first = store
        .ingest_tree(src.path(), &src.path().join("heavy"), &no_excludes())
        .expect("first ingest");

    // The warm re-ingest of an unchanged tree returns the same ids.
    let warm = store
        .ingest_tree(src.path(), &src.path().join("heavy"), &no_excludes())
        .expect("warm ingest");
    assert_eq!(warm.files, first.files);

    // Edit one file: the next ingest must pick up new content.
    fs::write(src.path().join("heavy/a.txt"), "alpha, edited\n").expect("edit");
    let second = store
        .ingest_tree(src.path(), &src.path().join("heavy"), &no_excludes())
        .expect("re-ingest after edit");
    assert_ne!(second.files["heavy/a.txt"], first.files["heavy/a.txt"]);
    assert_eq!(
        store.get(&second.files["heavy/a.txt"]).expect("get edited"),
        b"alpha, edited\n"
    );
    assert_eq!(
        second.files["heavy/pkg/b.txt"],
        first.files["heavy/pkg/b.txt"]
    );

    // The validation cache beside the store now carries both paths.
    let cache = ValidationCache::open(store_dir.path());
    assert!(
        cache.len() >= 2,
        "ingest must populate the validation cache"
    );
}

#[test]
fn forced_mtime_collision_and_identical_size_rehashes_fresh_content() {
    let store_dir = temp_root();
    let src = TempDir::new().expect("tempdir");
    let heavy = src.path().join("heavy");
    fs::create_dir_all(&heavy).expect("mkdir");
    let file = heavy.join("item.txt");
    fs::write(&file, "initial content\n").expect("write initial");

    let t0 = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    set_file_mtime(&file, t0);

    let mut store = DiskStore::open(store_dir.path()).expect("open store");
    let first = store
        .ingest_tree(src.path(), &heavy, &no_excludes())
        .expect("first ingest");
    let id1 = first.files["heavy/item.txt"];

    // Overwrite in-place with different content of the exact same byte length (16 bytes).
    fs::write(&file, "updated content\n").expect("write updated");
    // Force mtime collision with the prior cached timestamp.
    set_file_mtime(&file, t0);

    // Ingest again: despite identical size and forced mtime collision,
    // ctime (or inode) changed so it must not reuse id1.
    let second = store
        .ingest_tree(src.path(), &heavy, &no_excludes())
        .expect("second ingest");
    let id2 = second.files["heavy/item.txt"];
    assert_ne!(
        id1, id2,
        "fresh content must be re-hashed despite mtime and size collision"
    );
    assert_eq!(
        store.get(&id2).expect("get updated blob"),
        b"updated content\n"
    );
}

#[test]
fn near_now_mtime_rehashes_even_with_identical_size_and_mtime() {
    let store_dir = temp_root();
    let src = TempDir::new().expect("tempdir");
    let heavy = src.path().join("heavy");
    fs::create_dir_all(&heavy).expect("mkdir");
    let file = heavy.join("fast.txt");
    fs::write(&file, "rapid write 1\n").expect("write 1");

    let mut store = DiskStore::open(store_dir.path()).expect("open store");
    let first = store
        .ingest_tree(src.path(), &heavy, &no_excludes())
        .expect("first ingest");
    let id1 = first.files["heavy/fast.txt"];

    // Immediate rewrite within the same time window (< 2s) with identical size (14 bytes).
    fs::write(&file, "rapid write 2\n").expect("write 2");

    let second = store
        .ingest_tree(src.path(), &heavy, &no_excludes())
        .expect("second ingest");
    let id2 = second.files["heavy/fast.txt"];
    assert_ne!(
        id1, id2,
        "near-now rewrite must rehash rather than serving stale blob"
    );
    assert_eq!(store.get(&id2).expect("get"), b"rapid write 2\n");
}

#[test]
fn ingest_tree_reputs_a_swept_blob_on_a_cache_hit() {
    let store_dir = temp_root();
    let src = TempDir::new().expect("tempdir");
    write_ingest_tree(src.path());

    let mut store = DiskStore::open(store_dir.path()).expect("open store");
    let first = store
        .ingest_tree(src.path(), &src.path().join("heavy"), &no_excludes())
        .expect("first ingest");

    // Simulate a sweep reclaiming the blob behind a warm cache entry:
    // size and mtime still match, but the blob is gone from the store.
    let id = first.files["heavy/a.txt"];
    let hex = id.to_string();
    let blob = store_dir
        .path()
        .join("objects")
        .join(&hex[..2])
        .join(&hex[2..]);
    fs::remove_file(&blob).expect("sweep blob");
    assert!(!store.contains(&id));

    let second = store
        .ingest_tree(src.path(), &src.path().join("heavy"), &no_excludes())
        .expect("re-ingest after sweep");
    assert_eq!(second.files["heavy/a.txt"], id, "same bytes, same address");
    assert!(store.contains(&id));
    assert_eq!(store.get(&id).expect("get"), b"alpha\n");
}

#[test]
fn ingest_tree_skips_excluded_paths_and_subtrees() {
    let store_dir = temp_root();
    let src = TempDir::new().expect("tempdir");
    write_ingest_tree(src.path());
    fs::create_dir_all(src.path().join("heavy/skip")).expect("mkdir skip");
    fs::write(src.path().join("heavy/skip/x.txt"), "skip me\n").expect("write x");
    fs::write(src.path().join("heavy/skipme.txt"), "skip me too\n").expect("write skipme");

    let options = IngestOptions {
        snapshots: false,
        exclude: &|rel: &str| rel == "heavy/skip" || rel.ends_with("skipme.txt"),
    };
    let mut store = DiskStore::open(store_dir.path()).expect("open store");
    let ingested = store
        .ingest_tree(src.path(), &src.path().join("heavy"), &options)
        .expect("ingest");

    assert!(!ingested.files.contains_key("heavy/skipme.txt"));
    assert!(!ingested.files.contains_key("heavy/skip/x.txt"));
    assert!(!ingested.dirs.contains(&"heavy/skip".to_string()));
    assert!(ingested.files.contains_key("heavy/a.txt"));
}

#[test]
fn ingest_tree_fails_loudly_on_fifos_only_when_snapshots_enabled() {
    let store_dir = temp_root();
    let src = TempDir::new().expect("tempdir");
    write_ingest_tree(src.path());
    let fifo = src.path().join("heavy/pipe");
    let c_path = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).expect("cstr");
    assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o644) }, 0, "mkfifo");

    let mut store = DiskStore::open(store_dir.path()).expect("open store");
    assert!(
        store
            .ingest_tree(
                src.path(),
                &src.path().join("heavy"),
                &IngestOptions {
                    snapshots: true,
                    exclude: &|_| false
                }
            )
            .is_err(),
        "snapshot mode must refuse content a manifest cannot represent"
    );

    // Without snapshots the same tree degrades to a quiet skip.
    let ingested = store
        .ingest_tree(src.path(), &src.path().join("heavy"), &no_excludes())
        .expect("ingest without snapshots");
    assert!(!ingested.files.contains_key("heavy/pipe"));
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
    } else {
        // Inode timestamps come from the kernel's COARSE clock (up to
        // ~4ms granularity with HZ=250). A tamper that lands in the
        // same tick as the put would keep the old mtime, and the
        // ledger's trust hit is then legitimate under the documented
        // trust model ("bit rot preserving size AND mtime between
        // checks"). Tests must not depend on tick luck: force an
        // unambiguous mtime change.
        let shifted = mtime + std::time::Duration::from_secs(60);
        let f = fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("reopen blob");
        f.set_times(FileTimes::new().set_modified(shifted))
            .expect("shift mtime");
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

#[test]
fn save_without_changes_is_a_noop_not_a_rewrite() {
    let dir = temp_root();

    // A freshly opened, untouched cache must not write anything.
    let mut cache = ValidationCache::open(dir.path());
    cache.save().expect("clean save");
    assert!(
        !dir.path().join("ingest-cache.tsv").exists(),
        "a clean save must not create the cache file"
    );

    // After recording, the save persists — and a subsequent clean
    // save on the reopened cache leaves it alone.
    let mut store = DiskStore::open(dir.path()).expect("open");
    let src = TempDir::new().expect("tempdir");
    let path = write_source(&src, "dirty tracking\n");
    record_for(&mut cache, &mut store, &path);
    cache.save().expect("dirty save");
    let before = fs::metadata(dir.path().join("ingest-cache.tsv"))
        .expect("cache exists after dirty save")
        .modified()
        .expect("mtime");

    let mut reopened = ValidationCache::open(dir.path());
    reopened.save().expect("clean save on reopened cache");
    let after = fs::metadata(dir.path().join("ingest-cache.tsv"))
        .expect("cache still there")
        .modified()
        .expect("mtime");
    assert_eq!(before, after, "clean save must not rewrite the TSV");
}
