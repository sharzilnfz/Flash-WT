use std::fs;
use std::path::PathBuf;
use std::process::Command;

use crate::common::Fixture;

const HEAVY_FILES: usize = 100;

#[test]
fn create_json_emits_valid_envelope_and_suppresses_human_output() {
    let fx = Fixture::heavy_repo(HEAVY_FILES);
    fs::write(fx.repo.join(".flashwtinclude"), "heavy/\n").unwrap();

    let out = fx.flashwt(&["create", "demo", "--json"]);
    assert!(
        out.status.success(),
        "flashwt create --json failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.trim().lines().collect();
    assert_eq!(lines.len(), 1);

    let json: serde_json::Value = serde_json::from_str(lines[0]).expect("parse json");
    assert_eq!(json["flashwt_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["command"], "create");
    assert_eq!(json["status"], "ok");
    assert!(json["diagnostics"].is_array());

    let data = &json["data"];
    assert_eq!(data["branch"], "demo");
    assert!(
        data["worktree_path"]
            .as_str()
            .unwrap()
            .contains("origin-demo")
    );
    assert!(data["duration_ms"].is_number());
    assert!(data["hydration_method"].is_string());
    assert!(data["bytes_shared_cow"].is_number());
    assert!(data["bytes_copied"].is_number());
    assert_eq!(data["files_hydrated"], HEAVY_FILES);

    assert!(!stdout.contains("created worktree"));
    assert!(!stdout.contains("hydration complete"));
    assert!(!stdout.contains("file(s) through the store"));
}

#[test]
fn global_json_flag_parses_in_all_positions() {
    let fx = Fixture::heavy_repo(10);
    fs::write(fx.repo.join(".flashwtinclude"), "heavy/\n").unwrap();

    let out1 = fx.flashwt(&["--json", "create", "demo1"]);
    assert!(out1.status.success());
    let val1: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&out1.stdout).trim()).unwrap();
    assert_eq!(val1["status"], "ok");
    assert_eq!(val1["data"]["branch"], "demo1");

    let out2 = fx.flashwt(&["create", "--json", "demo2"]);
    assert!(out2.status.success());
    let val2: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&out2.stdout).trim()).unwrap();
    assert_eq!(val2["status"], "ok");
    assert_eq!(val2["data"]["branch"], "demo2");

    let out3 = fx.flashwt(&["create", "demo3", "--json"]);
    assert!(out3.status.success());
    let val3: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&out3.stdout).trim()).unwrap();
    assert_eq!(val3["status"], "ok");
    assert_eq!(val3["data"]["branch"], "demo3");
}

#[test]
fn create_json_with_nothing_to_hydrate() {
    let fx = Fixture::heavy_repo(1);
    fs::remove_dir_all(fx.repo.join("heavy")).unwrap();

    let out = fx.flashwt(&["create", "empty", "--json"]);
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
    fs::write(fx.repo.join(".flashwtinclude"), "heavy/\n").unwrap();

    let create_out = fx.flashwt(&["create", "to-remove", "--json"]);
    assert!(create_out.status.success());

    let remove_out = fx.flashwt(&["remove", "to-remove", "--json"]);
    assert!(remove_out.status.success());

    let stdout = String::from_utf8_lossy(&remove_out.stdout);
    let lines: Vec<&str> = stdout.trim().lines().collect();
    assert_eq!(lines.len(), 1);

    let json: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(json["flashwt_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["command"], "remove");
    assert_eq!(json["status"], "ok");

    let data = &json["data"];
    assert_eq!(data["branch"], "to-remove");
    assert!(
        data["worktree_path"]
            .as_str()
            .unwrap()
            .contains("origin-to-remove")
    );
    assert!(data["references_released"].is_number());
    assert!(data["mirror_removed"].is_boolean());

    assert!(!stdout.contains("removed worktree"));
}

#[test]
fn sweep_json_emits_valid_envelope() {
    let store_dir = tempfile::tempdir().unwrap();
    let fx = Fixture::heavy_repo(10);
    fs::write(fx.repo.join(".flashwtinclude"), "heavy/\n").unwrap();

    let _ = fx.flashwt_with_store(&["create", "sweep-me"], store_dir.path());
    let _ = fx.flashwt_with_store(&["remove", "sweep-me"], store_dir.path());

    let sweep_out = fx.flashwt_with_store(&["sweep", "--age", "0s", "--json"], store_dir.path());
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

    let mig_out = fx.flashwt_with_store(
        &["store", "migrate", "--activate-mark-sweep", "--json"],
        store_dir.path(),
    );
    assert!(mig_out.status.success());
    let mig_json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&mig_out.stdout).trim()).unwrap();
    assert_eq!(mig_json["command"], "store");
    assert_eq!(mig_json["status"], "ok");
    assert_eq!(mig_json["data"]["gc_mode"], "mark-sweep");

    let sweep2_out = fx.flashwt_with_store(&["sweep", "--age", "0s", "--json"], store_dir.path());
    assert!(sweep2_out.status.success());
    let sweep2_json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&sweep2_out.stdout).trim()).unwrap();
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
    fs::write(fx.repo.join(".flashwtinclude"), "heavy/\n").unwrap();

    let _ = fx.flashwt_with_store(&["create", "scrub-demo"], store_dir.path());

    let scrub_dry = fx.flashwt_with_store(&["scrub", "--dry-run", "--json"], store_dir.path());
    assert!(scrub_dry.status.success());
    let dry_json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&scrub_dry.stdout).trim()).unwrap();
    assert_eq!(dry_json["command"], "scrub");
    assert_eq!(dry_json["status"], "ok");
    assert_eq!(dry_json["data"]["dry_run"], true);
    assert!(dry_json["data"]["scanned"].as_u64().unwrap() > 0);
    assert_eq!(dry_json["data"]["corrupt"], serde_json::json!([]));
    assert_eq!(dry_json["data"]["deleted"], 0);

    let scrub_real = fx.flashwt_with_store(&["scrub", "--json"], store_dir.path());
    assert!(scrub_real.status.success());
    let real_json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&scrub_real.stdout).trim()).unwrap();
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

    let out = fx.flashwt(&["create", "taken", "--json"]);
    assert!(!out.status.success());

    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.trim().lines().collect();
    assert_eq!(lines.len(), 1);

    let json: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(json["flashwt_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["command"], "create");
    assert_eq!(json["status"], "error");
    assert!(json["data"].is_null());
    assert!(json["diagnostics"].is_array());
    assert_eq!(json["diagnostics"][0]["code"], "ERROR");
    assert!(
        json["diagnostics"][0]["message"]
            .as_str()
            .unwrap()
            .contains("already exists")
    );

    let out2 = fx.flashwt(&[
        "create",
        "missing-manifest-test",
        "--manifest",
        "nonexistent.flashwtinclude",
        "--json",
    ]);
    assert!(!out2.status.success());
    let json2: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&out2.stdout).trim()).unwrap();
    assert_eq!(json2["status"], "error");
    assert_eq!(json2["command"], "create");
}

const LIST_HEAVY_FILES: usize = 50;

#[test]
fn list_and_ls_alias_parity_on_single_repo() {
    let fx = Fixture::heavy_repo(LIST_HEAVY_FILES);
    let store_dir = tempfile::tempdir().unwrap();

    let out_list = fx.flashwt_with_store(&["list"], store_dir.path());
    assert!(out_list.status.success());
    let stdout_list = String::from_utf8_lossy(&out_list.stdout);

    let out_ls = fx.flashwt_with_store(&["ls"], store_dir.path());
    assert!(out_ls.status.success());
    let stdout_ls = String::from_utf8_lossy(&out_ls.stdout);

    assert_eq!(stdout_list, stdout_ls);
    assert!(stdout_list.contains("BRANCH"));
    assert!(stdout_list.contains("PATH"));
    assert!(stdout_list.contains("HYDRATED"));
    assert!(stdout_list.contains("DISK SAVED"));
    assert!(
        stdout_list.contains("* main")
            || stdout_list.contains("*  main")
            || stdout_list.contains("*")
    );
}

#[test]
fn list_accurately_reports_hydrated_worktrees_and_disk_savings() {
    let fx = Fixture::heavy_repo(LIST_HEAVY_FILES);
    fs::write(fx.repo.join(".flashwtinclude"), "heavy/\n").unwrap();
    let store_dir = tempfile::tempdir().unwrap();

    let create1 = fx.flashwt_with_store(&["create", "feat-alpha"], store_dir.path());
    assert!(create1.status.success());

    let create2 = fx.flashwt_with_store(&["create", "feat-beta"], store_dir.path());
    assert!(create2.status.success());

    let out = fx.flashwt_with_store(&["list"], store_dir.path());
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(stdout.contains("feat-alpha"));
    assert!(stdout.contains("feat-beta"));
    assert!(
        stdout.contains(&format!("{} files", 2 * LIST_HEAVY_FILES))
            || stdout.contains(&format!("{LIST_HEAVY_FILES} files"))
    );
    assert!(stdout.contains("Total disk saved:"));
    assert!(stdout.contains("across 3 worktrees"));
}

#[test]
fn list_json_output_envelope_schema() {
    let fx = Fixture::heavy_repo(LIST_HEAVY_FILES);
    fs::write(fx.repo.join(".flashwtinclude"), "heavy/\n").unwrap();
    let store_dir = tempfile::tempdir().unwrap();

    let create_out = fx.flashwt_with_store(&["create", "json-test"], store_dir.path());
    assert!(create_out.status.success());

    let list_out = fx.flashwt_with_store(&["list", "--json"], store_dir.path());
    assert!(list_out.status.success());

    let stdout = String::from_utf8_lossy(&list_out.stdout);
    let lines: Vec<&str> = stdout.trim().lines().collect();
    assert_eq!(lines.len(), 1);

    let json: serde_json::Value = serde_json::from_str(lines[0]).expect("parse list json");
    assert_eq!(json["flashwt_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["command"], "list");
    assert_eq!(json["status"], "ok");
    assert!(json["diagnostics"].is_array());

    let data = &json["data"];
    assert_eq!(data["total_files_hydrated"], LIST_HEAVY_FILES);
    assert!(data["total_disk_saved"].as_u64().unwrap() > 0);

    let worktrees = data["worktrees"].as_array().expect("worktrees array");
    assert_eq!(worktrees.len(), 2);

    let main_flashwt = worktrees
        .iter()
        .find(|w| w["is_main"] == true)
        .expect("find main worktree");
    assert_eq!(main_flashwt["is_main"], true);
    assert_eq!(main_flashwt["is_active"], true);
    assert_eq!(main_flashwt["is_ephemeral"], false);
    assert_eq!(main_flashwt["files_hydrated"], 0);

    let feat_flashwt = worktrees
        .iter()
        .find(|w| w["branch"] == "json-test")
        .expect("find json-test worktree");
    assert_eq!(feat_flashwt["is_main"], false);
    assert_eq!(feat_flashwt["is_active"], false);
    assert_eq!(feat_flashwt["is_ephemeral"], false);
    assert_eq!(feat_flashwt["files_hydrated"], LIST_HEAVY_FILES);
    assert!(feat_flashwt["bytes_saved"].as_u64().unwrap() > 0);
    assert_eq!(feat_flashwt["bytes_hydrated"], feat_flashwt["bytes_saved"]);

    let dirs = feat_flashwt["hydrated_dirs"].as_array().expect("hydrated dirs");
    assert_eq!(dirs[0], "heavy");
}

#[test]
fn list_scratch_lease_reporting() {
    let fx = Fixture::heavy_repo(LIST_HEAVY_FILES);
    fs::write(fx.repo.join(".flashwtinclude"), "heavy/\n").unwrap();
    let store_dir = tempfile::tempdir().unwrap();

    let scratch_out = fx.flashwt_with_store(&["scratch", "--ttl", "1h", "temp-box"], store_dir.path());
    assert!(scratch_out.status.success());

    let list_json_out = fx.flashwt_with_store(&["list", "--json"], store_dir.path());
    assert!(list_json_out.status.success());

    let json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&list_json_out.stdout).trim()).unwrap();
    let worktrees = json["data"]["worktrees"].as_array().unwrap();

    let scratch_flashwt = worktrees
        .iter()
        .find(|w| w["branch"] == "temp-box")
        .expect("find scratch worktree");

    assert_eq!(scratch_flashwt["is_ephemeral"], true);
    let lease = &scratch_flashwt["lease"];
    assert!(!lease.is_null());
    assert_eq!(lease["pid_alive"], true);
    assert_eq!(lease["is_expired"], false);
    assert!(lease["ttl_remaining_secs"].as_u64().unwrap() > 0);
    assert!(lease["ttl_remaining_secs"].as_u64().unwrap() <= 3600);

    let list_human = fx.flashwt_with_store(&["list"], store_dir.path());
    assert!(list_human.status.success());
    let human_stdout = String::from_utf8_lossy(&list_human.stdout);
    assert!(human_stdout.contains("ttl:"));
    assert!(human_stdout.contains("temp-box"));
}

#[test]
fn list_active_marker_from_sub_worktree() {
    let fx = Fixture::heavy_repo(LIST_HEAVY_FILES);
    fs::write(fx.repo.join(".flashwtinclude"), "heavy/\n").unwrap();
    let store_dir = tempfile::tempdir().unwrap();

    let create_out = fx.flashwt_with_store(&["create", "sub-worktree"], store_dir.path());
    assert!(create_out.status.success());

    let sub_path = fx.repo.parent().unwrap().join("origin-sub-worktree");
    assert!(sub_path.exists());

    let out = Command::new(env!("CARGO_BIN_EXE_flashwt"))
        .args(["list", "--json"])
        .env("FLASHWT_STORE", store_dir.path())
        .current_dir(&sub_path)
        .output()
        .expect("run flashwt binary from sub worktree");

    assert!(out.status.success());
    let json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).unwrap();

    let worktrees = json["data"]["worktrees"].as_array().unwrap();
    let main_flashwt = worktrees.iter().find(|w| w["is_main"] == true).unwrap();
    let sub_flashwt = worktrees
        .iter()
        .find(|w| w["branch"] == "sub-worktree")
        .unwrap();

    assert_eq!(main_flashwt["is_active"], false);
    assert_eq!(sub_flashwt["is_active"], true);
}

#[test]
fn list_maps_porcelain_metadata_for_detached_worktree() {
    let fx = Fixture::heavy_repo(LIST_HEAVY_FILES);
    let store_dir = tempfile::tempdir().unwrap();

    let detached_path = fx.repo.parent().unwrap().join("origin-detached-peek");
    let status = Command::new("git")
        .args(["worktree", "add", "--quiet", "--detach"])
        .arg(&detached_path)
        .arg("HEAD")
        .current_dir(&fx.repo)
        .status()
        .expect("git worktree add --detach");
    assert!(status.success());

    let out = fx.flashwt_with_store(&["list", "--json"], store_dir.path());
    assert!(out.status.success());
    let json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).unwrap();
    let worktrees = json["data"]["worktrees"].as_array().unwrap();
    assert_eq!(worktrees.len(), 2);

    let detached = worktrees
        .iter()
        .find(|w| {
            w["path"]
                .as_str()
                .unwrap()
                .ends_with("origin-detached-peek")
        })
        .expect("detached worktree listed");
    assert_eq!(detached["branch"], "(detached)");
    assert_eq!(detached["is_main"], false);
    assert_eq!(detached["is_active"], false);

    let main = worktrees.iter().find(|w| w["is_main"] == true).unwrap();
    assert_eq!(main["is_main"], true);
    assert_eq!(main["is_active"], true);
}

#[test]
fn human_bytes_zero_rendering_via_list() {
    let fx = Fixture::heavy_repo(1);
    let store_dir = tempfile::tempdir().unwrap();

    let out = fx.flashwt_with_store(&["list"], store_dir.path());
    assert!(out.status.success());

    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("0 B"),
        "expected zero-byte rendering in list output:\n{stdout}"
    );
    assert!(stdout.contains("Total disk saved: 0 B"));
}

#[test]
#[ignore = "expensive 10,000-file demo benchmark fixture"]
fn human_count_grouping_via_demo_fixture_summary() {
    let out = Command::new(env!("CARGO_BIN_EXE_flashwt"))
        .args(["demo", "--json"])
        .output()
        .expect("run flashwt binary");
    assert!(out.status.success());

    let stdout = String::from_utf8(out.stdout).unwrap();
    let line = stdout
        .lines()
        .find(|l| l.contains("\"files_count\""))
        .expect("demo json envelope with files_count");
    assert!(
        line.contains("\"files_count\":100"),
        "demo json should report the fixture count line: {line}"
    );
}

#[test]
fn human_duration_and_table_rendering_via_list_ttl() {
    let fx = Fixture::heavy_repo(1);
    let store_dir = tempfile::tempdir().unwrap();

    fx.flashwt_with_store(&["new", "preso-dur"], store_dir.path());
    let scratch = fx.flashwt_with_store(&["scratch", "preso-ttl"], store_dir.path());
    assert!(scratch.status.success());

    let out = fx.flashwt_with_store(&["list"], store_dir.path());
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("BRANCH"));
    assert!(stdout.contains("DISK SAVED"));
    assert!(
        stdout.contains("ttl:"),
        "expected a lease ttl in table:\n{stdout}"
    );
    assert!(stdout.contains("preso-ttl"));

    let header = stdout
        .lines()
        .find(|l| l.contains("BRANCH"))
        .expect("list table header line");
    let branch_col = header
        .find("BRANCH")
        .expect("BRANCH header column position");

    let row = stdout
        .lines()
        .find(|l| l.contains("preso-ttl"))
        .expect("worktree row carrying the branch cell");
    assert_eq!(
        row.find("preso-ttl"),
        Some(branch_col),
        "branch cell must share the BRANCH header's character position:\nheader: {header}\nrow: {row}"
    );

    let path_col = header.find("PATH").expect("PATH header column position");
    assert_eq!(
        row[path_col..].chars().next(),
        Some('/'),
        "path cell must start under the PATH header:\nheader: {header}\nrow: {row}"
    );
}

#[test]
fn schema_v1_json_contract_conformance() {
    let schema_text = include_str!("../../../../schema/v1.json");
    let schema: serde_json::Value =
        serde_json::from_str(schema_text).expect("schema/v1.json must be valid JSON");

    assert_eq!(schema["title"], "Flash-WT Envelope v1 Schema");
    assert_eq!(schema["properties"]["schema_version"]["const"], 1);

    let defs = &schema["definitions"];
    let required_defs = [
        "Diagnostic",
        "CreateData",
        "HydrateData",
        "InitData",
        "RemoveData",
        "SweepData",
        "ScrubData",
        "MigrateData",
        "ListData",
        "WorktreeEntry",
        "LeaseEntry",
        "ScratchData",
        "DemoData",
        "CleanData",
        "LeaseData",
        "OperationReceipt",
        "DoctorData",
        "DoctorEnvVars",
        "DoctorFsCapabilities",
        "StoreDuData",
    ];

    for def in required_defs {
        assert!(
            defs[def].is_object(),
            "missing {def} definition in schema/v1.json"
        );
    }

    let cmd_enum = schema["properties"]["command"]["enum"]
        .as_array()
        .expect("command enum array");
    let cmd_names: Vec<&str> = cmd_enum.iter().filter_map(|v| v.as_str()).collect();
    assert!(cmd_names.contains(&"doctor"));

    let req = schema["required"].as_array().expect("required array");
    let req_names: Vec<&str> = req.iter().filter_map(|v| v.as_str()).collect();
    assert!(req_names.contains(&"flashwt_version"));
    assert!(req_names.contains(&"schema_version"));
    assert!(req_names.contains(&"command"));
    assert!(req_names.contains(&"status"));
    assert!(req_names.contains(&"data"));
    assert!(req_names.contains(&"diagnostics"));
}

#[test]
fn mutating_command_receipt_written_and_crash_resume() {
    let fx = Fixture::heavy_repo(10);
    fs::write(fx.repo.join(".flashwtinclude"), "heavy/\n").expect("write .flashwtinclude");

    let branch = "crash-resume-feat";
    let dest = fx.repo.parent().unwrap().join(format!("origin-{branch}"));

    let status = Command::new("git")
        .args(["worktree", "add", "-b", branch])
        .arg(&dest)
        .arg("HEAD")
        .current_dir(&fx.repo)
        .status()
        .expect("git worktree add");
    assert!(status.success());

    assert!(dest.exists());
    assert!(
        !dest.join("heavy").exists(),
        "heavy must not be hydrated yet"
    );

    let git_dir = if dest.join(".git").is_file() {
        let content = fs::read_to_string(dest.join(".git")).expect("read dot git");
        let line = content
            .lines()
            .find(|l| l.starts_with("gitdir:"))
            .expect("gitdir line");
        let raw_path = PathBuf::from(line.strip_prefix("gitdir:").unwrap().trim());
        if raw_path.is_absolute() {
            raw_path
        } else {
            dest.join(raw_path)
        }
    } else {
        dest.join(".git")
    };

    fs::create_dir_all(&git_dir).expect("create git_dir");
    let receipt_file = git_dir.join("flashwt-receipt.json");

    let in_progress_receipt = serde_json::json!({
        "operation": "create",
        "state": "in_progress",
        "timestamp": 123456789,
        "source_root": fx.repo.display().to_string(),
        "dest": dest.display().to_string(),
        "hydrated_dirs": [],
        "branch": branch
    });
    fs::write(&receipt_file, in_progress_receipt.to_string()).expect("write in-progress receipt");

    let out = fx.flashwt(&["create", branch, "--json"]);
    assert!(
        out.status.success(),
        "resuming crashed create should succeed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).expect("parse json");
    assert_eq!(json["command"], "create");
    assert_eq!(json["status"], "ok");
    assert_eq!(json["data"]["branch"], branch);
    assert_eq!(json["data"]["resumed"], true);
    assert_eq!(json["data"]["files_hydrated"], 10);

    assert!(dest.join("heavy").exists());
    assert!(dest.join("heavy/pkg00/nested/file-0.txt").exists());

    let receipt_content = fs::read_to_string(&receipt_file).expect("read updated receipt");
    let completed_receipt: serde_json::Value =
        serde_json::from_str(&receipt_content).expect("parse receipt");
    assert_eq!(completed_receipt["state"], "completed");
    assert_eq!(completed_receipt["branch"], branch);
    let hydrated_dirs = completed_receipt["hydrated_dirs"]
        .as_array()
        .expect("hydrated_dirs array");
    assert!(!hydrated_dirs.is_empty());

    let rerun = fx.flashwt(&["create", branch, "--json"]);
    assert!(!rerun.status.success());
    let rerun_json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&rerun.stdout).trim()).expect("parse json");
    assert_eq!(rerun_json["status"], "error");
    assert!(
        rerun_json["diagnostics"][0]["message"]
            .as_str()
            .unwrap()
            .contains("already exists")
    );
}

#[test]
fn lease_show_machine_readable_json() {
    let fx = Fixture::heavy_repo(5);
    fs::write(fx.repo.join(".flashwtinclude"), "heavy/\n").expect("write .flashwtinclude");
    let store_dir = tempfile::tempdir().expect("tempdir");

    let scratch_out = fx.flashwt_with_store(
        &["scratch", "--ttl", "1h", "active-lease-demo"],
        store_dir.path(),
    );
    assert!(scratch_out.status.success());

    let show_all_out = fx.flashwt_with_store(&["lease", "show", "--json"], store_dir.path());
    assert!(show_all_out.status.success());

    let all_json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&show_all_out.stdout).trim())
            .expect("parse all leases json");
    assert_eq!(all_json["command"], "lease");
    assert_eq!(all_json["status"], "ok");

    let leases = all_json["data"]["leases"].as_array().expect("leases array");
    assert!(!leases.is_empty(), "expected at least one active lease");

    let found = leases
        .iter()
        .find(|l| {
            l["lease_id"]
                .as_str()
                .map(|id| id.contains("active-lease-demo"))
                .unwrap_or(false)
        })
        .expect("find active-lease-demo");

    assert_eq!(found["pid_alive"], true);
    assert_eq!(found["is_expired"], false);
    assert!(found["ttl_remaining_secs"].as_u64().unwrap() > 0);
    assert!(found["worktree_path"].is_string());
    assert!(found["git_dir"].is_string());

    let lease_id = found["lease_id"].as_str().unwrap();
    let show_one_out = fx.flashwt_with_store(&["lease", "show", lease_id, "--json"], store_dir.path());
    assert!(show_one_out.status.success());

    let one_json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&show_one_out.stdout).trim())
            .expect("parse specific lease json");
    assert_eq!(one_json["status"], "ok");
    assert_eq!(one_json["data"]["matched_lease"]["lease_id"], lease_id);

    let not_found_out = fx.flashwt_with_store(
        &["lease", "show", "nonexistent-lease-xyz", "--json"],
        store_dir.path(),
    );
    assert!(!not_found_out.status.success());
    let err_json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&not_found_out.stdout).trim())
            .expect("parse not found json");
    assert_eq!(err_json["status"], "error");
    assert_eq!(err_json["diagnostics"][0]["code"], "ERROR");
    assert!(
        err_json["diagnostics"][0]["message"]
            .as_str()
            .unwrap()
            .contains("not found")
    );

    let human_out = fx.flashwt_with_store(&["lease", "show"], store_dir.path());
    assert!(human_out.status.success());
    let human_stdout = String::from_utf8_lossy(&human_out.stdout);
    assert!(human_stdout.contains("LEASE ID"));
    assert!(human_stdout.contains("TTL REMAINING"));
    assert!(human_stdout.contains(lease_id));
}

#[test]
fn doctor_json_golden_output() {
    let fx = Fixture::heavy_repo(5);
    let store_dir = tempfile::tempdir().expect("tempdir");

    let out = fx.flashwt_with_store(&["doctor", "--json"], store_dir.path());
    assert!(
        out.status.success(),
        "doctor --json failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let json: serde_json::Value = serde_json::from_str(stdout.trim()).expect("parse doctor json");

    assert_eq!(json["flashwt_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["command"], "doctor");
    assert_eq!(json["status"], "ok");
    assert!(json["diagnostics"].is_array());

    let data = &json["data"];
    assert!(data["store_path"].is_string());
    assert_eq!(
        data["store_path"].as_str().unwrap(),
        store_dir.path().to_string_lossy()
    );

    let env_vars = &data["env_vars"];
    assert!(env_vars.is_object());
    assert_eq!(
        env_vars["flashwt_store"].as_str(),
        Some(store_dir.path().to_str().unwrap())
    );

    let fs_caps = &data["fs_capabilities"];
    assert!(fs_caps["apfs_clonefile"].is_boolean());
    assert!(fs_caps["ficlone"].is_boolean());
    assert!(fs_caps["copy_file_range"].is_boolean());

    let du = &data["store_disk_usage"];
    assert!(du["store_path"].is_string());
    assert!(du["objects_bytes"].is_number());
    assert!(du["snapshots_bytes"].is_number());
    assert!(du["mirrors_bytes"].is_number());
    assert!(du["refs_bytes"].is_number());
    assert!(du["caches_bytes"].is_number());
    assert!(du["total_bytes"].is_number());
}

#[test]
fn lease_show_json_golden_output() {
    let fx = Fixture::heavy_repo(5);
    fs::write(fx.repo.join(".flashwtinclude"), "heavy/\n").expect("write .flashwtinclude");
    let store_dir = tempfile::tempdir().expect("tempdir");

    let scratch_out = fx.flashwt_with_store(
        &["scratch", "--ttl", "1h", "golden-lease"],
        store_dir.path(),
    );
    assert!(scratch_out.status.success());

    let out = fx.flashwt_with_store(&["lease", "show", "--json"], store_dir.path());
    assert!(out.status.success());

    let json: serde_json::Value = serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim())
        .expect("parse lease show json");
    assert_eq!(json["flashwt_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["command"], "lease");
    assert_eq!(json["status"], "ok");
    assert!(json["diagnostics"].is_array());

    let leases = json["data"]["leases"].as_array().expect("leases array");
    assert!(!leases.is_empty());
    let lease = &leases[0];
    assert!(lease["lease_id"].is_string());
    assert!(lease["pid"].is_number());
    assert_eq!(lease["pid_alive"], true);
    assert!(lease["expires_at"].is_number());
    assert!(lease["ttl_remaining_secs"].is_number());
    assert_eq!(lease["is_expired"], false);
    assert!(lease["worktree_path"].is_string());
    assert!(lease["git_dir"].is_string());
}

#[test]
fn execution_receipt_json_golden_output() {
    let fx = Fixture::heavy_repo(5);
    fs::write(fx.repo.join(".flashwtinclude"), "heavy/\n").expect("write .flashwtinclude");

    let out = fx.flashwt(&["create", "golden-rcpt", "--json"]);
    assert!(out.status.success());

    let json: serde_json::Value = serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim())
        .expect("parse create json");
    assert_eq!(json["command"], "create");
    assert_eq!(json["status"], "ok");

    let receipt_path_str = json["data"]["receipt_path"]
        .as_str()
        .expect("receipt_path must be present in create data");
    let receipt_path = PathBuf::from(receipt_path_str);
    assert!(receipt_path.exists(), "receipt file must exist on disk");

    let content = fs::read_to_string(&receipt_path).expect("read receipt");
    let receipt: serde_json::Value = serde_json::from_str(&content).expect("parse receipt json");

    assert_eq!(receipt["operation"], "create");
    assert_eq!(receipt["state"], "completed");
    assert!(receipt["timestamp"].is_number());
    assert!(receipt["source_root"].is_string());
    assert!(receipt["dest"].is_string());
    assert!(receipt["hydrated_dirs"].is_array());
    assert_eq!(receipt["branch"], "golden-rcpt");
    assert!(receipt["pid"].is_number());

    let dest = PathBuf::from(json["data"]["worktree_path"].as_str().unwrap());
    let hyd_out = fx.flashwt(&["hydrate", &dest.to_string_lossy(), "--json"]);
    assert!(
        hyd_out.status.success(),
        "hydrate --json failed: {}",
        String::from_utf8_lossy(&hyd_out.stderr)
    );

    let hyd_json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&hyd_out.stdout).trim())
            .expect("parse hydrate json");
    assert_eq!(hyd_json["command"], "hydrate");
    assert_eq!(hyd_json["status"], "ok");

    let hyd_receipt_path_str = hyd_json["data"]["receipt_path"]
        .as_str()
        .expect("receipt_path in hydrate data");
    let hyd_receipt_content =
        fs::read_to_string(hyd_receipt_path_str).expect("read hydrate receipt");
    let hyd_receipt: serde_json::Value =
        serde_json::from_str(&hyd_receipt_content).expect("parse hydrate receipt json");
    assert_eq!(hyd_receipt["operation"], "hydrate");
    assert_eq!(hyd_receipt["state"], "completed");
    assert!(hyd_receipt["timestamp"].is_number());
    assert!(hyd_receipt["dest"].is_string());
}
