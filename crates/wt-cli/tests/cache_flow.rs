//! Ticket 02: staleness behavior through the CLI seam. The validation
//! cache beside the store must make warm creates cheaper without ever
//! changing what lands in a hydrated tree: edits arrive, touches stay
//! correct, and a missing or garbage cache just costs a cold run.

mod common;

use std::fs;
use std::path::Path;

use common::Fixture;

/// Total bytes and file count of everything under the store root
/// except the validation cache beside it — the externally observable
/// measure of how much content the store itself holds.
fn store_footprint(store: &Path) -> (usize, usize) {
    let mut files = 0usize;
    let mut bytes = 0usize;
    let mut stack = vec![store.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).expect("read store dir") {
            let p = entry.expect("store entry").path();
            if p.is_dir() {
                stack.push(p);
            } else if p.file_name().unwrap() != "ingest-cache.tsv" {
                files += 1;
                bytes += fs::metadata(&p).expect("store file metadata").len() as usize;
            }
        }
    }
    (files, bytes)
}

fn assert_success(output: &std::process::Output, what: &str) {
    assert!(
        output.status.success(),
        "{what} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Byte-compare the hydrated heavy directory against the source.
fn assert_heavy_matches_source(src: &Path, dest_root: &Path) {
    fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in fs::read_dir(dir).expect("read heavy dir") {
            let p = entry.expect("entry").path();
            if p.is_dir() {
                walk(&p, out);
            } else {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    walk(src, &mut files);
    assert!(!files.is_empty(), "fixture has no heavy files");
    for f in files {
        let rel = f.strip_prefix(src).unwrap();
        let hydrated = dest_root.join("heavy").join(rel);
        assert_eq!(
            fs::read(&f).unwrap(),
            fs::read(&hydrated).unwrap_or_else(|e| panic!("cannot read {}: {e}", hydrated.display())),
            "hydrated bytes differ for {}",
            rel.display()
        );
    }
}

#[test]
fn edited_source_lands_in_the_next_hydrated_tree() {
    let fx = Fixture::heavy_repo(8);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let base = tempfile::tempdir().expect("tempdir");
    let store = base.path().join("store");

    assert_success(&fx.wt_with_store(&["create", "one"], &store), "first create");

    let target = fx.repo.join("heavy/pkg00/nested/file-0.txt");
    fs::write(&target, "edited between creates\n").expect("edit source");
    let (files_before, _bytes_before) = store_footprint(&store);

    assert_success(&fx.wt_with_store(&["create", "two"], &store), "second create");
    let (files_after, _bytes_after) = store_footprint(&store);
    assert!(
        files_after > files_before,
        "an edit must add its new content to the store"
    );

    // The fresh tree carries the edit; an older tree keeps its own bytes.
    let parent = fx.repo.parent().unwrap();
    assert_heavy_matches_source(&fx.repo.join("heavy"), &parent.join("origin-two"));
    assert_eq!(
        fs::read(parent.join("origin-one/heavy/pkg00/nested/file-0.txt")).unwrap(),
        b"fake-heavy file 0 of 8\n",
        "previously created worktrees are not rewritten"
    );
}

#[test]
fn warm_create_reads_no_unchanged_file_bytes() {
    let fx = Fixture::heavy_repo(12);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let base = tempfile::tempdir().expect("tempdir");
    let store = base.path().join("store");

    assert_success(&fx.wt_with_store(&["create", "one"], &store), "first create");

    // Make every heavy file unreadable. If warm ingest still read any
    // byte of them, this create would fail outright instead of
    // succeeding off cached ids alone.
    {
        use std::os::unix::fs::PermissionsExt;
        let heavy = fx.repo.join("heavy");
        let mut stack = vec![heavy.clone()];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir).expect("read heavy") {
                let p = entry.expect("entry").path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    let mut perms = fs::metadata(&p).unwrap().permissions();
                    perms.set_mode(0o000);
                    fs::set_permissions(&p, perms).unwrap();
                }
            }
        }
    }

    assert_success(
        &fx.wt_with_store(&["create", "two"], &store),
        "warm create must skip reading unchanged files",
    );

    // And the hydrated tree still has exactly the right bytes.
    let expected = |i: usize| format!("fake-heavy file {i} of 12\n");
    let dest = fx.repo.parent().unwrap().join("origin-two/heavy");
    for i in 0..12 {
        let p = dest.join(format!("pkg{:02}/nested/file-{i}.txt", i % 20));
        assert_eq!(
            fs::read(&p).unwrap_or_else(|e| panic!("cannot read {}: {e}", p.display())),
            expected(i).into_bytes(),
            "warm hydration produced wrong bytes for file {i}"
        );
    }
}

#[test]
fn touched_file_still_hydrates_correct_bytes() {
    let fx = Fixture::heavy_repo(6);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let base = tempfile::tempdir().expect("tempdir");
    let store = base.path().join("store");

    assert_success(&fx.wt_with_store(&["create", "one"], &store), "first create");

    // Touch without editing: bump the mtime, keep every byte.
    let target = fx.repo.join("heavy/pkg01/nested/file-1.txt");
    {
        let file = fs::OpenOptions::new()
            .append(true)
            .open(&target)
            .expect("open for touch");
        file.set_times(std::fs::FileTimes::new().set_modified(std::time::SystemTime::now()))
            .expect("bump mtime");
    }
    let (files_before, bytes_before) = store_footprint(&store);

    assert_success(&fx.wt_with_store(&["create", "two"], &store), "second create");

    // A touch is a miss (re-hashed), so no wrong bytes can slip
    // through — but identical content dedupes back to the same blob.
    let (files_after, bytes_after) = store_footprint(&store);
    assert_eq!(
        (files_before, bytes_before),
        (files_after, bytes_after),
        "re-hashing a touched file must not grow the store"
    );
    assert_heavy_matches_source(&fx.repo.join("heavy"), &fx.repo.parent().unwrap().join("origin-two"));
}

#[test]
fn deleted_cache_falls_back_to_full_reingest_and_populates_again() {
    let fx = Fixture::heavy_repo(10);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let base = tempfile::tempdir().expect("tempdir");
    let store = base.path().join("store");

    assert_success(&fx.wt_with_store(&["create", "one"], &store), "first create");
    assert!(
        store.join("ingest-cache.tsv").is_file(),
        "first ingest must populate the cache beside the store"
    );

    fs::remove_file(store.join("ingest-cache.tsv")).expect("delete cache");
    fs::write(
        fx.repo.join("heavy/pkg03/nested/file-3.txt"),
        "written while the cache was gone\n",
    )
    .expect("edit source");

    assert_success(&fx.wt_with_store(&["create", "two"], &store), "cold-ish create");
    assert!(
        store.join("ingest-cache.tsv").is_file(),
        "a cache-less store must be repopulated by the next ingest"
    );
    assert_heavy_matches_source(&fx.repo.join("heavy"), &fx.repo.parent().unwrap().join("origin-two"));
}

#[test]
fn corrupt_cache_degrades_to_full_reingest_not_wrong_output() {
    let fx = Fixture::heavy_repo(10);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let base = tempfile::tempdir().expect("tempdir");
    let store = base.path().join("store");

    assert_success(&fx.wt_with_store(&["create", "one"], &store), "first create");

    fs::write(store.join("ingest-cache.tsv"), "\u{0}\u{1}garbage\t\t\t\nnot-a-cache")
        .expect("corrupt cache");

    assert_success(
        &fx.wt_with_store(&["create", "two"], &store),
        "create with a corrupt cache",
    );
    assert_heavy_matches_source(&fx.repo.join("heavy"), &fx.repo.parent().unwrap().join("origin-two"));
}
