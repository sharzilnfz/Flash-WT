# Ticket 07: Shared Test Fixture & Assertion Library

Status: ready-for-human

## Description

Individual integration test files independently implement duplicate `TestFixture`, `RichFixture`, `V2Fixture`, and `LockfileFixture` structs, re-implement Git command invocation helpers (`fn git`), and duplicate store footprint scanning logic.

## Requirements

1. **Unified `Fixture` in `common/mod.rs`**:
   - Expand `crates/flashwt-cli/tests/common/mod.rs` to provide a canonical `Fixture` struct supporting repository initialization, initial commits, file generation, and worktree creation.
   - Centralize subprocess runner `common::git(dir, args)` with standard error reporting.

2. **Shared Store Assertion Helpers**:
   - Provide shared `store_footprint(store_path)` and `assert_hydrated_files(worktree, expected)` helpers in `common/mod.rs`.
   - Replace duplicate fixture implementations across all consolidated test modules.

## Verification

- Run `cargo test -p flashwt-cli`.
- Confirm all tests use the shared fixture harness without duplicate boilerplate.
