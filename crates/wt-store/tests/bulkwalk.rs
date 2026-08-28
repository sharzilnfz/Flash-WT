// Tests assert with unwrap/expect by design: a panic IS the failure
// signal under test, so the workspace restriction lints stay off here.
#![allow(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]

//! Differential test for the macOS bulk walker (Step 0 follow-up).
//!
//! Builds a small but adversarial tree — nested directories, an empty
//! directory, chmod-varied files, one symlink — walks it with BOTH
//! `bulkwalk::walk` and a portable read_dir+symlink_metadata reference
//! implementation, and asserts the two agree on every path's kind,
//! size, mtime, and mode. Any parse bug in the attrbuffer layout shows
//! up as a mismatch here.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;
use std::time::UNIX_EPOCH;

use tempfile::TempDir;
use wt_store::bulkwalk;

#[test]
fn thread_local_scratch_buffer_reused_without_allocation() {
    let mut initial_ptr: *const u8 = std::ptr::null();
    bulkwalk::with_scratch_buffer(|buf| {
        assert_eq!(buf.len(), bulkwalk::INITIAL_BUFFER_CAPACITY);
        assert_eq!(buf.len(), 32 * 1024);
        initial_ptr = buf.as_ptr();
    });

    for _ in 0..500 {
        bulkwalk::with_scratch_buffer(|buf| {
            assert_eq!(buf.len(), bulkwalk::INITIAL_BUFFER_CAPACITY);
            assert_eq!(
                buf.as_ptr(),
                initial_ptr,
                "scratch buffer pointer must remain stable"
            );
        });
    }
}

#[test]
fn scratch_buffer_doubles_on_erange_up_to_1mb() {
    bulkwalk::with_scratch_buffer(|buf| {
        assert_eq!(buf.len(), 32 * 1024);
        let mut cap = buf.len();
        while cap < bulkwalk::MAX_BUFFER_CAPACITY {
            let new_cap = (cap * 2).min(bulkwalk::MAX_BUFFER_CAPACITY);
            buf.resize(new_cap, 0);
            cap = buf.len();
        }
        assert_eq!(buf.len(), 1024 * 1024);
        assert_eq!(buf.len(), bulkwalk::MAX_BUFFER_CAPACITY);
    });

    bulkwalk::with_scratch_buffer(|buf| {
        assert_eq!(buf.len(), bulkwalk::MAX_BUFFER_CAPACITY);
    });
}

/// What one entry looks like from the portable side.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RefEntry {
    is_dir: bool,
    is_symlink: bool,
    is_file: bool,
    size: u64,
    mtime_secs: u64,
    mtime_nanos: u32,
    mode: u32,
}

/// The legacy reference walk: read_dir for names plus one
/// symlink_metadata per entry. Never follows symlinks.
fn legacy_walk(root: &Path) -> BTreeMap<String, RefEntry> {
    let mut out = BTreeMap::new();
    fn rec(dir: &Path, prefix: &str, out: &mut BTreeMap<String, RefEntry>) {
        let entries = fs::read_dir(dir).expect("read_dir");
        for entry in entries.flatten() {
            let rel = if prefix.is_empty() {
                entry.file_name().to_string_lossy().into_owned()
            } else {
                format!("{prefix}/{}", entry.file_name().to_string_lossy())
            };
            let meta = fs::symlink_metadata(entry.path()).expect("symlink_metadata");
            let ft = meta.file_type();
            let since = meta.modified().expect("mtime").duration_since(UNIX_EPOCH);
            let (secs, nanos) = match since {
                Ok(d) => (d.as_secs(), d.subsec_nanos()),
                Err(_) => (0, 0),
            };
            out.insert(
                rel.clone(),
                RefEntry {
                    is_dir: ft.is_dir(),
                    is_symlink: ft.is_symlink(),
                    is_file: ft.is_file(),
                    // DATALENGTH on non-regular files is not st_size;
                    // only regular-file sizes are compared.
                    size: if ft.is_file() { meta.len() } else { 0 },
                    mtime_secs: secs,
                    mtime_nanos: nanos,
                    mode: meta.mode() & 0o7777,
                },
            );
            if ft.is_dir() {
                rec(&entry.path(), &rel, out);
            }
        }
    }
    rec(root, "", &mut out);
    out
}

fn bulk_map(entries: Vec<bulkwalk::BulkEntry>) -> BTreeMap<String, RefEntry> {
    entries
        .into_iter()
        .map(|e| {
            (
                e.rel_path,
                RefEntry {
                    is_dir: e.is_dir,
                    is_symlink: e.is_symlink,
                    is_file: e.is_file,
                    size: if e.is_file { e.size } else { 0 },
                    mtime_secs: e.mtime_secs,
                    mtime_nanos: e.mtime_nanos,
                    mode: e.mode & 0o7777,
                },
            )
        })
        .collect()
}

fn build_tree(root: &Path) {
    let nested = root.join("a/b/c");
    fs::create_dir_all(&nested).expect("nested dirs");
    fs::create_dir(root.join("empty")).expect("empty dir");

    fs::write(root.join("a/b/c/deep.txt"), "deep content\n").expect("deep file");
    let plain = root.join("plain.txt");
    fs::write(&plain, "root level\n").expect("root file");
    let exec = root.join("exec.sh");
    fs::write(&exec, "#!/bin/sh\necho hi\n").expect("exec file");
    fs::set_permissions(&exec, fs::Permissions::from_mode(0o755)).expect("chmod 755");
    let private = root.join("a/private.key");
    fs::write(&private, "secret\n").expect("private file");
    fs::set_permissions(&private, fs::Permissions::from_mode(0o600)).expect("chmod 600");
    std::os::unix::fs::symlink("c/deep.txt", root.join("a/rel-link")).expect("symlink");
}

#[test]
#[cfg(target_os = "macos")]
fn bulk_walk_agrees_with_read_dir_reference_walk() {
    let dir = TempDir::new().expect("tempdir");
    build_tree(dir.path());

    let reference = legacy_walk(dir.path());
    assert!(
        reference.contains_key("a/b/c/deep.txt"),
        "reference walk must reach the deepest file"
    );
    assert!(reference.contains_key("empty"), "empty dir must appear");
    assert!(reference.contains_key("a/rel-link"), "symlink must appear");

    let bulk = bulk_map(bulkwalk::walk(dir.path()).expect("bulk walk"));

    let ref_paths: Vec<_> = reference.keys().collect();
    let bulk_paths: Vec<_> = bulk.keys().collect();
    assert_eq!(ref_paths, bulk_paths, "identical rel-path sets");

    for (rel, want) in &reference {
        let got = &bulk[rel];
        assert_eq!(got, want, "entry {rel} differs between walkers");
        assert_eq!(
            got.is_symlink, want.is_symlink,
            "kind flags must match exactly"
        );
    }

    // Spot-check the kinds the test tree was built to contain.
    assert!(bulk["empty"].is_dir && !bulk["empty"].is_file);
    assert!(bulk["exec.sh"].is_file && bulk["exec.sh"].mode == 0o755);
    assert!(bulk["a/private.key"].mode == 0o600);
    assert!(bulk["a/rel-link"].is_symlink);
    assert!(!bulk["a/rel-link"].is_file);
}
