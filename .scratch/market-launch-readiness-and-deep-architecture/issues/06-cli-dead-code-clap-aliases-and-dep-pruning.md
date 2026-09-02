# 06: CLI Dead Code Elimination, Native Clap Aliases & Dependency Pruning

Status: ready-for-agent

Blocked by: None (can start immediately).

## Problem

`wt-cli` contains dead structs and forwarder shims (`HydrationFilter`, `manifest.rs`), duplicate subcommand enum variants that repeat command dispatch logic, repetitive JSON envelope serialization blocks, unneeded zeroed struct literals, and an unnecessary direct dependency on the `sha2` crate.

## Work

1. Delete the unused `HydrationFilter` struct and its 8 unused methods in `hydration_filter.rs`; delete the forwarder shim module `manifest.rs`.
2. Convert duplicate subcommand enum variants (`New`, `Isolate`, `TestDrive`) to native Clap aliases (`#[command(alias = "...")]`) on `Create`, `Scratch`, and `Demo`.
3. Deduplicate JSON envelope emission in `commands/mod.rs` by extracting a shared `emit_json` helper function.
4. Derive `Default` on `CleanData` in `envelope.rs` and eliminate 4 repetitive zeroed struct literals in `clean.rs`.
5. Simplify scratch worktree ID generation to timestamp-PID bit mixing and use `wt_store` content hashing in `demo.rs`, removing the direct `sha2` crate dependency from `crates/wt-cli/Cargo.toml`.
6. Remove dead parsed fields `is_locked` and `is_prunable` on `RawGitWorktree` and dead constructor `Diagnostic::info`.

## Files Owned

- `crates/wt-cli/Cargo.toml`
- `crates/wt-cli/src/cli.rs`
- `crates/wt-cli/src/commands/mod.rs`
- `crates/wt-cli/src/commands/create.rs`
- `crates/wt-cli/src/commands/clean.rs`
- `crates/wt-cli/src/commands/scratch.rs`
- `crates/wt-cli/src/commands/demo.rs`
- `crates/wt-cli/src/hydration_filter.rs`
- `crates/wt-cli/src/manifest.rs`
- `crates/wt-cli/src/envelope.rs`
- `crates/wt-cli/src/diagnostics.rs`
- `crates/wt-cli/src/gitops.rs`

## Done When

- [ ] Dead `HydrationFilter` struct and `manifest.rs` module are removed.
- [ ] `wt new`, `wt isolate`, and `wt test-drive` operate cleanly as native Clap aliases.
- [ ] Envelope emission logic is consolidated into a single reusable helper (`emit_json`).
- [ ] Direct `sha2` dependency is removed from `crates/wt-cli/Cargo.toml`.
- [ ] All CLI commands (`new`, `create`, `clean`, `list`, `scratch`, `demo`) pass unit and integration tests.
