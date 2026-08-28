// Tests assert with unwrap/expect by design: a panic IS the failure
// signal under test, so the workspace restriction lints stay off here.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Integration tests verifying ref-aware branch stacking metadata,
//! symbolic ref preservation, and base movement diagnostics (ticket 02).

mod common;

use common::Fixture;
use std::fs;
use std::process::Command;

fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn create_with_base_records_symbolic_ref_and_initial_commit_in_mirror() {
    let store_dir = tempfile::tempdir().unwrap();
    let fx = Fixture::heavy_repo(10);
    git(&fx.repo, &["branch", "-M", "main"]);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let initial_main_commit = git(&fx.repo, &["rev-parse", "HEAD"]);

    let out = fx.wt_with_store(&["create", "stack-1", "--base", "main"], store_dir.path());
    assert!(
        out.status.success(),
        "wt create --base failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let mirrors = wt_store::mirror::read_all(store_dir.path());
    assert!(!mirrors.is_empty(), "mirror must be published in store");

    let mirror = mirrors
        .iter()
        .find_map(|r| r.mirror.as_ref().ok())
        .expect("valid mirror");

    assert_eq!(mirror.base_branch.as_deref(), Some("main"));
    assert_eq!(
        mirror.base_commit.as_deref(),
        Some(initial_main_commit.as_str())
    );
}

#[test]
fn base_movement_detected_in_json_and_human_output() {
    let fx = Fixture::heavy_repo(10);
    git(&fx.repo, &["branch", "-M", "main"]);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let initial_main_commit = git(&fx.repo, &["rev-parse", "HEAD"]);

    // Create branch 1 based on main
    let out1 = fx.wt(&["create", "feat-1", "--base", "main"]);
    assert!(out1.status.success());

    // Advance main with a new commit
    fs::write(fx.repo.join("new-file.txt"), "advanced main\n").unwrap();
    git(&fx.repo, &["add", "new-file.txt"]);
    git(&fx.repo, &["commit", "-m", "advance main"]);
    let new_main_commit = git(&fx.repo, &["rev-parse", "HEAD"]);
    assert_ne!(initial_main_commit, new_main_commit);

    // 1. Run wt create inside feat-1 worktree with --json
    let feat1_dir = fx.repo.parent().unwrap().join("origin-feat-1");
    let out_json = Command::new(env!("CARGO_BIN_EXE_wt"))
        .args(["create", "sub-feat", "--json"])
        .current_dir(&feat1_dir)
        .output()
        .expect("run wt binary");

    assert!(out_json.status.success());
    let stdout = String::from_utf8_lossy(&out_json.stdout);
    let json: serde_json::Value = serde_json::from_str(stdout.trim()).expect("parse json");
    let diags = json["diagnostics"].as_array().expect("diagnostics array");

    let base_diag = diags
        .iter()
        .find(|d| d["code"] == "BASE_BRANCH_MOVED")
        .expect("BASE_BRANCH_MOVED diagnostic must be present");

    let msg = base_diag["message"].as_str().expect("message string");
    assert!(
        msg.contains("main"),
        "message should mention base branch 'main': {msg}"
    );
    assert!(
        msg.contains(&initial_main_commit),
        "message should mention old commit: {msg}"
    );
    assert!(
        msg.contains(&new_main_commit),
        "message should mention new commit: {msg}"
    );

    // 2. Run wt create with human output and check stderr for warning
    let out_human = Command::new(env!("CARGO_BIN_EXE_wt"))
        .args(["create", "sub-feat-2"])
        .current_dir(&feat1_dir)
        .output()
        .expect("run wt binary");

    assert!(out_human.status.success());
    let stderr = String::from_utf8_lossy(&out_human.stderr);
    assert!(
        stderr.contains("warning: Base branch 'main' has moved")
            || stderr.contains("warning: base branch 'main' has moved"),
        "human output should surface warning to stderr: {stderr}"
    );

    // 3. From main repo, create a stacked branch --base feat-1 (where feat-1's base main has moved)
    let out_stacked = fx.wt(&["create", "feat-stacked", "--base", "feat-1", "--json"]);
    assert!(out_stacked.status.success());
    let stdout_stacked = String::from_utf8_lossy(&out_stacked.stdout);
    let json_stacked: serde_json::Value =
        serde_json::from_str(stdout_stacked.trim()).expect("parse json");
    let diags_stacked = json_stacked["diagnostics"]
        .as_array()
        .expect("diagnostics array");

    let stacked_diag = diags_stacked
        .iter()
        .find(|d| d["code"] == "BASE_BRANCH_MOVED")
        .expect("BASE_BRANCH_MOVED diagnostic for parent of feat-1 must be present");
    let msg_stacked = stacked_diag["message"].as_str().expect("message string");
    assert!(msg_stacked.contains("main"));
}

#[test]
fn no_diagnostic_when_base_has_not_moved() {
    let fx = Fixture::heavy_repo(10);
    git(&fx.repo, &["branch", "-M", "main"]);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let out = fx.wt(&["create", "feat-clean", "--base", "main", "--json"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let json: serde_json::Value = serde_json::from_str(stdout.trim()).expect("parse json");
    let diags = json["diagnostics"].as_array().expect("diagnostics array");

    assert!(
        !diags.iter().any(|d| d["code"] == "BASE_BRANCH_MOVED"),
        "should not emit BASE_BRANCH_MOVED when base has not moved"
    );
}

#[test]
fn remove_surfaces_base_movement_diagnostic() {
    let fx = Fixture::heavy_repo(10);
    git(&fx.repo, &["branch", "-M", "main"]);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let out_create = fx.wt(&["create", "feat-to-remove", "--base", "main"]);
    assert!(out_create.status.success());

    // Advance main
    fs::write(fx.repo.join("advance.txt"), "advance\n").unwrap();
    git(&fx.repo, &["add", "advance.txt"]);
    git(&fx.repo, &["commit", "-m", "advance"]);

    let out_remove = fx.wt(&["remove", "feat-to-remove", "--json"]);
    assert!(out_remove.status.success());
    let stdout = String::from_utf8_lossy(&out_remove.stdout);
    let json: serde_json::Value = serde_json::from_str(stdout.trim()).expect("parse json");
    let diags = json["diagnostics"].as_array().expect("diagnostics array");

    let base_diag = diags
        .iter()
        .find(|d| d["code"] == "BASE_BRANCH_MOVED")
        .expect("BASE_BRANCH_MOVED diagnostic should be present on remove");
    assert!(base_diag["message"].as_str().unwrap().contains("main"));
}
