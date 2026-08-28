// Tests assert with unwrap/expect by design: a panic IS the failure
// signal under test, so the workspace restriction lints stay off here.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Integration tests verifying versioned JSON output envelope across
//! all CLI commands (ticket 01).

mod common;

use std::fs;
use common::Fixture;

const HEAVY_FILES: usize = 100;

#[test]
fn create_json_emits_valid_envelope_and_suppresses_human_output() {
    let fx = Fixture::heavy_repo(HEAVY_FILES);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let out = fx.wt(&["create", "demo", "--json"]);
    assert!(
        out.status.success(),
        "wt create --json failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.trim().lines().collect();
    assert_eq!(lines.len(), 1, "stdout must contain exactly 1 line of NDJSON: {stdout}");

    let json: serde_json::Value = serde_json::from_str(lines[0]).expect("parse json");
    assert_eq!(json["wt_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["command"], "create");
    assert_eq!(json["status"], "ok");
    assert!(json["diagnostics"].is_array());

    let data = &json["data"];
    assert_eq!(data["branch"], "demo");
    assert!(data["worktree_path"].as_str().unwrap().contains("origin-demo"));
    assert!(data["duration_ms"].is_number());
    assert!(data["hydration_method"].is_string());
    assert!(data["bytes_shared_cow"].is_number());
    assert!(data["bytes_copied"].is_number());
    assert_eq!(data["files_hydrated"], HEAVY_FILES);

    // Ensure human-readable progress lines are suppressed from stdout
    assert!(!stdout.contains("created worktree"));
    assert!(!stdout.contains("hydration complete"));
    assert!(!stdout.contains("file(s) through the store"));
}

#[test]
fn global_json_flag_parses_in_all_positions() {
    let fx = Fixture::heavy_repo(10);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    // Before subcommand: wt --json create demo1
    let out1 = fx.wt(&["--json", "create", "demo1"]);
    assert!(out1.status.success());
    let val1: serde_json::Value = serde_json::from_str(String::from_utf8_lossy(&out1.stdout).trim()).unwrap();
    assert_eq!(val1["status"], "ok");
    assert_eq!(val1["data"]["branch"], "demo1");

    // After subcommand: wt create --json demo2
    let out2 = fx.wt(&["create", "--json", "demo2"]);
    assert!(out2.status.success());
    let val2: serde_json::Value = serde_json::from_str(String::from_utf8_lossy(&out2.stdout).trim()).unwrap();
    assert_eq!(val2["status"], "ok");
    assert_eq!(val2["data"]["branch"], "demo2");

    // At end: wt create demo3 --json
    let out3 = fx.wt(&["create", "demo3", "--json"]);
    assert!(out3.status.success());
    let val3: serde_json::Value = serde_json::from_str(String::from_utf8_lossy(&out3.stdout).trim()).unwrap();
    assert_eq!(val3["status"], "ok");
    assert_eq!(val3["data"]["branch"], "demo3");
}

#[test]
fn create_json_with_nothing_to_hydrate() {
    let fx = Fixture::heavy_repo(1);
    fs::remove_dir_all(fx.repo.join("heavy")).unwrap();

    let out = fx.wt(&["create", "empty", "--json"]);
    assert!(out.status.success());

    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.trim().lines().collect();
    assert_eq!(lines.len(), 1);

    let json: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(json["command"], "create");
    assert_eq!(json["status"], "ok");
    assert_eq!(json["data"]["files_hydrated"], 0);
    assert_eq!(json["data"]["hydration_method"], "none");
    assert!(!stdout.contains("nothing to hydrate"));
}

#[test]
fn remove_json_emits_valid_envelope() {
    let fx = Fixture::heavy_repo(10);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let create_out = fx.wt(&["create", "to-remove", "--json"]);
    assert!(create_out.status.success());

    let remove_out = fx.wt(&["remove", "to-remove", "--json"]);
    assert!(remove_out.status.success());

    let stdout = String::from_utf8_lossy(&remove_out.stdout);
    let lines: Vec<&str> = stdout.trim().lines().collect();
    assert_eq!(lines.len(), 1);

    let json: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(json["wt_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["command"], "remove");
    assert_eq!(json["status"], "ok");

    let data = &json["data"];
    assert_eq!(data["branch"], "to-remove");
    assert!(data["worktree_path"].as_str().unwrap().contains("origin-to-remove"));
    assert!(data["references_released"].is_number());
    assert!(data["mirror_removed"].is_boolean());

    assert!(!stdout.contains("removed worktree"));
}

#[test]
fn sweep_json_emits_valid_envelope() {
    let store_dir = tempfile::tempdir().unwrap();
    let fx = Fixture::heavy_repo(10);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let _ = fx.wt_with_store(&["create", "sweep-me"], store_dir.path());
    let _ = fx.wt_with_store(&["remove", "sweep-me"], store_dir.path());

    // Legacy sweep --json
    let sweep_out = fx.wt_with_store(&["sweep", "--age", "0s", "--json"], store_dir.path());
    assert!(sweep_out.status.success());

    let stdout = String::from_utf8_lossy(&sweep_out.stdout);
    let lines: Vec<&str> = stdout.trim().lines().collect();
    assert_eq!(lines.len(), 1);

    let json: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(json["command"], "sweep");
    assert_eq!(json["status"], "ok");
    assert_eq!(json["data"]["mode"], "legacy");
    assert!(json["data"]["examined"].is_number());
    assert!(json["data"]["reclaimed"].is_number());
    assert!(!stdout.contains("swept store"));

    // Migrate to mark-sweep and sweep --json
    let mig_out = fx.wt_with_store(&["store", "migrate", "--activate-mark-sweep", "--json"], store_dir.path());
    assert!(mig_out.status.success());
    let mig_json: serde_json::Value = serde_json::from_str(String::from_utf8_lossy(&mig_out.stdout).trim()).unwrap();
    assert_eq!(mig_json["command"], "store");
    assert_eq!(mig_json["status"], "ok");
    assert_eq!(mig_json["data"]["gc_mode"], "mark-sweep");

    let sweep2_out = fx.wt_with_store(&["sweep", "--age", "0s", "--json"], store_dir.path());
    assert!(sweep2_out.status.success());
    let sweep2_json: serde_json::Value = serde_json::from_str(String::from_utf8_lossy(&sweep2_out.stdout).trim()).unwrap();
    assert_eq!(sweep2_json["command"], "sweep");
    assert_eq!(sweep2_json["status"], "ok");
    assert_eq!(sweep2_json["data"]["mode"], "mark-sweep");
    assert!(sweep2_json["data"]["mirrors_removed"].is_number());
    assert!(sweep2_json["data"]["snapshot_dirs_removed"].is_number());
    assert!(sweep2_json["data"]["snapshot_cap_evicted"].is_number());
}

#[test]
fn scrub_json_emits_valid_envelope() {
    let store_dir = tempfile::tempdir().unwrap();
    let fx = Fixture::heavy_repo(10);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let _ = fx.wt_with_store(&["create", "scrub-demo"], store_dir.path());

    // Dry run
    let scrub_dry = fx.wt_with_store(&["scrub", "--dry-run", "--json"], store_dir.path());
    assert!(scrub_dry.status.success());
    let dry_json: serde_json::Value = serde_json::from_str(String::from_utf8_lossy(&scrub_dry.stdout).trim()).unwrap();
    assert_eq!(dry_json["command"], "scrub");
    assert_eq!(dry_json["status"], "ok");
    assert_eq!(dry_json["data"]["dry_run"], true);
    assert!(dry_json["data"]["scanned"].as_u64().unwrap() > 0);
    assert_eq!(dry_json["data"]["corrupt"], serde_json::json!([]));
    assert_eq!(dry_json["data"]["deleted"], 0);

    // Non dry run
    let scrub_real = fx.wt_with_store(&["scrub", "--json"], store_dir.path());
    assert!(scrub_real.status.success());
    let real_json: serde_json::Value = serde_json::from_str(String::from_utf8_lossy(&scrub_real.stdout).trim()).unwrap();
    assert_eq!(real_json["command"], "scrub");
    assert_eq!(real_json["status"], "ok");
    assert_eq!(real_json["data"]["dry_run"], false);
    assert!(real_json["data"]["scanned"].as_u64().unwrap() > 0);
}

#[test]
fn json_error_envelope_on_failure() {
    let fx = Fixture::heavy_repo(5);
    let taken = fx.repo.parent().unwrap().join("origin-taken");
    fs::create_dir_all(&taken).unwrap();

    // Creation failure due to existing directory
    let out = fx.wt(&["create", "taken", "--json"]);
    assert!(!out.status.success(), "expected failure for existing directory");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.trim().lines().collect();
    assert_eq!(lines.len(), 1, "expected single line error json on stdout: {stdout}");

    let json: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(json["wt_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["command"], "create");
    assert_eq!(json["status"], "error");
    assert!(json["data"].is_null());
    assert!(json["diagnostics"].is_array());
    assert_eq!(json["diagnostics"][0]["code"], "ERROR");
    assert!(json["diagnostics"][0]["message"].as_str().unwrap().contains("already exists"));

    // Missing manifest failure
    let out2 = fx.wt(&["create", "missing-manifest-test", "--manifest", "nonexistent.wtinclude", "--json"]);
    assert!(!out2.status.success());
    let json2: serde_json::Value = serde_json::from_str(String::from_utf8_lossy(&out2.stdout).trim()).unwrap();
    assert_eq!(json2["status"], "error");
    assert_eq!(json2["command"], "create");
}
