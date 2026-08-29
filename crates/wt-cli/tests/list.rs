// Tests assert with unwrap/expect by design: a panic IS the failure
// signal under test, so the workspace restriction lints stay off here.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Integration tests for worktree discovery and disk accounting (`wt list` / `wt ls`) (ticket 02).

mod common;

use common::Fixture;
use std::fs;
use std::process::Command;

const HEAVY_FILES: usize = 50;

#[test]
fn list_and_ls_alias_parity_on_single_repo() {
    let fx = Fixture::heavy_repo(HEAVY_FILES);
    let store_dir = tempfile::tempdir().unwrap();

    let out_list = fx.wt_with_store(&["list"], store_dir.path());
    assert!(out_list.status.success());
    let stdout_list = String::from_utf8_lossy(&out_list.stdout);

    let out_ls = fx.wt_with_store(&["ls"], store_dir.path());
    assert!(out_ls.status.success());
    let stdout_ls = String::from_utf8_lossy(&out_ls.stdout);

    assert_eq!(stdout_list, stdout_ls);
    assert!(stdout_list.contains("BRANCH"));
    assert!(stdout_list.contains("PATH"));
    assert!(stdout_list.contains("HYDRATED"));
    assert!(stdout_list.contains("DISK SAVED"));
    assert!(stdout_list.contains("* main") || stdout_list.contains("*  main") || stdout_list.contains("*"));
}

#[test]
fn list_accurately_reports_hydrated_worktrees_and_disk_savings() {
    let fx = Fixture::heavy_repo(HEAVY_FILES);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();
    let store_dir = tempfile::tempdir().unwrap();

    // Create two worktrees
    let create1 = fx.wt_with_store(&["create", "feat-alpha"], store_dir.path());
    assert!(create1.status.success());

    let create2 = fx.wt_with_store(&["create", "feat-beta"], store_dir.path());
    assert!(create2.status.success());

    let out = fx.wt_with_store(&["list"], store_dir.path());
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);

    // Verify header and branches
    assert!(stdout.contains("feat-alpha"));
    assert!(stdout.contains("feat-beta"));
    assert!(stdout.contains(&format!("{} files", 2 * HEAVY_FILES)) || stdout.contains(&format!("{HEAVY_FILES} files")));
    assert!(stdout.contains("Total disk saved:"));
    assert!(stdout.contains("across 3 worktrees"));
}

#[test]
fn list_json_output_envelope_schema() {
    let fx = Fixture::heavy_repo(HEAVY_FILES);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();
    let store_dir = tempfile::tempdir().unwrap();

    let create_out = fx.wt_with_store(&["create", "json-test"], store_dir.path());
    assert!(create_out.status.success());

    let list_out = fx.wt_with_store(&["list", "--json"], store_dir.path());
    assert!(list_out.status.success());

    let stdout = String::from_utf8_lossy(&list_out.stdout);
    let lines: Vec<&str> = stdout.trim().lines().collect();
    assert_eq!(lines.len(), 1, "stdout must be single line NDJSON: {stdout}");

    let json: serde_json::Value = serde_json::from_str(lines[0]).expect("parse list json");
    assert_eq!(json["wt_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["command"], "list");
    assert_eq!(json["status"], "ok");
    assert!(json["diagnostics"].is_array());

    let data = &json["data"];
    assert_eq!(data["total_files_hydrated"], HEAVY_FILES);
    assert!(data["total_disk_saved"].as_u64().unwrap() > 0);

    let worktrees = data["worktrees"].as_array().expect("worktrees array");
    assert_eq!(worktrees.len(), 2);

    let main_wt = worktrees
        .iter()
        .find(|w| w["is_main"] == true)
        .expect("find main worktree");
    assert_eq!(main_wt["is_main"], true);
    assert_eq!(main_wt["is_active"], true);
    assert_eq!(main_wt["is_ephemeral"], false);
    assert_eq!(main_wt["files_hydrated"], 0);

    let feat_wt = worktrees
        .iter()
        .find(|w| w["branch"] == "json-test")
        .expect("find json-test worktree");
    assert_eq!(feat_wt["is_main"], false);
    assert_eq!(feat_wt["is_active"], false);
    assert_eq!(feat_wt["is_ephemeral"], false);
    assert_eq!(feat_wt["files_hydrated"], HEAVY_FILES);
    assert!(feat_wt["bytes_saved"].as_u64().unwrap() > 0);
    assert_eq!(feat_wt["bytes_hydrated"], feat_wt["bytes_saved"]);

    let dirs = feat_wt["hydrated_dirs"].as_array().expect("hydrated dirs");
    assert_eq!(dirs[0], "heavy");
}

#[test]
fn list_scratch_lease_reporting() {
    let fx = Fixture::heavy_repo(HEAVY_FILES);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();
    let store_dir = tempfile::tempdir().unwrap();

    let scratch_out = fx.wt_with_store(&["scratch", "--ttl", "1h", "temp-box"], store_dir.path());
    assert!(scratch_out.status.success());

    let list_json_out = fx.wt_with_store(&["list", "--json"], store_dir.path());
    assert!(list_json_out.status.success());

    let json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&list_json_out.stdout).trim()).unwrap();
    let worktrees = json["data"]["worktrees"].as_array().unwrap();

    let scratch_wt = worktrees
        .iter()
        .find(|w| w["branch"] == "temp-box")
        .expect("find scratch worktree");

    assert_eq!(scratch_wt["is_ephemeral"], true);
    let lease = &scratch_wt["lease"];
    assert!(!lease.is_null());
    assert_eq!(lease["pid_alive"], true);
    assert_eq!(lease["is_expired"], false);
    assert!(lease["ttl_remaining_secs"].as_u64().unwrap() > 0);
    assert!(lease["ttl_remaining_secs"].as_u64().unwrap() <= 3600);

    // Also verify human-readable list table contains lease/TTL information
    let list_human = fx.wt_with_store(&["list"], store_dir.path());
    assert!(list_human.status.success());
    let human_stdout = String::from_utf8_lossy(&list_human.stdout);
    assert!(human_stdout.contains("ttl:"));
    assert!(human_stdout.contains("temp-box"));
}

#[test]
fn list_active_marker_from_sub_worktree() {
    let fx = Fixture::heavy_repo(HEAVY_FILES);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();
    let store_dir = tempfile::tempdir().unwrap();

    let create_out = fx.wt_with_store(&["create", "sub-worktree"], store_dir.path());
    assert!(create_out.status.success());

    let sub_path = fx.repo.parent().unwrap().join("origin-sub-worktree");
    assert!(sub_path.exists());

    // Execute wt list from within sub_path
    let out = Command::new(env!("CARGO_BIN_EXE_wt"))
        .args(["list", "--json"])
        .env("WT_STORE", store_dir.path())
        .current_dir(&sub_path)
        .output()
        .expect("run wt binary from sub worktree");

    assert!(out.status.success());
    let json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).unwrap();

    let worktrees = json["data"]["worktrees"].as_array().unwrap();
    let main_wt = worktrees.iter().find(|w| w["is_main"] == true).unwrap();
    let sub_wt = worktrees
        .iter()
        .find(|w| w["branch"] == "sub-worktree")
        .unwrap();

    assert_eq!(main_wt["is_active"], false);
    assert_eq!(sub_wt["is_active"], true);
}

#[test]
fn list_maps_porcelain_metadata_for_detached_worktree() {
    let fx = Fixture::heavy_repo(HEAVY_FILES);
    let store_dir = tempfile::tempdir().unwrap();

    // A linked worktree created with raw git, in detached HEAD state:
    // the porcelain record carries no `branch` line, so the workspace
    // metadata mapping must render it as "(detached)".
    let detached_path = fx.repo.parent().unwrap().join("origin-detached-peek");
    let status = Command::new("git")
        .args(["worktree", "add", "--quiet", "--detach"])
        .arg(&detached_path)
        .arg("HEAD")
        .current_dir(&fx.repo)
        .status()
        .expect("git worktree add --detach");
    assert!(status.success());

    let out = fx.wt_with_store(&["list", "--json"], store_dir.path());
    assert!(out.status.success());
    let json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).unwrap();
    let worktrees = json["data"]["worktrees"].as_array().unwrap();
    assert_eq!(worktrees.len(), 2);

    let detached = worktrees
        .iter()
        .find(|w| w["path"].as_str().unwrap().ends_with("origin-detached-peek"))
        .expect("detached worktree listed");
    assert_eq!(detached["branch"], "(detached)");
    assert_eq!(detached["is_main"], false);
    assert_eq!(detached["is_active"], false);

    let main = worktrees.iter().find(|w| w["is_main"] == true).unwrap();
    assert_eq!(main["is_main"], true);
    assert_eq!(main["is_active"], true);
}
