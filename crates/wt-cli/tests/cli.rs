//! Founding end-to-end suite (ticket 01). Everything asserts through
//! the CLI boundary: exit codes, stdout/stderr, and files on disk.

mod common;

use std::fs;

use common::{list_files, Fixture};

const HEAVY_FILES: usize = 2_000;

#[test]
fn help_lists_create() {
    let out = Fixture::heavy_repo(1).wt(&["create", "--help"]);
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("NAME"));
    assert!(text.contains("--manifest"));
    assert!(text.contains("--dir"));
}

#[test]
fn smoke_create_makes_a_working_worktree() {
    let fx = Fixture::heavy_repo(HEAVY_FILES);

    let out = fx.wt(&["create", "demo"]);
    assert!(
        out.status.success(),
        "wt create failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Default destination is a sibling of the repo named <repo>-<name>.
    let dest = fx.repo.parent().unwrap().join("origin-demo");
    assert!(
        dest.is_dir(),
        "worktree directory missing at {}",
        dest.display()
    );

    // It is a real git worktree (worktrees carry a .git file pointer).
    assert!(dest.join(".git").is_file());

    // Tracked content arrived.
    assert_eq!(
        fs::read_to_string(dest.join("src.txt")).unwrap(),
        "tracked source\n"
    );

    // The CLI names what it did and what it has not done yet.
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("created worktree"));
    assert!(stdout.contains(dest.display().to_string().as_str()));
}

#[test]
fn heavy_fixture_is_actually_thousands_of_files() {
    let fx = Fixture::heavy_repo(HEAVY_FILES);
    let files = list_files(&fx.repo.join("heavy"));
    assert_eq!(files.len(), HEAVY_FILES);
}

#[test]
fn create_fails_when_destination_exists() {
    let fx = Fixture::heavy_repo(5);
    let taken = fx.repo.parent().unwrap().join("origin-taken");
    fs::create_dir_all(&taken).unwrap();

    let out = fx.wt(&["create", "taken"]);
    assert!(!out.status.success());
}
