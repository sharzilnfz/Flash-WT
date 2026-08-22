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
