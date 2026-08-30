# 04: CLI Dead Code Elimination, Native Clap Aliases & Dependency Pruning

**What to build:**
Streamline `wt-cli` by deleting dead structs, adopting native Clap aliases, deduplicating envelope formatting, and dropping unnecessary direct dependencies:
1. Delete the unused `HydrationFilter` struct and its 8 unused methods in `hydration_filter.rs`; delete the forwarder shim module `manifest.rs`.
2. Convert duplicate subcommand enum variants (`New`, `Isolate`, `TestDrive`) to native Clap aliases (`#[command(alias = "...")]`) on `Create`, `Scratch`, and `Demo`.
3. Deduplicate JSON envelope emission in `commands/mod.rs` by extracting a shared `emit_json` helper function.
4. Derive `Default` on `CleanData` in `envelope.rs` and eliminate 4 repetitive zeroed struct literals in `clean.rs`.
5. Simplify scratch worktree ID generation to timestamp-PID bit mixing and use `wt_store` content hashing in `demo.rs`, removing the direct `sha2` crate dependency from `wt-cli/Cargo.toml`.
6. Remove dead parsed fields `is_locked` and `is_prunable` on `RawGitWorktree` and dead constructor `Diagnostic::info`.

**Blocked by:** None (can start immediately).

**Status:** ready-for-agent

- [ ] Dead `HydrationFilter` struct and `manifest.rs` module are removed
- [ ] `wt new`, `wt isolate`, and `wt test-drive` operate cleanly as native Clap aliases
- [ ] Envelope emission logic is consolidated into a single reusable helper
- [ ] Direct `sha2` dependency is removed from `crates/wt-cli/Cargo.toml`
- [ ] All CLI commands (`new`, `create`, `clean`, `list`, `scratch`, `demo`, `completions`) pass unit and integration tests
