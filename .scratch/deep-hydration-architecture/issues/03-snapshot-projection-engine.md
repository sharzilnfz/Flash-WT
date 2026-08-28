# Issue 03: Snapshot Projection Engine Relocation

Status: ready-for-human

## Context
`wt-cli/src/snapshots.rs` contains 700+ lines of storage-layer caching policies: v2 delta rebuild heuristics, selection index rotation, ENOENT healing retries, and lockfile validation tiers.

## Requirements
- Move snapshot selection, incremental delta planning, and self-healing blob rebuilds into a deep `SnapshotProjectionEngine` in `wt-store`.
- Ensure the CLI interacts only with the projection seam (`materialize_snapshot`), while internal cache ring and healing mechanics remain encapsulated in `wt-store`.
- Preserve full macOS snapshot acceleration and parity benchmarks.

## Files Owned
- `crates/wt-store/src/snapshot/mod.rs`
- `crates/wt-store/src/snapindex.rs`
- `crates/wt-cli/src/snapshots.rs`
