# Issue 01: Unified Hydration Engine

Status: ready-for-agent

## Context
`wt-cli/src/hydrate.rs` and `wt-cli/src/commands/create.rs` currently expose six separate functions for ingesting files, selecting strategy, materializing blobs, writing sidecar manifests, claiming references, and publishing mirrors. This shallowness forces `create.rs` and `scratch.rs` to manually coordinate storage and copy invariants.

## Requirements
- Introduce a deep `HydrationEngine` module that encapsulates directory walking, verification caching, multi-threaded CoW/reflink placement, atomic mirror persistence, and sidecar ledger updates.
- Reduce caller surface to a single `engine.hydrate(request) -> Result<HydrationReport>` method.
- Refactor `wt create`, `wt scratch`, and `wt isolate` to call `HydrationEngine`.
- Ensure all existing end-to-end hydration tests pass without regression.

## Files Owned
- `crates/wt-cli/src/hydrate.rs`
- `crates/wt-cli/src/commands/create.rs`
- `crates/wt-cli/src/commands/scratch.rs`
