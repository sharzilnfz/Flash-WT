# Issue 02: Autonomous Store & Lease Reclamation Engine

Status: ready-for-human

## Context
Garbage collection and lease expiration are split between `wt-cli/src/gc.rs` and `wt-store/src/gc.rs`. `wt-cli` contains raw store GC logic (`sweep_leases`) that inspects PID tables, parses ledgers, removes mirrors, and runs `git worktree remove` and `git branch -D`.

## Requirements
- Consolidate lease evaluation, mark-and-sweep object collection, and snapshot budget pruning inside a deep `StoreReclaimer` module in `wt-store`.
- Define a clean `WorkspaceCleaner` adapter trait for git worktree and filesystem removals.
- Expose a single `reclaimer.sweep(policy) -> Result<SweepSummary>` method.
- Update `wt sweep` command to invoke `StoreReclaimer`.

## Files Owned
- `crates/wt-store/src/gc.rs`
- `crates/wt-store/src/lease.rs`
- `crates/wt-cli/src/gc.rs`
- `crates/wt-cli/src/commands/sweep.rs`
