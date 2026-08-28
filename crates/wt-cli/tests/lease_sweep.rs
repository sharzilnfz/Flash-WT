// Tests assert with unwrap/expect by design: a panic IS the failure
// signal under test, so the workspace restriction lints stay off here.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Integration tests for automated lease sweeping for dead or expired sandbox worktrees (ticket 04).

mod common;

use common::Fixture;
use std::fs;
use std::sync::Arc;
use std::time::SystemTime;
use wt_store::{WorktreeLease, current_process_start_time, lease_path, publish_lease, read_leases};

const HEAVY_FILES: usize = 20;

#[test]
fn dead_process_lease_is_reaped_by_sweep() {
    let store_dir = tempfile::tempdir().unwrap();
    let fx = Fixture::heavy_repo(HEAVY_FILES);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    // 1. Create a bare scratch worktree
    let out = fx.wt_with_store(&["scratch", "--json"], store_dir.path());
    assert!(out.status.success());
    let json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).unwrap();
    let branch = json["data"]["branch"].as_str().unwrap().to_string();
    let lease_id = json["data"]["lease_id"].as_str().unwrap().to_string();
    let worktree_path = std::path::PathBuf::from(json["data"]["worktree_path"].as_str().unwrap());
    let lease_file = std::path::PathBuf::from(json["data"]["lease_file"].as_str().unwrap());

    assert!(worktree_path.exists());
    assert!(lease_file.exists());

    // 2. Overwrite the lease file with a dead PID (999_999_999)
    let git_dir = fx.repo.join(".git").join("worktrees").join(&branch);
    let dead_lease = WorktreeLease::new(
        &lease_id,
        worktree_path.clone(),
        git_dir.clone(),
        999_999_999,
        12345,
        1900000000,
    );
    publish_lease(store_dir.path(), &dead_lease).unwrap();

    // 3. Run wt sweep
    let sweep_out = fx.wt_with_store(&["sweep", "--age", "0s", "--json"], store_dir.path());
    assert!(sweep_out.status.success());
    let sweep_json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&sweep_out.stdout).trim()).unwrap();
    assert_eq!(sweep_json["command"], "sweep");
    assert_eq!(sweep_json["status"], "ok");
    assert_eq!(sweep_json["data"]["leases_examined"], 1);
    assert_eq!(sweep_json["data"]["leases_reclaimed"], 1);
    assert!(
        sweep_json["data"]["lease_bytes_reclaimed"]
            .as_u64()
            .unwrap()
            > 0
    );

    // 4. Verify everything is reaped cleanly
    assert!(!lease_file.exists(), "lease file must be deleted");
    assert!(
        !worktree_path.exists(),
        "worktree directory must be deleted"
    );
    assert!(!git_dir.exists(), "gitdir in .git/worktrees must be pruned");

    // Git branch must be deleted
    let branch_check = std::process::Command::new("git")
        .args(["rev-parse", "--verify", &branch])
        .current_dir(&fx.repo)
        .output()
        .unwrap();
    assert!(!branch_check.status.success(), "branch should be deleted");

    // No leases remaining
    assert_eq!(read_leases(store_dir.path()).len(), 0);
}

#[test]
fn shifted_start_time_pid_reuse_is_reaped() {
    let store_dir = tempfile::tempdir().unwrap();
    let fx = Fixture::heavy_repo(HEAVY_FILES);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let out = fx.wt_with_store(&["scratch", "--json"], store_dir.path());
    assert!(out.status.success());
    let json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).unwrap();
    let branch = json["data"]["branch"].as_str().unwrap().to_string();
    let lease_id = json["data"]["lease_id"].as_str().unwrap().to_string();
    let worktree_path = std::path::PathBuf::from(json["data"]["worktree_path"].as_str().unwrap());

    // Current PID (alive), but start time = 1 (wrong start time fingerprint simulating PID reuse)
    let git_dir = fx.repo.join(".git").join("worktrees").join(&branch);
    let my_pid = std::process::id();
    let reused_pid_lease = WorktreeLease::new(
        &lease_id,
        worktree_path.clone(),
        git_dir.clone(),
        my_pid,
        1, // shifted start time
        1900000000,
    );
    publish_lease(store_dir.path(), &reused_pid_lease).unwrap();

    let sweep_out = fx.wt_with_store(&["sweep", "--age", "0s", "--json"], store_dir.path());
    assert!(sweep_out.status.success());
    let sweep_json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&sweep_out.stdout).trim()).unwrap();
    assert_eq!(sweep_json["data"]["leases_reclaimed"], 1);

    assert!(!worktree_path.exists());
    assert_eq!(read_leases(store_dir.path()).len(), 0);
}

#[test]
fn expired_ttl_lease_is_reaped_even_with_live_pid() {
    let store_dir = tempfile::tempdir().unwrap();
    let fx = Fixture::heavy_repo(HEAVY_FILES);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let out = fx.wt_with_store(&["scratch", "--json"], store_dir.path());
    assert!(out.status.success());
    let json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).unwrap();
    let branch = json["data"]["branch"].as_str().unwrap().to_string();
    let lease_id = json["data"]["lease_id"].as_str().unwrap().to_string();
    let worktree_path = std::path::PathBuf::from(json["data"]["worktree_path"].as_str().unwrap());

    let now_secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // Alive PID, valid start time, but expired timestamp in the past
    let git_dir = fx.repo.join(".git").join("worktrees").join(&branch);
    let expired_lease = WorktreeLease::new(
        &lease_id,
        worktree_path.clone(),
        git_dir.clone(),
        std::process::id(),
        current_process_start_time(),
        now_secs.saturating_sub(60), // expired 60s ago
    );
    publish_lease(store_dir.path(), &expired_lease).unwrap();

    let sweep_out = fx.wt_with_store(&["sweep", "--age", "0s", "--json"], store_dir.path());
    assert!(sweep_out.status.success());
    let sweep_json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&sweep_out.stdout).trim()).unwrap();
    assert_eq!(sweep_json["data"]["leases_reclaimed"], 1);

    assert!(!worktree_path.exists());
    assert_eq!(read_leases(store_dir.path()).len(), 0);
}

#[test]
fn active_lease_is_protected_from_sweep() {
    let store_dir = tempfile::tempdir().unwrap();
    let fx = Fixture::heavy_repo(HEAVY_FILES);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let out = fx.wt_with_store(&["scratch", "--json"], store_dir.path());
    assert!(out.status.success());
    let json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).unwrap();
    let branch = json["data"]["branch"].as_str().unwrap().to_string();
    let worktree_path = std::path::PathBuf::from(json["data"]["worktree_path"].as_str().unwrap());

    // Sweep while lease is active
    let sweep_out = fx.wt_with_store(&["sweep", "--age", "0s", "--json"], store_dir.path());
    assert!(sweep_out.status.success());
    let sweep_json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&sweep_out.stdout).trim()).unwrap();
    assert_eq!(sweep_json["data"]["leases_examined"], 1);
    assert_eq!(sweep_json["data"]["leases_reclaimed"], 0);
    assert_eq!(sweep_json["data"]["lease_bytes_reclaimed"], 0);

    // Active worktree and lease still exist
    assert!(worktree_path.exists());
    assert_eq!(read_leases(store_dir.path()).len(), 1);

    // Clean up
    let _ = fx.wt_with_store(&["remove", &branch], store_dir.path());
}

#[test]
fn sweep_reclaims_unreferenced_blobs_of_reaped_lease() {
    let store_dir = tempfile::tempdir().unwrap();
    let fx = Fixture::heavy_repo(HEAVY_FILES);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    // Migrate to mark-sweep
    let _ = fx.wt_with_store(
        &["store", "migrate", "--activate-mark-sweep"],
        store_dir.path(),
    );

    // Create scratch worktree
    let out = fx.wt_with_store(&["scratch", "--json"], store_dir.path());
    assert!(out.status.success());
    let json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).unwrap();
    let branch = json["data"]["branch"].as_str().unwrap().to_string();
    let lease_id = json["data"]["lease_id"].as_str().unwrap().to_string();
    let worktree_path = std::path::PathBuf::from(json["data"]["worktree_path"].as_str().unwrap());
    let git_dir = fx.repo.join(".git").join("worktrees").join(&branch);

    // Simulate dead PID
    let dead_lease = WorktreeLease::new(
        &lease_id,
        worktree_path.clone(),
        git_dir.clone(),
        999_999_999,
        0,
        1900000000,
    );
    publish_lease(store_dir.path(), &dead_lease).unwrap();

    // Sweep with age 0s
    let sweep_out = fx.wt_with_store(&["sweep", "--age", "0s", "--json"], store_dir.path());
    assert!(sweep_out.status.success());
    let sweep_json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&sweep_out.stdout).trim()).unwrap();

    assert_eq!(sweep_json["data"]["leases_reclaimed"], 1);
    assert_eq!(sweep_json["data"]["mirrors_removed"], 1);
    assert!(sweep_json["data"]["reclaimed"].as_u64().unwrap() >= HEAVY_FILES as u64);
}

#[test]
fn concurrent_lease_sweeping_stress() {
    let store_dir = tempfile::tempdir().unwrap();
    let store_path = Arc::new(store_dir.path().to_path_buf());

    let mut handles = Vec::new();
    for i in 0..6 {
        let store_p = Arc::clone(&store_path);
        handles.push(std::thread::spawn(move || {
            let fx = Fixture::heavy_repo(10);
            fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();
            let name = format!("concurrent-dead-{i}");
            let out = fx.wt_with_store(&["scratch", &name, "--json"], &store_p);
            assert!(out.status.success());

            // Set dead PID
            let lease_p = lease_path(&store_p, &name);
            let text = fs::read_to_string(&lease_p).unwrap();
            let mut lease = WorktreeLease::parse(&name, &text).unwrap();
            lease.pid = 999_999_000 + i;
            publish_lease(&store_p, &lease).unwrap();

            // Run sweep concurrently
            let sweep = fx.wt_with_store(&["sweep", "--age", "0s", "--json"], &store_p);
            assert!(
                sweep.status.success(),
                "sweep failed: stdout={}, stderr={}",
                String::from_utf8_lossy(&sweep.stdout),
                String::from_utf8_lossy(&sweep.stderr)
            );
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    // Final sweep cleans up anything left
    let fx_final = Fixture::heavy_repo(5);
    let final_sweep =
        fx_final.wt_with_store(&["sweep", "--age", "0s", "--json"], store_path.as_path());
    assert!(final_sweep.status.success());
    assert_eq!(read_leases(store_path.as_path()).len(), 0);
}
