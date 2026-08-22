//! Founding end-to-end suite (ticket 01). Everything asserts through
//! the CLI boundary: exit codes, stdout/stderr, and files on disk.

mod common;

use std::fs;
use std::path::Path;

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

// --- ticket 02: worktree command and manifest ---

/// Assert the tree rooted at `src` and `dest` hold identical files with
/// byte-identical contents.
fn assert_same_tree(src: &Path, dest: &Path) {
    let a = list_files(src);
    let b = list_files(dest);
    let rel = |base: &Path, p: &Path| p.strip_prefix(base).unwrap().to_path_buf();
    let ra: Vec<_> = a.iter().map(|p| rel(src, p)).collect();
    let rb: Vec<_> = b.iter().map(|p| rel(dest, p)).collect();
    assert_eq!(ra, rb, "file sets differ");
    for (pa, pb) in a.iter().zip(b.iter()) {
        assert_eq!(
            fs::read(pa).unwrap(),
            fs::read(pb).unwrap(),
            "contents differ for {}",
            rel(src, pa).display()
        );
    }
}

#[test]
fn create_hydrates_manifest_dirs_byte_identical() {
    let fx = Fixture::heavy_repo(HEAVY_FILES);
    fs::write(fx.repo.join(".wtinclude"), "# heavy dirs\nheavy/\n").unwrap();

    let out = fx.wt(&["create", "demo"]);
    assert!(
        out.status.success(),
        "wt create failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let dest = fx.repo.parent().unwrap().join("origin-demo");
    assert_same_tree(&fx.repo.join("heavy"), &dest.join("heavy"));
}

#[test]
fn create_without_manifest_uses_defaults_and_writes_starter() {
    let fx = Fixture::heavy_repo(50);
    // node_modules is one of the documented defaults.
    fs::rename(fx.repo.join("heavy"), fx.repo.join("node_modules")).unwrap();

    let out = fx.wt(&["create", "demo"]);
    assert!(
        out.status.success(),
        "wt create failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let dest = fx.repo.parent().unwrap().join("origin-demo");
    assert_same_tree(&fx.repo.join("node_modules"), &dest.join("node_modules"));

    // A suggested starter manifest lands in the source repo root.
    let starter = fs::read_to_string(fx.repo.join(".wtinclude")).unwrap();
    assert!(starter.contains("node_modules"));
    assert!(String::from_utf8_lossy(&out.stdout).contains(".wtinclude"));
}

#[test]
fn output_lists_every_hydrated_directory_and_its_source() {
    let fx = Fixture::heavy_repo(HEAVY_FILES);
    fs::create_dir_all(fx.repo.join("artifacts/pkg")).unwrap();
    fs::write(fx.repo.join("artifacts/pkg/a.bin"), b"artifact").unwrap();
    fs::write(fx.repo.join(".wtinclude"), "heavy/\nartifacts/\n").unwrap();

    let out = fx.wt(&["create", "demo"]);
    assert!(out.status.success());

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("hydrated heavy") && stdout.contains("hydrated artifacts"),
        "stdout must name each hydrated directory:\n{stdout}"
    );
    assert!(
        stdout.contains(&fx.repo.display().to_string()),
        "stdout must name the hydration source:\n{stdout}"
    );
}

#[test]
fn create_reports_when_nothing_matches() {
    let fx = Fixture::heavy_repo(1);
    fs::remove_dir_all(fx.repo.join("heavy")).unwrap();

    let out = fx.wt(&["create", "demo"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("nothing to hydrate"), "stdout:\n{stdout}");
}

#[test]
fn explicit_manifest_that_is_missing_is_an_error() {
    let fx = Fixture::heavy_repo(5);
    let out = fx.wt(&["create", "demo", "--manifest", "nope.wtinclude"]);
    assert!(!out.status.success());
}

#[test]
fn manifest_patterns_match_nested_directories() {
    let fx = Fixture::heavy_repo(HEAVY_FILES);
    // Anchored pattern with a glob segment.
    fs::write(fx.repo.join(".wtinclude"), "/heavy/pkg0*/nested\n").unwrap();

    let out = fx.wt(&["create", "demo"]);
    assert!(out.status.success());

    let dest = fx.repo.parent().unwrap().join("origin-demo");
    let hydrated = list_files(&dest.join("heavy"));
    assert!(!hydrated.is_empty(), "nested pattern hydrated nothing");
    for f in &hydrated {
        assert!(
            f.to_string_lossy().contains("pkg0"),
            "unexpected file outside pkg0*: {f:?}"
        );
    }
}
