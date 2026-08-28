//! Integration tests for toolchain relocation and manifest exclusions (ticket 08).

// Tests assert with unwrap/expect by design: a panic IS the failure
// signal under test, so the workspace restriction lints stay off here.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

use common::Fixture;

#[test]
fn test_venv_post_hydration_relocation() {
    let base = tempfile::tempdir().unwrap();
    let repo = base.path().join("origin");
    fs::create_dir_all(&repo).unwrap();

    // Initialize git repository
    Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(&repo)
        .status()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(&repo)
        .status()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(&repo)
        .status()
        .unwrap();

    let venv = repo.join(".venv");
    fs::create_dir_all(venv.join("bin")).unwrap();
    fs::create_dir_all(venv.join("__pycache__")).unwrap();

    // 1. pyvenv.cfg
    let cfg_content = format!(
        "home = /usr/bin\ninclude-system-site-packages = false\nversion = 3.11.2\nexecutable = /usr/bin/python3.11\ncommand = /usr/bin/python3 -m venv {}\n",
        venv.display()
    );
    fs::write(venv.join("pyvenv.cfg"), cfg_content).unwrap();

    // 2. bin/activate* shell scripts
    let act_bash = format!(
        r#"VIRTUAL_ENV="{}"
export VIRTUAL_ENV
_OLD_VIRTUAL_PATH="$PATH"
PATH="$VIRTUAL_ENV/bin:$PATH"
export PATH
"#,
        venv.display()
    );
    fs::write(venv.join("bin/activate"), act_bash).unwrap();

    let act_csh = format!(r#"setenv VIRTUAL_ENV "{}""#, venv.display());
    fs::write(venv.join("bin/activate.csh"), act_csh).unwrap();

    let act_fish = format!(r#"set -gx VIRTUAL_ENV "{}""#, venv.display());
    fs::write(venv.join("bin/activate.fish"), act_fish).unwrap();

    // 3. Script with shebang
    let script_content = format!(
        "#!{}/bin/python3\nimport sys\nprint('hello from worktree')\n",
        venv.display()
    );
    let script_path = venv.join("bin/pytest");
    fs::write(&script_path, script_content).unwrap();
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();

    // 4. .pyc bytecode file
    let pyc_bytes = b"\x6f\r\r\n\0\0\0\0\0\0\0\0dummybytecode";
    let pyc_path = venv.join("bin/cached.pyc");
    fs::write(&pyc_path, pyc_bytes).unwrap();
    let pyc_cache_path = venv.join("__pycache__/module.cpython-311.pyc");
    fs::write(&pyc_cache_path, pyc_bytes).unwrap();

    // Track a source file and commit
    fs::write(repo.join("main.py"), "print('hello')\n").unwrap();
    Command::new("git")
        .args(["add", "main.py"])
        .current_dir(&repo)
        .status()
        .unwrap();
    Command::new("git")
        .args(["commit", "--quiet", "-m", "init"])
        .current_dir(&repo)
        .status()
        .unwrap();

    // Create worktree via wt
    let store_dir = base.path().join("store");
    let wt_out = Command::new(env!("CARGO_BIN_EXE_wt"))
        .args(["create", "demo"])
        .env("WT_STORE", &store_dir)
        .current_dir(&repo)
        .output()
        .unwrap();

    assert!(
        wt_out.status.success(),
        "wt create failed: {}",
        String::from_utf8_lossy(&wt_out.stderr)
    );

    let dest = base.path().join("origin-demo");
    let dest_venv = dest.join(".venv");

    // Acceptance criterion 1: pyvenv.cfg updated with target worktree paths
    let updated_cfg = fs::read_to_string(dest_venv.join("pyvenv.cfg")).unwrap();
    assert!(
        updated_cfg.contains(&format!(
            "command = /usr/bin/python3 -m venv {}",
            dest_venv.display()
        )),
        "pyvenv.cfg was not updated with dest venv path. Got:\n{updated_cfg}"
    );
    assert!(!updated_cfg.contains(&venv.to_string_lossy().into_owned()));

    // Acceptance criterion 2: bin/activate* shell scripts updated
    let updated_bash = fs::read_to_string(dest_venv.join("bin/activate")).unwrap();
    assert!(
        updated_bash.contains(&format!("VIRTUAL_ENV=\"{}\"", dest_venv.display())),
        "bin/activate was not updated with dest venv path. Got:\n{updated_bash}"
    );
    assert!(!updated_bash.contains(&venv.to_string_lossy().into_owned()));

    let updated_csh = fs::read_to_string(dest_venv.join("bin/activate.csh")).unwrap();
    assert!(
        updated_csh.contains(&format!("setenv VIRTUAL_ENV \"{}\"", dest_venv.display())),
        "bin/activate.csh was not updated. Got:\n{updated_csh}"
    );

    let updated_fish = fs::read_to_string(dest_venv.join("bin/activate.fish")).unwrap();
    assert!(
        updated_fish.contains(&format!("set -gx VIRTUAL_ENV \"{}\"", dest_venv.display())),
        "bin/activate.fish was not updated. Got:\n{updated_fish}"
    );

    // Acceptance criterion 3: Shebang lines patched to point to target worktree Python
    let updated_script = fs::read_to_string(dest_venv.join("bin/pytest")).unwrap();
    assert!(
        updated_script.starts_with(&format!("#!{}/bin/python3\n", dest_venv.display())),
        "Shebang was not updated to dest Python binary. Got:\n{updated_script}"
    );
    let perms = fs::metadata(dest_venv.join("bin/pytest"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(perms & 0o777, 0o755, "Executable permissions not preserved");

    // Acceptance criterion 4: .pyc cache files preserved without rewriting
    assert_eq!(
        fs::read(dest_venv.join("bin/cached.pyc")).unwrap(),
        pyc_bytes
    );
    assert_eq!(
        fs::read(dest_venv.join("__pycache__/module.cpython-311.pyc")).unwrap(),
        pyc_bytes
    );
}

#[test]
fn test_starter_manifest_and_volatile_cache_exclusions() {
    let fx = Fixture::heavy_repo(5);

    // Create target/, node_modules/, and .next/ directories with normal files and volatile caches
    let target_dir = fx.repo.join("target");
    fs::create_dir_all(target_dir.join("debug/deps")).unwrap();
    fs::write(target_dir.join("debug/deps/libfoo.rlib"), b"rlib data").unwrap();
    fs::create_dir_all(target_dir.join("debug/incremental/app-xyz")).unwrap();
    fs::write(
        target_dir.join("debug/incremental/app-xyz/s-abc.o"),
        b"incremental data",
    )
    .unwrap();

    let node_modules_dir = fx.repo.join("node_modules");
    fs::create_dir_all(node_modules_dir.join("pkg")).unwrap();
    fs::write(
        node_modules_dir.join("pkg/index.js"),
        b"console.log('pkg');",
    )
    .unwrap();
    fs::create_dir_all(node_modules_dir.join(".vite/deps")).unwrap();
    fs::write(node_modules_dir.join(".vite/deps/react.js"), b"vite cache").unwrap();

    let next_dir = fx.repo.join(".next");
    fs::create_dir_all(next_dir.join("cache/webpack")).unwrap();
    fs::write(next_dir.join("cache/webpack/bundle.pack"), b"next cache").unwrap();

    // Run wt create demo without explicit manifest (creates starter manifest)
    let out = fx.wt(&["create", "demo"]);
    assert!(
        out.status.success(),
        "wt create failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Acceptance criterion 5: Starter manifests omit volatile compiler incremental caches
    let starter = fs::read_to_string(fx.repo.join(".wtinclude")).unwrap();
    assert!(
        starter.contains("!target/debug/incremental/"),
        "Starter manifest missing !target/debug/incremental/"
    );
    assert!(
        starter.contains("!node_modules/.vite/"),
        "Starter manifest missing !node_modules/.vite/"
    );
    assert!(
        starter.contains("!.next/cache/"),
        "Starter manifest missing !.next/cache/"
    );

    // Check hydrated destination
    let dest = fx.repo.parent().unwrap().join("origin-demo");

    // Regular heavy content arrived
    assert!(dest.join("target/debug/deps/libfoo.rlib").is_file());
    assert!(dest.join("node_modules/pkg/index.js").is_file());

    // Volatile caches are omitted
    assert!(
        !dest.join("target/debug/incremental").exists(),
        "target/debug/incremental was hydrated but should have been excluded!"
    );
    assert!(
        !dest.join("node_modules/.vite").exists(),
        "node_modules/.vite was hydrated but should have been excluded!"
    );
    assert!(
        !dest.join(".next/cache").exists(),
        ".next/cache was hydrated but should have been excluded!"
    );
}

#[test]
fn test_cargo_workspace_builds_in_hydrated_worktree() {
    let base = tempfile::tempdir().unwrap();
    let repo = base.path().join("origin");
    fs::create_dir_all(repo.join("src")).unwrap();

    // Initialize git repository
    Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(&repo)
        .status()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(&repo)
        .status()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(&repo)
        .status()
        .unwrap();

    let cargo_toml = r#"[package]
name = "sample-crate"
version = "0.1.0"
edition = "2021"

[dependencies]
"#;
    fs::write(repo.join("Cargo.toml"), cargo_toml).unwrap();
    fs::write(
        repo.join("src/main.rs"),
        "fn main() { println!(\"cargo-test-ok\"); }\n",
    )
    .unwrap();

    // Track and commit cargo project
    Command::new("git")
        .args(["add", "."])
        .current_dir(&repo)
        .status()
        .unwrap();
    Command::new("git")
        .args(["commit", "--quiet", "-m", "init"])
        .current_dir(&repo)
        .status()
        .unwrap();

    // Build the project in repo to generate target/ and incremental caches
    let build_out = Command::new("cargo")
        .args(["build"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(
        build_out.status.success(),
        "cargo build failed in repo: {}",
        String::from_utf8_lossy(&build_out.stderr)
    );

    assert!(repo.join("target/debug/sample-crate").is_file());
    assert!(repo.join("target/debug/incremental").is_dir());

    // Create worktree via wt
    let store_dir = base.path().join("store");
    let wt_out = Command::new(env!("CARGO_BIN_EXE_wt"))
        .args(["create", "demo"])
        .env("WT_STORE", &store_dir)
        .current_dir(&repo)
        .output()
        .unwrap();

    assert!(
        wt_out.status.success(),
        "wt create failed: {}",
        String::from_utf8_lossy(&wt_out.stderr)
    );

    let dest = base.path().join("origin-demo");

    // target/debug/deps should be hydrated, but target/debug/incremental excluded
    assert!(dest.join("target/debug/deps").is_dir());
    assert!(!dest.join("target/debug/incremental").exists());

    // Acceptance criterion 6: cargo workspace builds and runs without path errors
    let run_out = Command::new("cargo")
        .args(["run", "--quiet"])
        .current_dir(&dest)
        .output()
        .unwrap();

    assert!(
        run_out.status.success(),
        "cargo run in hydrated worktree failed: {}",
        String::from_utf8_lossy(&run_out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run_out.stdout).trim(),
        "cargo-test-ok"
    );
}
