# 05: Deep Worktree Retirement and Storage Reclamation

**What to build:** Deepen `StoreReclaimer` in `flashwt-store` to own worktree sidecar retirement, mirror unlinking, and garbage collection through a single reclamation seam. Delete the three shallow command forwarders (`commands/remove.rs`, `commands/sweep.rs`, `commands/migrate.rs`) by collapsing command dispatch into the CLI entry points.

**Blocked by:** 03: Deep Hydration Engine and Storage Ledger in flashwt-store

**Status:** ready-for-agent

- [x] `StoreReclaimer` in `flashwt-store` provides a `retire_worktree` method that parses the sidecar ledger, releases references, and deletes the store-local mirror
- [x] Workspace directory removal is coordinated through the `WorkspaceCleaner` adapter
- [x] Command handlers call `StoreReclaimer` directly without manual sidecar TSV parsing in the CLI crate
- [x] The shallow command modules `crates/flashwt-cli/src/commands/remove.rs`, `crates/flashwt-cli/src/commands/sweep.rs`, and `crates/flashwt-cli/src/commands/migrate.rs` are deleted and their dispatch is consolidated
- [x] Existing garbage collection tests (`gc.rs`, `gc_mirror.rs`, `clean.rs`) pass without regression
