// Tests assert with unwrap/expect by design: a panic IS the failure
// signal under test, so the workspace restriction lints stay off here.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Integration tests for `wt scratch` and `wt isolate` ephemeral sandboxes
//! with lease persistence (ticket 03).

mod common;

use common::Fixture;
use std::fs;
use std::time::SystemTime;
use wt_store::{WorktreeLease, lease_path};

const HEAVY_FILES: usize = 20;

#[test]
fn bare_scratch_generates_worktree_and_persists_lease() {
    let store_dir = tempfile::tempdir().unwrap();
    let fx = Fixture::heavy_repo(HEAVY_FILES);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let out = fx.wt_with_store(&["scratch", "--json"], store_dir.path());
    assert!(
        out.status.success(),
        "wt scratch --json failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.trim().lines().collect();
    assert_eq!(
        lines.len(),
        1,
        "stdout must be single-line NDJSON: {stdout}"
    );

    let json: serde_json::Value = serde_json::from_str(lines[0]).expect("parse json");
    assert_eq!(json["command"], "scratch");
    assert_eq!(json["status"], "ok");

    let data = &json["data"];
    let branch = data["branch"].as_str().unwrap();
    let worktree_path = std::path::PathBuf::from(data["worktree_path"].as_str().unwrap());
    let lease_id = data["lease_id"].as_str().unwrap();
    let lease_file = std::path::PathBuf::from(data["lease_file"].as_str().unwrap());
    let expires_at = data["expires_at"].as_u64().unwrap();

    assert!(branch.starts_with("scratch-"));
    assert!(
        worktree_path.exists(),
        "worktree directory must exist on disk"
    );
    assert!(
        worktree_path.join("heavy").exists(),
        "heavy dir must be hydrated"
    );
    assert!(
        lease_file.exists(),
        "lease file must exist on disk: {lease_file:?}"
    );

    // Verify lease file contents
    let lease_text = fs::read_to_string(&lease_file).unwrap();
    let parsed_lease = WorktreeLease::parse(lease_id, &lease_text).expect("parse lease file");
    assert_eq!(parsed_lease.id, lease_id);
    assert_eq!(parsed_lease.expires_at, expires_at);
    assert!(parsed_lease.pid > 0);
    assert!(parsed_lease.start_time > 0);

    let now_secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    assert!(expires_at > now_secs);

    // Clean up
    let _ = fx.wt_with_store(&["remove", branch], store_dir.path());
}

#[test]
fn scratch_run_executes_child_command_and_cleans_up_on_clean_exit() {
    let store_dir = tempfile::tempdir().unwrap();
    let fx = Fixture::heavy_repo(HEAVY_FILES);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let out = fx.wt_with_store(
        &["scratch", "--run", "echo 'hello sandbox' && test -d heavy"],
        store_dir.path(),
    );
    assert!(
        out.status.success(),
        "wt scratch --run failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Check stdout in non-json mode forwards output
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("hello sandbox"));

    // Verify no leftover lease files in store
    let leases = wt_store::read_leases(store_dir.path());
    assert_eq!(
        leases.len(),
        0,
        "lease file must be removed after clean exit"
    );
}

#[test]
fn scratch_run_forwards_exit_codes_and_cleans_up() {
    let store_dir = tempfile::tempdir().unwrap();
    let fx = Fixture::heavy_repo(HEAVY_FILES);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let out = fx.wt_with_store(&["scratch", "--run", "exit 42"], store_dir.path());
    assert_eq!(
        out.status.code(),
        Some(42),
        "scratch must forward child command exit code"
    );

    // Verify lease was cleaned up even on non-zero exit code
    let leases = wt_store::read_leases(store_dir.path());
    assert_eq!(leases.len(), 0, "lease must be cleaned up on non-zero exit");
}

#[test]
fn isolate_command_alias_works_identically() {
    let store_dir = tempfile::tempdir().unwrap();
    let fx = Fixture::heavy_repo(10);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let out = fx.wt_with_store(
        &["isolate", "--run", "test -d heavy", "--json"],
        store_dir.path(),
    );
    assert!(out.status.success());

    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.trim().lines().collect();
    assert_eq!(lines.len(), 1);

    let json: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(json["command"], "scratch");
    assert_eq!(json["status"], "ok");
    assert_eq!(json["data"]["command"], "test -d heavy");
    assert_eq!(json["data"]["exit_code"], 0);
    assert_eq!(json["data"]["cleaned_up"], true);

    let leases = wt_store::read_leases(store_dir.path());
    assert_eq!(leases.len(), 0);
}

#[test]
fn scratch_with_ttl_persists_custom_expiration() {
    let store_dir = tempfile::tempdir().unwrap();
    let fx = Fixture::heavy_repo(5);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let now_secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let out = fx.wt_with_store(&["scratch", "--ttl", "2h", "--json"], store_dir.path());
    assert!(out.status.success());

    let json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).unwrap();
    let branch = json["data"]["branch"].as_str().unwrap();
    let expires_at = json["data"]["expires_at"].as_u64().unwrap();

    // 2 hours = 7200 seconds
    assert!(expires_at >= now_secs + 7150 && expires_at <= now_secs + 7250);

    let lease_path = lease_path(store_dir.path(), json["data"]["lease_id"].as_str().unwrap());
    assert!(lease_path.exists());

    let _ = fx.wt_with_store(&["remove", branch], store_dir.path());
}

#[test]
fn scratch_with_custom_name() {
    let store_dir = tempfile::tempdir().unwrap();
    let fx = Fixture::heavy_repo(5);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let out = fx.wt_with_store(&["scratch", "custom-sandbox", "--json"], store_dir.path());
    assert!(out.status.success());

    let json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).unwrap();
    assert_eq!(json["data"]["branch"], "custom-sandbox");
    assert_eq!(json["data"]["lease_id"], "custom-sandbox");

    let lease_path = lease_path(store_dir.path(), "custom-sandbox");
    assert!(lease_path.exists());

    let _ = fx.wt_with_store(&["remove", "custom-sandbox"], store_dir.path());
}

#[test]
fn scratch_run_json_output_envelope() {
    let store_dir = tempfile::tempdir().unwrap();
    let fx = Fixture::heavy_repo(5);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let out = fx.wt_with_store(
        &["scratch", "--run", "echo 'in sandbox' && exit 0", "--json"],
        store_dir.path(),
    );
    assert!(out.status.success());

    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.trim().lines().collect();
    assert_eq!(lines.len(), 1, "stdout must be exactly single-line NDJSON");

    let json: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(json["command"], "scratch");
    assert_eq!(json["status"], "ok");
    assert_eq!(json["data"]["command"], "echo 'in sandbox' && exit 0");
    assert_eq!(json["data"]["exit_code"], 0);
    assert_eq!(json["data"]["cleaned_up"], true);
}
