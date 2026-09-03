# 06: CLI Dead Code Elimination, Native Clap Aliases & Dependency Pruning

Status: ready-for-agent

Blocked by: None (can start immediately).

## Problem

`flashwt-cli` contains dead structs and forwarder shims (`HydrationFilter`, `manifest.rs`), duplicate subcommand enum variants that repeat command dispatch logic, repetitive JSON envelope serialization blocks, unneeded zeroed struct literals, and an unnecessary direct dependency on the `sha2` crate.

## Work

1. Delete the unused `HydrationFilter` struct and its 8 unused methods in `hydration_filter.rs`; delete the forwarder shim module `manifest.rs`.
2. Convert duplicate subcommand enum variants (`New`, `Isolate`, `TestDrive`) to native Clap aliases (`#[command(alias = "...")]`) on `Create`, `Scratch`, and `Demo`.
3. Deduplicate JSON envelope emission in `commands/mod.rs` by extracting a shared `emit_json` helper function.
4. Derive `Default` on `CleanData` in `envelope.rs` and eliminate 4 repetitive zeroed struct literals in `clean.rs`.
5. Simplify scratch worktree ID generation to timestamp-PID bit mixing and use `flashwt_store` content hashing in `demo.rs`, removing the direct `sha2` crate dependency from `crates/flashwt-cli/Cargo.toml`.
6. Remove dead parsed fields `is_locked` and `is_prunable` on `RawGitWorktree` and dead constructor `Diagnostic::info`.

## Files Owned

- `crates/flashwt-cli/Cargo.toml`
- `crates/flashwt-cli/src/cli.rs`
- `crates/flashwt-cli/src/commands/mod.rs`
- `crates/flashwt-cli/src/commands/create.rs`
- `crates/flashwt-cli/src/commands/clean.rs`
- `crates/flashwt-cli/src/commands/scratch.rs`
- `crates/flashwt-cli/src/commands/demo.rs`
- `crates/flashwt-cli/src/hydration_filter.rs`
- `crates/flashwt-cli/src/manifest.rs`
- `crates/flashwt-cli/src/envelope.rs`
- `crates/flashwt-cli/src/diagnostics.rs`
- `crates/flashwt-cli/src/gitops.rs`

## Done When

- [ ] Dead `HydrationFilter` struct and `manifest.rs` module are removed.
- [ ] `flashwt new`, `flashwt isolate`, and `flashwt test-drive` operate cleanly as native Clap aliases.
- [ ] Envelope emission logic is consolidated into a single reusable helper (`emit_json`).
- [ ] Direct `sha2` dependency is removed from `crates/flashwt-cli/Cargo.toml`.
- [ ] All CLI commands (`new`, `create`, `clean`, `list`, `scratch`, `demo`) pass unit and integration tests.
