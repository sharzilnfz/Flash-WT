# Issue 03: Snapshot Projection Engine Relocation

Status: ready-for-human

## Context
`flashwt-cli/src/snapshots.rs` contains 700+ lines of storage-layer caching policies: v2 delta rebuild heuristics, selection index rotation, ENOENT healing retries, and lockfile validation tiers.

## Requirements
- Move snapshot selection, incremental delta planning, and self-healing blob rebuilds into a deep `SnapshotProjectionEngine` in `flashwt-store`.
- Ensure the CLI interacts only with the projection seam (`materialize_snapshot`), while internal cache ring and healing mechanics remain encapsulated in `flashwt-store`.
- Preserve full macOS snapshot acceleration and parity benchmarks.

## Files Owned
- `crates/flashwt-store/src/snapshot/mod.rs`
- `crates/flashwt-store/src/snapindex.rs`
- `crates/flashwt-cli/src/snapshots.rs`
