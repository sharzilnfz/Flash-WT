// Tests assert with unwrap/expect by design: a panic IS the failure
// signal under test, so the workspace restriction lints stay off here.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Ticket 05: hydration flows through the content-addressed store.
//! Everything is asserted through the CLI seam: two worktrees of the
//! same project must share store content, and corrupt store content
//! must fail loudly instead of landing in a fresh tree.

mod common;

use std::fs;
use std::path::Path;

use common::{list_files, Fixture};

/// Total bytes and file count of everything under the store root —
/// the externally observable measure of how much content it holds.
fn store_footprint(store: &Path) -> (usize, usize) {
    let mut files = 0usize;
    let mut bytes = 0usize;
    let mut stack = vec![store.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).expect("read store dir") {
            let p = entry.expect("store entry").path();
            if p.is_dir() {
                // GC bookkeeping (ticket 07 mirrors) is not content.
                if p.file_name() == Some(std::ffi::OsStr::new("worktrees")) {
                    continue;
                }
                stack.push(p);
            } else {
                files += 1;
                bytes += fs::metadata(&p).expect("store file metadata").len() as usize;
            }
        }
    }
    (files, bytes)
}

fn assert_same_tree(src: &Path, dest: &Path) {
    let rel = |base: &Path, p: &Path| p.strip_prefix(base).unwrap().to_path_buf();
    let a = list_files(src);
    let b = list_files(dest);
    assert_eq!(
        a.iter().map(|p| rel(src, p)).collect::<Vec<_>>(),
        b.iter().map(|p| rel(dest, p)).collect::<Vec<_>>(),
        "file sets differ"
    );
    for (pa, pb) in a.iter().zip(b.iter()) {
        assert_eq!(fs::read(pa).unwrap(), fs::read(pb).unwrap());
    }
}

#[test]
fn second_worktree_adds_no_new_store_content() {
    let fx = Fixture::heavy_repo(300);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let base = tempfile::tempdir().expect("tempdir");
    let store = base.path().join("store");
    let wt = |name: &str| fx.wt_with_store(&["create", name], &store);

    let first = wt("one");
    assert!(
        first.status.success(),
        "first create failed: {}",
        String::from_utf8_lossy(&first.stderr)
    );

    let (files_after_first, bytes_after_first) = store_footprint(&store);
    assert!(files_after_first > 0, "store stayed empty after ingest");

    let second = wt("two");
    assert!(
        second.status.success(),
        "second create failed: {}",
        String::from_utf8_lossy(&second.stderr)
    );

    // Dedupe: ingesting the same heavy directory again stored nothing
    // new — every blob already existed at its hash address.
    let (files_after_second, bytes_after_second) = store_footprint(&store);
    assert_eq!(
        files_after_first, files_after_second,
        "second worktree added new store objects"
    );
    assert_eq!(
        bytes_after_first, bytes_after_second,
        "second worktree grew the store's footprint"
    );

    // Both projections are complete and byte-identical to the source.
    let dest_two = fx.repo.parent().unwrap().join("origin-two");
    assert_same_tree(&fx.repo.join("heavy"), &dest_two.join("heavy"));
    let dest_one = fx.repo.parent().unwrap().join("origin-one");
    assert_same_tree(&fx.repo.join("heavy"), &dest_one.join("heavy"));

    // Each worktree holds a reference ledger ticket 06 will consume.
    // A worktree's .git is a pointer file, so resolve the linked
    // gitdir it names.
    for name in ["one", "two"] {
        let wt_root = fx.repo.parent().unwrap().join(format!("origin-{name}"));
        let pointer = fs::read_to_string(wt_root.join(".git")).expect("worktree .git pointer");
        let git_dir = pointer
            .trim()
            .strip_prefix("gitdir: ")
            .expect(".git points at a gitdir");
        let ledger = Path::new(git_dir).join("wt-hydrated.tsv");
        let text = fs::read_to_string(&ledger)
            .unwrap_or_else(|e| panic!("missing ledger {}: {e}", ledger.display()));
        assert_eq!(text.lines().count(), 300, "ledger incomplete for {name}");
    }

    let stdout = String::from_utf8_lossy(&second.stdout);
    assert!(
        stdout.contains("via store"),
        "second create must say it flowed through the store:\n{stdout}"
    );
}

#[test]
fn hash_mismatch_during_materialize_fails_loudly() {
    let fx = Fixture::heavy_repo(20);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let base = tempfile::tempdir().expect("tempdir");
    let store = base.path().join("store");

    let first = fx.wt_with_store(&["create", "good"], &store);
    assert!(first.status.success());

    // Corrupt one stored blob behind the CLI's back. `put` skips
    // existing objects by path, so the next ingest keeps the bad blob
    // and materialize is the step that must catch it.
    let objects = store.join("objects");
    let blob = list_files(&objects)
        .into_iter()
        .find(|p| p.is_file())
        .expect("store has objects");
    let mut bytes = fs::read(&blob).unwrap();
    bytes[0] ^= 0xff;
    // Since ticket 07, linked-out blobs carry a read-only shared
    // inode; disk-level corruption does not respect that, so neither
    // does the simulation.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&blob).unwrap().permissions();
        perms.set_mode(perms.mode() | 0o200);
        fs::set_permissions(&blob, perms).unwrap();
    }
    fs::write(&blob, &bytes).unwrap();

    let second = fx.wt_with_store(&["create", "bad"], &store);
    assert!(
        !second.status.success(),
        "corrupt store content must fail the create"
    );
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(
        stderr.contains("hash verification"),
        "failure must name hash verification loudly:\n{stderr}"
    );
}
