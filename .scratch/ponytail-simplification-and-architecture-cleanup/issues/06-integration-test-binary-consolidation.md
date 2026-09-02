# Ticket 06: Integration Test Binary Consolidation

Status: ready-for-agent

## Description

`crates/wt-cli/tests/` contains 26 discrete test files. Each test file compiles into a separate executable linking `clap`, `serde`, and `libc`, causing excessive link times and long CI runs. Furthermore, obsolete test files targeting pre-v0.1 legacy refcounts still exist.

## Requirements

1. **Consolidate 26 CLI Test Files into 5 Cohesive Module Targets**:
   - Merge related test files into 5 primary test executables under `crates/wt-cli/tests/`:
     1. `commands.rs` (combining `cli.rs`, `new.rs`, `hydrate.rs`, `clean.rs`, `scratch.rs`, `scrub.rs`, `demo.rs`, `completions.rs`)
     2. `snapshots.rs` (combining `snapshots.rs`, `snapshots_v2.rs`, `lockfile_fastpath.rs`, `apfs_defaults.rs`)
     3. `gc.rs` (combining `gc_mirror.rs`, `gc_snapshot_cap.rs`, `lease_sweep.rs`)
     4. `storage.rs` (combining `cow_materialization.rs`, `hardlink_safety.rs`, `store_flow.rs`, `cache_flow.rs`, `branch_stacking.rs`, `toolchain_relocation.rs`)
     5. `presentation.rs` (combining `json_output.rs`, `list.rs`, `output.rs`, `config.rs`)

2. **Delete Obsolete Legacy Refcount Test Suites**:
   - Delete `crates/wt-cli/tests/gc.rs` (which tests obsolete `refs/` plumbing replaced by `gc_mirror.rs`).
   - In `crates/wt-store/tests/store.rs`, delete legacy refcount lock and sweep test cases (`test_add_release_ref`, `test_sweep_refcount`).

3. **Verify Zero Regressions**:
   - Ensure all 130+ integration test cases are preserved and execute within the consolidated suites.

## Verification

- Run `cargo test -p wt-cli` and `cargo test -p wt-store`.
- Measure test execution time to confirm reduction from minutes to under 30 seconds.
