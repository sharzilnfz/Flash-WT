// Tests assert with unwrap/expect by design: a panic IS the failure
// signal under test, so the workspace restriction lints stay off here.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Integration tests for `wt demo` and `wt test-drive` (ticket 01).

mod common;

use std::process::Command;
use tempfile::tempdir;

#[test]
fn demo_terminal_output_renders_scorecard_and_completes_successfully() {
    let store_dir = tempdir().unwrap();
    let isolated_cwd = tempdir().unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_wt"))
        .args(["demo"])
        .env("WT_STORE", store_dir.path())
        .current_dir(isolated_cwd.path())
        .output()
        .expect("run wt demo");

    assert!(
        out.status.success(),
        "wt demo failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("wt demo: Zero-Setup End-to-End Performance Test Drive"));
    assert!(stdout.contains("Step 1/5: Synthesizing realistic fixture..."));
    assert!(stdout.contains("10,000 files across 100 packages"));
    assert!(stdout.contains("Step 2/5: Benchmarking standard filesystem recursive copy..."));
    assert!(stdout.contains("Step 3/5: Benchmarking wt Copy-on-Write hydration..."));
    assert!(stdout.contains("Step 4/5: Verifying Copy-on-Write mutation isolation..."));
    assert!(stdout.contains("Step 5/5: Cleaning up benchmark artifacts..."));
    assert!(stdout.contains("PERFORMANCE SCORECARD"));
    assert!(stdout.contains("Standard Copy"));
    assert!(stdout.contains("wt Hydration"));
    assert!(stdout.contains("Mutation Isolation     : VERIFIED"));
    assert!(stdout.contains("Status                 : ALL CHECKS PASSED (5/5)"));
}

#[test]
fn demo_json_emits_valid_ndjson_envelope_with_complete_metrics() {
    let store_dir = tempdir().unwrap();
    let isolated_cwd = tempdir().unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_wt"))
        .args(["demo", "--json"])
        .env("WT_STORE", store_dir.path())
        .current_dir(isolated_cwd.path())
        .output()
        .expect("run wt demo --json");

    assert!(
        out.status.success(),
        "wt demo --json failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.trim().lines().collect();
    assert_eq!(
        lines.len(),
        1,
        "stdout must contain exactly 1 line of NDJSON: {stdout}"
    );

    let json: serde_json::Value = serde_json::from_str(lines[0]).expect("parse json");
    assert_eq!(json["wt_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["command"], "demo");
    assert_eq!(json["status"], "ok");
    assert!(json["diagnostics"].is_array());

    let data = &json["data"];
    assert_eq!(data["files_count"], 10000);
    assert!(data["total_bytes"].as_u64().unwrap() > 0);
    assert!(data["baseline_copy_duration_ms"].is_number());
    assert!(data["baseline_copy_bytes"].as_u64().unwrap() > 0);
    assert!(data["wt_hydration_duration_ms"].is_number());
    assert!(data["speedup_ratio"].as_f64().unwrap() > 0.0);
    assert!(data["hydration_method"].is_string());
    assert!(data["bytes_shared_cow"].is_number());
    assert!(data["bytes_copied"].is_number());
    assert!(data["space_savings_bytes"].as_u64().unwrap() > 0);
    assert_eq!(data["isolation_verified"], true);
    assert_eq!(data["cleaned_up"], true);
    assert!(data["total_duration_ms"].as_u64().unwrap() > 0);

    // Ensure terminal prose is suppressed in --json mode
    assert!(!stdout.contains("PERFORMANCE SCORECARD"));
    assert!(!stdout.contains("Step 1/5"));
}

#[test]
fn test_drive_alias_works_identically_with_json_envelope() {
    let store_dir = tempdir().unwrap();
    let isolated_cwd = tempdir().unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_wt"))
        .args(["test-drive", "--json"])
        .env("WT_STORE", store_dir.path())
        .current_dir(isolated_cwd.path())
        .output()
        .expect("run wt test-drive --json");

    assert!(
        out.status.success(),
        "wt test-drive --json failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.trim().lines().collect();
    assert_eq!(lines.len(), 1);

    let json: serde_json::Value = serde_json::from_str(lines[0]).expect("parse json");
    assert_eq!(json["command"], "demo");
    assert_eq!(json["status"], "ok");

    let data = &json["data"];
    assert_eq!(data["files_count"], 10000);
    assert_eq!(data["isolation_verified"], true);
    assert_eq!(data["cleaned_up"], true);
}

#[test]
fn demo_runs_outside_any_git_repository() {
    let store_dir = tempdir().unwrap();
    let non_git_dir = tempdir().unwrap();

    // Verify non_git_dir is indeed not a git repository
    let git_check = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(non_git_dir.path())
        .output()
        .expect("check git");
    assert!(!git_check.status.success());

    // wt demo must succeed even with zero git setup
    let out = Command::new(env!("CARGO_BIN_EXE_wt"))
        .args(["demo", "--json"])
        .env("WT_STORE", store_dir.path())
        .current_dir(non_git_dir.path())
        .output()
        .expect("run wt demo in non-git directory");

    assert!(
        out.status.success(),
        "wt demo failed in non-git directory: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let json: serde_json::Value = serde_json::from_str(stdout.trim()).expect("parse json");
    assert_eq!(json["status"], "ok");
    assert_eq!(json["data"]["files_count"], 10000);
}
