//! Integration tests for core commands:
//! cli (create/init/manifests), new, hydrate, clean, scratch/isolate, scrub, demo, completions.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use crate::common::{Fixture, git, git_out, list_files};
use wt_store::{ContentId, DiskStore, PublishOptions, WorktreeLease, lease_path};

const HEAVY_FILES: usize = 2_000;

// =========================================================================
// CLI / Create / Manifests (from cli.rs)
// =========================================================================

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

    let dest = fx.repo.parent().unwrap().join("origin-demo");
    assert!(
        dest.is_dir(),
        "worktree directory missing at {}",
        dest.display()
    );
    assert!(dest.join(".git").is_file());
    assert_eq!(
        fs::read_to_string(dest.join("src.txt")).unwrap(),
        "tracked source\n"
    );

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
fn create_without_manifest_uses_defaults_and_does_not_write_starter() {
    let fx = Fixture::heavy_repo(50);
    fs::rename(fx.repo.join("heavy"), fx.repo.join("node_modules")).unwrap();

    let out = fx.wt(&["create", "demo"]);
    assert!(
        out.status.success(),
        "wt create failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let dest = fx.repo.parent().unwrap().join("origin-demo");
    assert_same_tree(&fx.repo.join("node_modules"), &dest.join("node_modules"));
    assert!(!fx.repo.join(".wtinclude").exists());
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
    fs::write(fx.repo.join(".wtinclude"), "/heavy/pkg0*/nested\n").unwrap();

    let out = fx.wt(&["create", "demo"]);
    assert!(
        out.status.success(),
        "wt create failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

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

// =========================================================================
// `wt new` and `wt isolate` verbs (from new.rs)
// =========================================================================

#[test]
fn wt_new_creates_worktree_with_structured_receipt() {
    let fx = Fixture::node_modules_repo();
    let out = fx.wt_env(&["new", "feat-alpha"], &[]);
    assert!(
        out.status.success(),
        "wt new failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("✓ Created worktree"),
        "expected ✓ Created worktree in {stdout}"
    );
    assert!(
        stdout.contains("feat-alpha"),
        "expected branch name in {stdout}"
    );
    assert!(
        stdout.contains("Next: cd"),
        "expected actionable next step hint in {stdout}"
    );

    let wt_path = fx.repo.parent().unwrap().join("origin-feat-alpha");
    assert!(wt_path.exists(), "worktree directory must exist");
    assert!(
        wt_path.join("node_modules/pkg/index.js").exists(),
        "hydrated file must exist"
    );
}

#[test]
fn wt_new_json_emits_valid_ndjson_envelope() {
    let fx = Fixture::node_modules_repo();
    let out = fx.wt_env(&["new", "feat-json", "--json"], &[]);
    assert!(out.status.success(), "wt new --json failed");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let val: serde_json::Value = serde_json::from_str(stdout.trim()).expect("valid JSON envelope");
    assert_eq!(val["command"], "create");
    assert_eq!(val["status"], "ok");
    assert_eq!(val["data"]["branch"], "feat-json");
    assert_eq!(val["data"]["files_hydrated"], 1);
}

#[test]
fn wt_isolate_aliases_scratch_worktree_execution() {
    let fx = Fixture::node_modules_repo();
    let out = fx.wt_env(&["isolate", "--run", "echo isolate-ok", "--json"], &[]);
    assert!(out.status.success(), "wt isolate failed");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let val: serde_json::Value = serde_json::from_str(stdout.trim()).expect("valid JSON envelope");
    assert_eq!(val["command"], "scratch");
    assert_eq!(val["status"], "ok");
    assert_eq!(val["data"]["command"], "echo isolate-ok");
    assert_eq!(val["data"]["cleaned_up"], true);
}

// =========================================================================
// `wt-hydrate` and Standalone Hydration (from hydrate.rs)
// =========================================================================

#[test]
fn wt_hydrate_populates_existing_git_worktree() {
    let fx = Fixture::heavy_repo(50);
    fs::rename(fx.repo.join("heavy"), fx.repo.join("node_modules")).unwrap();
    let parent = fx.repo.parent().unwrap();
    let external_dest = parent.join("external-worktree");

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

    let out = fx.wt_hydrate(&["hydrate", &external_dest.to_string_lossy()]);
    assert!(
        out.status.success(),
        "wt-hydrate hydrate failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

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
    assert_eq!(list_files(&dest.join("node_modules")).len(), 30);
}

#[test]
fn wt_hydrate_with_custom_manifest() {
    let fx = Fixture::heavy_repo(20);
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
    assert!(!fx.repo.join(".wtinclude").exists());

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

    let init_again = fx.wt(&["init"]);
    assert!(!init_again.status.success());

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

// =========================================================================
// `wt clean` (from clean.rs)
// =========================================================================

#[test]
fn wt_clean_unregisters_raw_git_worktree_and_spares_sibling() {
    let fx = Fixture::node_modules_repo();

    let first = fx.repo.parent().unwrap().join("origin-raw-a");
    let second = fx.repo.parent().unwrap().join("origin-raw-b");
    git(&fx.repo, &["branch", "raw-a"]);
    git(&fx.repo, &["branch", "raw-b"]);
    git(
        &fx.repo,
        &[
            "worktree",
            "add",
            "--quiet",
            first.to_str().unwrap(),
            "raw-a",
        ],
    );
    git(
        &fx.repo,
        &[
            "worktree",
            "add",
            "--quiet",
            second.to_str().unwrap(),
            "raw-b",
        ],
    );
    let canonical_first = std::fs::canonicalize(&first).unwrap();
    assert!(first.exists() && second.exists());

    let out = fx.wt_env(&["clean", "raw-a", "--json"], &[]);
    assert!(
        out.status.success(),
        "wt clean failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let val: serde_json::Value = serde_json::from_str(stdout.trim()).expect("valid JSON envelope");
    assert_eq!(
        val["data"]["removed_worktrees"][0],
        canonical_first.to_string_lossy().as_ref()
    );
    assert!(
        !first.exists(),
        "cleaned worktree directory must be removed"
    );

    let registered = git_out(&fx.repo, &["worktree", "list", "--porcelain"]);
    assert!(
        !registered.contains("origin-raw-a"),
        "git must forget the removed worktree: {registered}"
    );
    assert!(
        registered.contains("origin-raw-b"),
        "untouched sibling must stay registered: {registered}"
    );
    assert!(second.exists(), "untouched sibling directory must survive");
}

#[test]
fn wt_clean_single_worktree_removes_and_sweeps_with_receipt() {
    let fx = Fixture::node_modules_repo();
    let out = fx.wt_env(&["new", "feature-clean"], &[]);
    assert!(out.status.success());

    let wt_path = fx.repo.parent().unwrap().join("origin-feature-clean");
    assert!(wt_path.exists());

    let out_clean = fx.wt_env(&["clean", "feature-clean"], &[]);
    assert!(
        out_clean.status.success(),
        "wt clean failed: {}",
        String::from_utf8_lossy(&out_clean.stderr)
    );

    let stdout = String::from_utf8_lossy(&out_clean.stdout);
    assert!(
        stdout.contains("✓ Removed worktree"),
        "receipt must contain ✓ Removed worktree: {stdout}"
    );
    assert!(
        stdout.contains("feature-clean"),
        "receipt must contain branch name: {stdout}"
    );
    assert!(!wt_path.exists(), "worktree directory must be removed");
}

#[test]
fn wt_clean_single_json_emits_valid_envelope() {
    let fx = Fixture::node_modules_repo();
    let out = fx.wt_env(&["new", "feature-clean-json"], &[]);
    assert!(out.status.success());

    let out_clean = fx.wt_env(&["clean", "feature-clean-json", "--json"], &[]);
    assert!(out_clean.status.success());

    let stdout = String::from_utf8_lossy(&out_clean.stdout);
    let val: serde_json::Value = serde_json::from_str(stdout.trim()).expect("valid JSON envelope");
    assert_eq!(val["command"], "clean");
    assert_eq!(val["status"], "ok");
    assert_eq!(val["data"]["branches_removed"][0], "feature-clean-json");
    assert_eq!(val["data"]["mirrors_removed"], 1);
}

#[test]
fn wt_clean_batch_removes_merged_branches_non_interactively() {
    let fx = Fixture::node_modules_repo();

    let out1 = fx.wt_env(&["new", "merged-feat"], &[]);
    assert!(out1.status.success());

    let out2 = fx.wt_env(&["new", "unmerged-feat"], &[]);
    assert!(out2.status.success());

    let unmerged_wt = fx.repo.parent().unwrap().join("origin-unmerged-feat");
    fs::write(unmerged_wt.join("src.txt"), "divergent work").unwrap();
    git(&unmerged_wt, &["add", "."]);
    git(&unmerged_wt, &["commit", "--quiet", "-m", "unmerged work"]);

    let out_batch = fx.wt_env(&["clean", "--json"], &[]);
    assert!(out_batch.status.success());

    let stdout = String::from_utf8_lossy(&out_batch.stdout);
    let val: serde_json::Value = serde_json::from_str(stdout.trim()).expect("valid JSON envelope");
    assert_eq!(val["command"], "clean");

    let removed = val["data"]["branches_removed"].as_array().unwrap();
    let removed_names: Vec<&str> = removed.iter().map(|v| v.as_str().unwrap()).collect();
    assert!(
        removed_names.contains(&"merged-feat"),
        "merged-feat must be cleaned"
    );
    assert!(
        !removed_names.contains(&"unmerged-feat"),
        "unmerged-feat must not be cleaned without --force/--all"
    );
}

#[test]
fn wt_clean_all_removes_all_worktrees() {
    let fx = Fixture::node_modules_repo();

    let out1 = fx.wt_env(&["new", "w1"], &[]);
    assert!(out1.status.success());
    let out2 = fx.wt_env(&["new", "w2"], &[]);
    assert!(out2.status.success());

    let out_all = fx.wt_env(&["clean", "--all", "--json"], &[]);
    assert!(out_all.status.success());

    let stdout = String::from_utf8_lossy(&out_all.stdout);
    let val: serde_json::Value = serde_json::from_str(stdout.trim()).expect("valid JSON envelope");
    let removed = val["data"]["branches_removed"].as_array().unwrap();
    assert_eq!(removed.len(), 2);
}

// =========================================================================
// `wt scratch` (from scratch.rs)
// =========================================================================

#[test]
fn bare_scratch_generates_worktree_and_persists_lease() {
    let store_dir = tempfile::tempdir().unwrap();
    let fx = Fixture::heavy_repo(20);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let out = fx.wt_with_store(&["scratch", "--json"], store_dir.path());
    assert!(
        out.status.success(),
        "wt scratch --json failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.trim().lines().collect();
    assert_eq!(lines.len(), 1);

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
    assert!(worktree_path.exists());
    assert!(worktree_path.join("heavy").exists());
    assert!(lease_file.exists());

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

    let _ = fx.wt_with_store(&["remove", branch], store_dir.path());
}

#[test]
fn scratch_run_executes_child_command_and_cleans_up_on_clean_exit() {
    let store_dir = tempfile::tempdir().unwrap();
    let fx = Fixture::heavy_repo(20);
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

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("hello sandbox"));

    let leases = wt_store::read_leases(store_dir.path());
    assert_eq!(leases.len(), 0);
}

#[test]
fn scratch_run_forwards_exit_codes_and_cleans_up() {
    let store_dir = tempfile::tempdir().unwrap();
    let fx = Fixture::heavy_repo(20);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();

    let out = fx.wt_with_store(&["scratch", "--run", "exit 42"], store_dir.path());
    assert_eq!(out.status.code(), Some(42));

    let leases = wt_store::read_leases(store_dir.path());
    assert_eq!(leases.len(), 0);
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

    assert!(expires_at >= now_secs + 7150 && expires_at <= now_secs + 7250);

    let lpath = lease_path(store_dir.path(), json["data"]["lease_id"].as_str().unwrap());
    assert!(lpath.exists());

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

    let lpath = lease_path(store_dir.path(), "custom-sandbox");
    assert!(lpath.exists());

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
    assert_eq!(lines.len(), 1);

    let json: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(json["command"], "scratch");
    assert_eq!(json["status"], "ok");
    assert_eq!(json["data"]["command"], "echo 'in sandbox' && exit 0");
    assert_eq!(json["data"]["exit_code"], 0);
    assert_eq!(json["data"]["cleaned_up"], true);
}

// =========================================================================
// `wt scrub` (from scrub.rs)
// =========================================================================

fn tamper_blob(store: &Path) -> PathBuf {
    let blob = list_files(&store.join("objects"))
        .into_iter()
        .find(|p| p.is_file())
        .expect("store has objects");
    let mtime = fs::metadata(&blob).unwrap().modified().unwrap();
    let mut bytes = fs::read(&blob).unwrap();
    bytes[0] ^= 0xff;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&blob).unwrap().permissions();
        perms.set_mode(perms.mode() | 0o200);
        fs::set_permissions(&blob, perms).unwrap();
    }
    fs::write(&blob, &bytes).unwrap();
    let f = fs::OpenOptions::new()
        .write(true)
        .open(&blob)
        .expect("reopen blob");
    f.set_times(fs::FileTimes::new().set_modified(mtime))
        .expect("restore mtime");
    blob
}

fn blob_id(blob: &Path) -> ContentId {
    let shard = blob.parent().unwrap().file_name().unwrap();
    let hex = format!(
        "{}{}",
        shard.to_string_lossy(),
        blob.file_name().unwrap().to_string_lossy()
    );
    ContentId::from_hex(&hex).expect("object path parses as a content id")
}

fn wt_scrub_cmd(fx: &Fixture, args: &[&str], store: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_flashwt"))
        .args(args)
        .env("WT_STORE", store)
        .env("WT_SNAPSHOTS", "1")
        .env_remove("WT_HARDLINK")
        .env_remove("WT_NO_HARDLINK")
        .env_remove("WT_VERIFY")
        .current_dir(&fx.repo)
        .output()
        .expect("run wt binary")
}

fn scrub_fixture(files: usize) -> (Fixture, PathBuf) {
    let fx = Fixture::heavy_repo(files);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();
    let base = tempfile::tempdir().unwrap();
    let store = base.path().join("store");
    (fx, store)
}

#[test]
fn dry_run_reports_corruption_but_touches_nothing() {
    let (fx, store) = scrub_fixture(20);

    let first = wt_scrub_cmd(&fx, &["create", "one"], &store);
    assert!(first.status.success(), "create failed");

    let blob = tamper_blob(&store);

    let out = wt_scrub_cmd(&fx, &["scrub", "--dry-run"], &store);
    assert!(out.status.success(), "scrub --dry-run failed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("corrupt 1") && stdout.contains("would delete 1"),
        "dry run must report corrupt blob:\n{stdout}"
    );
    assert!(blob.is_file(), "dry run must not delete anything");

    let second = wt_scrub_cmd(&fx, &["create", "two"], &store);
    assert!(second.status.success(), "warm create must succeed");
}

#[test]
fn scrub_deletes_corrupt_blob_and_references_fail_cleanly() {
    let (fx, store) = scrub_fixture(20);

    let first = wt_scrub_cmd(&fx, &["create", "one"], &store);
    assert!(first.status.success());

    let blob = tamper_blob(&store);
    let id = blob_id(&blob);

    let out = wt_scrub_cmd(&fx, &["scrub"], &store);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("corrupt 1") && stdout.contains("deleted 1"));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains(&id.to_string()));
    assert!(!blob.exists());

    let ledger = fs::read_to_string(store.join("verified.tsv")).unwrap();
    assert!(!ledger.contains(&id.to_string()));

    let disk = DiskStore::open(&store).unwrap();
    assert!(matches!(
        disk.get(&id),
        Err(wt_store::Error::UnknownContent(_))
    ));
    assert!(matches!(
        disk.ensure_verified(&id),
        Err(wt_store::Error::UnknownContent(_))
    ));

    let again = wt_scrub_cmd(&fx, &["scrub"], &store);
    assert!(again.status.success());
    assert!(String::from_utf8_lossy(&again.stdout).contains("corrupt 0"));
}

#[test]
fn scrub_on_healthy_store_deletes_nothing() {
    let (fx, store) = scrub_fixture(20);

    let first = wt_scrub_cmd(&fx, &["create", "one"], &store);
    assert!(first.status.success());

    let before = list_files(&store.join("objects"));

    let out = wt_scrub_cmd(&fx, &["scrub"], &store);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let scanned = before.len();
    assert!(
        stdout.contains(&format!("scanned {scanned}"))
            && stdout.contains("corrupt 0")
            && stdout.contains("deleted 0")
    );
    assert_eq!(list_files(&store.join("objects")), before);
}

#[test]
fn parallel_sharded_scrubbing_detects_multiple_blob_corruptions_and_emits_json() {
    let (fx, store) = scrub_fixture(100);

    let first = wt_scrub_cmd(&fx, &["create", "one"], &store);
    assert!(first.status.success());

    let all_blobs = list_files(&store.join("objects"))
        .into_iter()
        .filter(|p| p.is_file())
        .collect::<Vec<_>>();
    assert!(all_blobs.len() >= 50);

    let mut tampered_ids = Vec::new();
    for &idx in &[0, all_blobs.len() / 2, all_blobs.len() - 1] {
        let blob = &all_blobs[idx];
        let id = blob_id(blob);
        tampered_ids.push(id.to_string());
        let mtime = fs::metadata(blob).unwrap().modified().unwrap();
        let mut bytes = fs::read(blob).unwrap();
        bytes[0] ^= 0xff;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(blob).unwrap().permissions();
            perms.set_mode(perms.mode() | 0o200);
            fs::set_permissions(blob, perms).unwrap();
        }
        fs::write(blob, &bytes).unwrap();
        let f = fs::OpenOptions::new().write(true).open(blob).unwrap();
        f.set_times(fs::FileTimes::new().set_modified(mtime))
            .unwrap();
    }

    let dry_out = wt_scrub_cmd(&fx, &["scrub", "--dry-run", "--json"], &store);
    assert!(dry_out.status.success());
    let dry_json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&dry_out.stdout).trim()).unwrap();
    assert_eq!(dry_json["status"], "ok");
    assert_eq!(dry_json["data"]["dry_run"], true);
    assert_eq!(dry_json["data"]["corrupt"].as_array().unwrap().len(), 3);
    assert_eq!(dry_json["data"]["deleted"], 0);

    let diagnostics = dry_json["diagnostics"].as_array().unwrap();
    assert!(diagnostics.iter().any(|d| d["code"] == "CORRUPT_BLOB"));

    let real_out = wt_scrub_cmd(&fx, &["scrub", "--json"], &store);
    assert!(real_out.status.success());
    let real_json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&real_out.stdout).trim()).unwrap();
    assert_eq!(real_json["status"], "ok");
    assert_eq!(real_json["data"]["deleted"], 3);

    let clean_out = wt_scrub_cmd(&fx, &["scrub", "--json"], &store);
    assert!(clean_out.status.success());
    let clean_json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&clean_out.stdout).trim()).unwrap();
    assert_eq!(clean_json["data"]["corrupt"].as_array().unwrap().len(), 0);
}

fn publish_test_snapshot(store: &Path) -> PathBuf {
    use wt_store::SnapshotEntry;
    let mut ds = DiskStore::open(store).unwrap();
    let b1 = ds.put(b"snap test content 1").unwrap();
    let b2 = ds.put(b"snap test content 2").unwrap();
    let entries = vec![
        SnapshotEntry::file("f1.txt", b1, 0o644),
        SnapshotEntry::file("sub/f2.txt", b2, 0o644),
    ];
    let manifest = wt_store::Manifest::new(entries.clone()).unwrap();
    ds.publish_snapshot(entries, PublishOptions::default())
        .unwrap();
    store.join("snapshots").join(manifest.hash.to_string())
}

#[test]
fn scrub_detects_and_purges_snapshot_with_missing_complete_marker() {
    let (fx, store) = scrub_fixture(10);
    let snap = publish_test_snapshot(&store);
    assert!(snap.is_dir());

    let complete_file = snap.join(".complete");
    if complete_file.exists() {
        fs::remove_file(&complete_file).unwrap();
    }

    let dry_out = wt_scrub_cmd(&fx, &["scrub", "--dry-run", "--json"], &store);
    assert!(dry_out.status.success());
    let dry_json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&dry_out.stdout).trim()).unwrap();
    let corrupt_snaps = dry_json["data"]["corrupt_snapshots"].as_array().unwrap();
    assert!(!corrupt_snaps.is_empty());
    assert_eq!(dry_json["data"]["snapshot_dirs_deleted"], 0);
    assert!(snap.exists());

    let diagnostics = dry_json["diagnostics"].as_array().unwrap();
    assert!(diagnostics.iter().any(|d| d["code"] == "CORRUPT_SNAPSHOT"));

    let real_out = wt_scrub_cmd(&fx, &["scrub", "--json"], &store);
    assert!(real_out.status.success());
    let real_json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&real_out.stdout).trim()).unwrap();
    assert!(real_json["data"]["snapshot_dirs_deleted"].as_u64().unwrap() >= 1);
    assert!(!snap.exists());
}

#[test]
fn scrub_detects_and_purges_snapshot_with_unparseable_manifest() {
    let (fx, store) = scrub_fixture(10);
    let snap = publish_test_snapshot(&store);
    assert!(snap.is_dir());

    fs::write(snap.join("manifest.tsv"), "not a valid manifest header\n").unwrap();

    let real_out = wt_scrub_cmd(&fx, &["scrub", "--json"], &store);
    assert!(real_out.status.success());
    let real_json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&real_out.stdout).trim()).unwrap();
    assert!(real_json["data"]["snapshot_dirs_deleted"].as_u64().unwrap() >= 1);
    assert!(!snap.exists());
}

#[test]
fn scrub_detects_and_purges_snapshot_with_corrupted_file_tree() {
    let (fx, store) = scrub_fixture(10);
    let snap = publish_test_snapshot(&store);
    assert!(snap.is_dir());

    let tree_files = list_files(&snap.join("tree"))
        .into_iter()
        .filter(|p| p.is_file())
        .collect::<Vec<_>>();
    assert!(!tree_files.is_empty());

    fs::write(&tree_files[0], b"corrupted file content in snapshot tree").unwrap();

    let real_out = wt_scrub_cmd(&fx, &["scrub", "--json"], &store);
    assert!(real_out.status.success());
    let real_json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&real_out.stdout).trim()).unwrap();
    assert!(real_json["data"]["snapshot_dirs_deleted"].as_u64().unwrap() >= 1);
    assert!(!snap.exists());
}

// =========================================================================
// `wt demo` and `wt test-drive` (from demo.rs)
// =========================================================================

#[test]
fn demo_terminal_output_renders_scorecard_and_completes_successfully() {
    let store_dir = tempfile::tempdir().unwrap();
    let isolated_cwd = tempfile::tempdir().unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_flashwt"))
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
    let store_dir = tempfile::tempdir().unwrap();
    let isolated_cwd = tempfile::tempdir().unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_flashwt"))
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

    assert!(!stdout.contains("PERFORMANCE SCORECARD"));
    assert!(!stdout.contains("Step 1/5"));
}

#[test]
fn test_drive_alias_works_identically_with_json_envelope() {
    let store_dir = tempfile::tempdir().unwrap();
    let isolated_cwd = tempfile::tempdir().unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_flashwt"))
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
    let store_dir = tempfile::tempdir().unwrap();
    let non_git_dir = tempfile::tempdir().unwrap();

    let git_check = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(non_git_dir.path())
        .output()
        .expect("check git");
    assert!(!git_check.status.success());

    let out = Command::new(env!("CARGO_BIN_EXE_flashwt"))
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

// =========================================================================
// `wt completions` (from completions.rs)
// =========================================================================

fn completions_stdout(shell: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_flashwt"))
        .args(["completions", shell])
        .output()
        .expect("run wt binary");
    assert!(
        out.status.success(),
        "wt completions {shell} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        !text.trim().is_empty(),
        "wt completions {shell} produced no output"
    );
    text
}

#[test]
fn bash_completions_define_the_wt_function() {
    let text = completions_stdout("bash");
    assert!(text.contains("_wt()"), "missing bash function: {text}");
    assert!(text.contains("--json"));
}

#[test]
fn zsh_completions_carry_the_compdef_header() {
    let text = completions_stdout("zsh");
    assert!(text.contains("#compdef wt"), "missing zsh header: {text}");
    assert!(text.contains("--json"));
}

#[test]
fn fish_completions_register_wt_completions() {
    let text = completions_stdout("fish");
    assert!(
        text.contains("complete") && text.contains("wt"),
        "missing fish completion calls: {text}"
    );
    assert!(text.contains("-l json"));
}

#[test]
fn powershell_completions_register_an_argument_completer() {
    let text = completions_stdout("powershell");
    assert!(
        text.contains("Register-ArgumentCompleter"),
        "missing PowerShell registration: {text}"
    );
    assert!(text.contains("--json"));
}

#[test]
fn elvish_completions_bind_completion_calls() {
    let text = completions_stdout("elvish");
    assert!(
        text.contains("edit:completion"),
        "missing elvish completion hooks: {text}"
    );
    assert!(text.contains("--json"));
}

#[test]
fn unknown_shell_is_rejected() {
    let out = Command::new(env!("CARGO_BIN_EXE_flashwt"))
        .args(["completions", "csh"])
        .output()
        .expect("run wt binary");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("csh"), "stderr should name the bad value");
}

#[test]
fn completions_requires_a_shell_argument() {
    let out = Command::new(env!("CARGO_BIN_EXE_flashwt"))
        .args(["completions"])
        .output()
        .expect("run wt binary");
    assert!(!out.status.success());
}

#[test]
fn help_lists_the_completions_subcommand() {
    let out = Command::new(env!("CARGO_BIN_EXE_flashwt"))
        .args(["--help"])
        .output()
        .expect("run wt binary");
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("completions"));
}
