//! Fast-hydration ticket 05: trust-once materialization, asserted
//! end to end through the CLI seam.
//!
//! A blob's hash is checked at least once before anything lands in a
//! tree; afterwards a verified-blob ledger beside the store lets warm
//! runs skip the read-and-hash entirely. The trust boundary is proved
//! from outside: tamper with a stored blob and watch which creates
//! notice.

mod common;

use std::fs;
use std::path::Path;
use std::process::Command;

use common::{list_files, Fixture};

/// Flip the first byte of one stored blob. When `restore_mtime` is
/// set, put the original mtime back exactly afterwards: size never
/// changes, so no stat-visible property does, and only a real re-hash
/// could catch the tampering.
fn tamper_blob(store: &Path, restore_mtime: bool) {
    let blob = list_files(&store.join("objects"))
        .into_iter()
        .find(|p| p.is_file())
        .expect("store has objects");
    let mtime = fs::metadata(&blob).unwrap().modified().unwrap();
    let mut bytes = fs::read(&blob).unwrap();
    bytes[0] ^= 0xff;
    // Disk-level corruption does not respect read-only inodes.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&blob).unwrap().permissions();
        perms.set_mode(perms.mode() | 0o200);
        fs::set_permissions(&blob, perms).unwrap();
    }
    fs::write(&blob, &bytes).unwrap();
    if restore_mtime {
        let f = fs::OpenOptions::new()
            .write(true)
            .open(&blob)
            .expect("reopen blob");
        f.set_times(fs::FileTimes::new().set_modified(mtime))
            .expect("restore mtime");
    }
}

fn wt(
    fx: &Fixture,
    args: &[&str],
    store: &Path,
    extra_env: &[(&str, &str)],
) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_wt"));
    cmd.args(args)
        .env("WT_STORE", store)
        .env_remove("WT_HARDLINK")
        .env_remove("WT_NO_HARDLINK")
        .env_remove("WT_VERIFY")
        .current_dir(&fx.repo);
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    cmd.output().expect("run wt binary")
}

fn fixture(files: usize) -> (Fixture, std::path::PathBuf) {
    let fx = Fixture::heavy_repo(files);
    fs::write(fx.repo.join(".wtinclude"), "heavy/\n").unwrap();
    let base = tempfile::tempdir().unwrap();
    let store = base.path().join("store");
    (fx, store)
}

#[test]
fn second_create_succeeds_on_tampering_no_stat_can_see() {
    let (fx, store) = fixture(20);

    let first = wt(&fx, &["create", "one"], &store, &[]);
    assert!(
        first.status.success(),
        "first create failed: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    // A clean run leaves its verifications behind.
    assert!(store.join("verified.tsv").is_file());

    // Bit rot that preserves both size and mtime. The next create
    // SUCCEEDS — proving no byte was re-read or re-hashed. This is
    // the documented residual risk of trust-once verification, not a
    // bug: WT_VERIFY=1 is the paranoid escape hatch (tested below).
    tamper_blob(&store, true);

    let second = wt(&fx, &["create", "two"], &store, &[]);
    assert!(
        second.status.success(),
        "warm create must skip re-verification entirely: {}",
        String::from_utf8_lossy(&second.stderr)
    );
}

#[test]
fn tampering_that_moves_mtime_still_fails_loudly() {
    let (fx, store) = fixture(20);

    let first = wt(&fx, &["create", "good"], &store, &[]);
    assert!(first.status.success());

    tamper_blob(&store, false);

    let second = wt(&fx, &["create", "bad"], &store, &[]);
    assert!(
        !second.status.success(),
        "corrupt store content must fail the create"
    );
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(
        stderr.contains("hash verification"),
        "failure must name hash verification loudly:\n{stderr}"
    );
}

#[test]
fn deleted_ledger_forces_full_verification_again() {
    let (fx, store) = fixture(20);

    let first = wt(&fx, &["create", "one"], &store, &[]);
    assert!(first.status.success());

    // No ledger, no trust: even stat-invisible tampering gets caught,
    // because every blob is verified on first touch.
    fs::remove_file(store.join("verified.tsv")).expect("delete ledger");
    tamper_blob(&store, true);

    let second = wt(&fx, &["create", "two"], &store, &[]);
    assert!(
        !second.status.success(),
        "without a ledger nothing may be trusted"
    );
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(
        stderr.contains("hash verification"),
        "degraded-to-cold must still verify loudly:\n{stderr}"
    );
}

#[test]
fn wt_verify_catches_even_stat_invisible_tampering() {
    let (fx, store) = fixture(20);

    let first = wt(&fx, &["create", "one"], &store, &[]);
    assert!(first.status.success());

    tamper_blob(&store, true);

    let paranoid = wt(&fx, &["create", "two"], &store, &[("WT_VERIFY", "1")]);
    assert!(
        !paranoid.status.success(),
        "WT_VERIFY=1 must re-hash everything regardless of the ledger"
    );
    let stderr = String::from_utf8_lossy(&paranoid.stderr);
    assert!(
        stderr.contains("hash verification"),
        "WT_VERIFY failure must name hash verification loudly:\n{stderr}"
    );
}
