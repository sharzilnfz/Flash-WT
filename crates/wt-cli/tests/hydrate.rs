//! Integration tests for standalone hydration command and product renaming (Ticket 09).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs;
use std::process::Command;

use common::{Fixture, list_files};

#[test]
fn wt_hydrate_populates_existing_git_worktree() {
    let fx = Fixture::heavy_repo(50);
    // node_modules is a standard default pattern
    fs::rename(fx.repo.join("heavy"), fx.repo.join("node_modules")).unwrap();
    let parent = fx.repo.parent().unwrap();
    let external_dest = parent.join("external-worktree");

    // Create an external worktree using raw git directly (simulating Worktrunk or manual setup)
    let git_add = Command::new("git")
        .args([
            "worktree",
            "add",
            "-b",
            "external-feature",
            &external_dest.to_string_lossy(),
            "HEAD",
        ])
        .current_dir(&fx.repo)
        .status()
        .expect("git worktree add");
    assert!(git_add.success(), "failed to create raw git worktree");
    assert!(external_dest.join(".git").is_file());
    assert!(!external_dest.join("node_modules").exists());

    // Hydrate using the canonical binary wt-hydrate
    let out = fx.wt_hydrate(&["hydrate", &external_dest.to_string_lossy()]);

    assert!(
        out.status.success(),
        "wt-hydrate hydrate failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Verify heavy directory materialized
    assert!(external_dest.join("node_modules").is_dir());
    let src_files = list_files(&fx.repo.join("node_modules"));
    let dest_files = list_files(&external_dest.join("node_modules"));
    assert_eq!(src_files.len(), 50);
    assert_eq!(dest_files.len(), 50);
}

#[test]
fn wt_hydrate_alias_works_and_supports_json_envelope() {
    let fx = Fixture::heavy_repo(30);
    fs::rename(fx.repo.join("heavy"), fx.repo.join("node_modules")).unwrap();
    let parent = fx.repo.parent().unwrap();
    let dest = parent.join("target-dir");
    fs::create_dir_all(&dest).expect("mkdir target-dir");

    // Hydrate using the `wt` alias with --json
    let out = fx.wt(&[
        "--json",
        "hydrate",
        &dest.to_string_lossy(),
        "--source",
        &fx.repo.to_string_lossy(),
    ]);

    assert!(
        out.status.success(),
        "wt hydrate failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("parse JSON");
    assert_eq!(json["status"], "ok");
    assert_eq!(json["command"], "hydrate");
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["data"]["files_hydrated"], 30);
    assert_eq!(
        json["data"]["source_path"],
        fx.repo.canonicalize().unwrap().display().to_string()
    );
    assert_eq!(
        json["data"]["destination_path"],
        dest.canonicalize().unwrap().display().to_string()
    );

    // Verify contents arrived
    assert_eq!(list_files(&dest.join("node_modules")).len(), 30);
}

#[test]
fn wt_hydrate_with_custom_manifest() {
    let fx = Fixture::heavy_repo(20);
    // Custom folder and manifest
    fs::write(fx.repo.join("custom.wtinclude"), "heavy/\n").unwrap();
    let parent = fx.repo.parent().unwrap();
    let dest = parent.join("custom-dest");
    fs::create_dir_all(&dest).expect("mkdir custom-dest");

    let out = fx.wt_hydrate(&[
        "hydrate",
        &dest.to_string_lossy(),
        "--manifest",
        &fx.repo.join("custom.wtinclude").to_string_lossy(),
    ]);

    assert!(
        out.status.success(),
        "wt-hydrate hydrate failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(list_files(&dest.join("heavy")).len(), 20);
}

#[test]
fn wt_init_creates_starter_manifest_and_wt_new_does_not_auto_create() {
    let fx = Fixture::heavy_repo(10);
    fs::rename(fx.repo.join("heavy"), fx.repo.join("node_modules")).unwrap();

    // 1. Verify .wtinclude does not exist initially
    assert!(!fx.repo.join(".wtinclude").exists());

    // 2. Run wt new without prior .wtinclude -> uses in-memory defaults, does not write file
    let new_out = fx.wt(&["new", "feature-1"]);
    assert!(
        new_out.status.success(),
        "wt new failed: {}",
        String::from_utf8_lossy(&new_out.stderr)
    );
    assert!(
        !fx.repo.join(".wtinclude").exists(),
        ".wtinclude must NOT be auto-created during wt new"
    );

    let dest1 = fx.repo.parent().unwrap().join("origin-feature-1");
    assert!(dest1.join("node_modules").is_dir());

    // 3. Run explicit `wt init`
    let init_out = fx.wt(&["init"]);
    assert!(
        init_out.status.success(),
        "wt init failed: {}",
        String::from_utf8_lossy(&init_out.stderr)
    );
    assert!(
        fx.repo.join(".wtinclude").is_file(),
        ".wtinclude must be created by wt init"
    );
    let content = fs::read_to_string(fx.repo.join(".wtinclude")).unwrap();
    assert!(content.contains("node_modules/"));
    assert!(content.contains("target/"));

    // 4. Running init again without --force fails
    let init_again = fx.wt(&["init"]);
    assert!(!init_again.status.success());

    // 5. Running init with --force succeeds
    let init_force = fx.wt(&["init", "--force"]);
    assert!(init_force.status.success());
}

#[test]
fn wt_hydrate_fails_when_destination_does_not_exist() {
    let fx = Fixture::heavy_repo(5);
    let non_existent = fx.repo.parent().unwrap().join("does-not-exist");

    let out = fx.wt(&["hydrate", &non_existent.to_string_lossy()]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("does not exist"));
}
