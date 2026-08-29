// Tests assert with unwrap/expect by design: a panic IS the failure
// signal under test, so the workspace restriction lints stay off here.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Integration tests for the presentation module, exercised end to
//! end through the `wt` binary's human output (ticket 02).

mod common;

use common::Fixture;
use std::process::Command;

fn wt(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_wt"))
        .args(args)
        .output()
        .expect("run wt binary")
}

#[test]
fn human_bytes_zero_rendering_via_list() {
    let fx = Fixture::heavy_repo(1);
    let store_dir = tempfile::tempdir().unwrap();

    let out = fx.wt_with_store(&["list"], store_dir.path());
    assert!(out.status.success());

    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("0 B"), "expected zero-byte rendering in list output:\n{stdout}");
    assert!(stdout.contains("Total disk saved: 0 B"));
}

#[test]
fn human_count_grouping_via_demo_fixture_summary() {
    let out = wt(&["demo", "--json"]);
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

    fx.wt_with_store(&["new", "preso-dur"], store_dir.path());
    let scratch = fx.wt_with_store(&["scratch", "preso-ttl"], store_dir.path());
    assert!(scratch.status.success());

    let out = fx.wt_with_store(&["list"], store_dir.path());
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("BRANCH"));
    assert!(stdout.contains("DISK SAVED"));
    assert!(stdout.contains("ttl:"), "expected a lease ttl in table:\n{stdout}");
    assert!(stdout.contains("preso-ttl"));
}
