// Tests assert with unwrap/expect by design: a panic IS the failure
// signal under test, so the workspace restriction lints stay off here.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Contract and unit tests for ticket 05:
//! Upfront filesystem capability probing and Linux copy acceleration.

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

use common::Fixture;
use wt_copy::{BackendKind, SourcePolicy, candidates, select_backend};
use wt_store::{DiskStore, FsCapabilities, probe_fs};

#[test]
fn store_initialization_probes_and_caches_filesystem_capabilities() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store_dir = temp.path().join("store");
    let store = DiskStore::open(&store_dir).expect("open store");

    let caps: FsCapabilities = store.fs_capabilities();
    assert!(caps.device_id > 0, "device_id must be non-zero");

    let direct = probe_fs(&store_dir).expect("probe_fs");
    assert_eq!(caps.device_id, direct.device_id);
    assert_eq!(caps.fs_type, direct.fs_type);
    assert_eq!(caps.reflink_capable, direct.reflink_capable);

    #[cfg(target_os = "linux")]
    {
        assert!(caps.fs_type > 0, "Linux statfs must report fs_type");
        if caps.reflink_capable {
            assert!(
                caps.fs_type == (libc::BTRFS_SUPER_MAGIC as u64)
                    || caps.fs_type == (libc::XFS_SUPER_MAGIC as u64),
                "reflink capable only on btrfs/XFS"
            );
        }
    }

    #[cfg(target_os = "macos")]
    {
        // APFS is reflink/clone capable
        assert!(
            caps.reflink_capable,
            "APFS on macOS should report reflink_capable"
        );
    }
}

#[test]
fn strategy_selection_prefers_acceleration_on_linux() {
    let temp = tempfile::tempdir().expect("tempdir");
    let caps = probe_fs(temp.path()).expect("probe_fs");

    let candidate_list = candidates();
    assert!(!candidate_list.is_empty());

    #[cfg(target_os = "linux")]
    {
        assert!(
            candidate_list
                .iter()
                .any(|b| b.kind() == BackendKind::CopyFileRange),
            "CopyFileRange backend must be a candidate on Linux"
        );
        assert!(
            candidate_list
                .iter()
                .any(|b| b.kind() == BackendKind::Reflink),
            "Reflink backend must be a candidate on Linux"
        );

        let picked = select_backend(temp.path(), SourcePolicy::Any);
        if caps.reflink_capable {
            assert_eq!(picked.kind(), BackendKind::Reflink);
        } else if caps.is_ext4() {
            assert_eq!(picked.kind(), BackendKind::CopyFileRange);
        }
    }

    #[cfg(target_os = "macos")]
    {
        let picked = select_backend(temp.path(), SourcePolicy::Any);
        if caps.reflink_capable {
            assert_eq!(picked.kind(), BackendKind::Clonefile);
        }
    }
}

#[test]
fn copy_file_range_accelerated_materialization_produces_identical_trees() {
    let fx = Fixture::heavy_repo(50);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let out = fx.wt(&["create", "accel", "--json"]);
    assert!(
        out.status.success(),
        "create failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.trim().lines().collect();
    assert_eq!(lines.len(), 1);

    let json: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(json["status"], "ok");
    assert_eq!(json["data"]["files_hydrated"], 50);

    let method = json["data"]["hydration_method"].as_str().unwrap();
    assert!(
        matches!(
            method,
            "clone" | "reflink" | "copy_file_range" | "byte_copy"
        ),
        "unexpected hydration method: {method}"
    );

    let hydrated = fx
        .repo
        .parent()
        .unwrap()
        .join("origin-accel/heavy/pkg00/nested/file-0.txt");
    assert!(hydrated.exists());
    assert_eq!(
        fs::read_to_string(&hydrated).unwrap(),
        "fake-heavy file 0 of 50\n"
    );
    let meta = fs::metadata(&hydrated).unwrap();
    assert_eq!(
        meta.permissions().mode() & 0o200,
        0o200,
        "file must be writable"
    );
}

#[test]
fn cross_device_or_fallback_copies_emit_diagnostic_warning_in_json() {
    let fx = Fixture::heavy_repo(20);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    // Force fallback via WT_NO_HARDLINK and inspect JSON diagnostics
    let out = Command::new(env!("CARGO_BIN_EXE_wt"))
        .args(["create", "cross-vol", "--json"])
        .env("WT_SNAPSHOTS", "0")
        .env("WT_NO_HARDLINK", "1")
        .current_dir(&fx.repo)
        .output()
        .expect("run wt binary");

    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.trim().lines().collect();
    assert_eq!(lines.len(), 1);

    let json: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(json["status"], "ok");
    assert_eq!(json["data"]["hydration_method"], "byte_copy");
    assert_eq!(json["data"]["bytes_shared_cow"], 0);
    assert!(json["data"]["bytes_copied"].as_u64().unwrap() > 0);

    let diags = json["diagnostics"].as_array().unwrap();
    assert!(
        diags
            .iter()
            .any(|d| d["code"] == "CROSS_DEVICE_COPY_DEGRADATION"),
        "expected CROSS_DEVICE_COPY_DEGRADATION diagnostic warning in:\n{stdout}"
    );
}

#[test]
fn sequential_buffered_copy_preserves_executable_mode_and_content() {
    let temp = tempfile::tempdir().expect("tempdir");
    let src = temp.path().join("script.sh");
    let dest = temp.path().join("dest_script.sh");

    let script_content = "#!/bin/sh\necho 'hello from accelerated sequential copy'\n";
    fs::write(&src, script_content).expect("write script");
    fs::set_permissions(&src, fs::Permissions::from_mode(0o755)).expect("set mode");

    let copied_bytes = wt_copy::buffered_copy_file(&src, &dest).expect("buffered_copy_file");
    assert_eq!(copied_bytes, script_content.len() as u64);
    assert_eq!(
        fs::read_to_string(&dest).expect("read dest"),
        script_content
    );

    let dest_mode = fs::metadata(&dest).expect("metadata").permissions().mode();
    assert_eq!(dest_mode & 0o7777, 0o755, "mode must be 0755");
}
