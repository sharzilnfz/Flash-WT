# Issue 02: Autonomous Store & Lease Reclamation Engine

Status: ready-for-human

## Context
Garbage collection and lease expiration are split between `flashwt-cli/src/gc.rs` and `flashwt-store/src/gc.rs`. `flashwt-cli` contains raw store GC logic (`sweep_leases`) that inspects PID tables, parses ledgers, removes mirrors, and runs `git worktree remove` and `git branch -D`.

## Requirements
- Consolidate lease evaluation, mark-and-sweep object collection, and snapshot budget pruning inside a deep `StoreReclaimer` module in `flashwt-store`.
- Define a clean `WorkspaceCleaner` adapter trait for git worktree and filesystem removals.
- Expose a single `reclaimer.sweep(policy) -> Result<SweepSummary>` method.
- Update `flashwt sweep` command to invoke `StoreReclaimer`.

## Files Owned
- `crates/flashwt-store/src/gc.rs`
- `crates/flashwt-store/src/lease.rs`
- `crates/flashwt-cli/src/gc.rs`
- `crates/flashwt-cli/src/commands/sweep.rs`
