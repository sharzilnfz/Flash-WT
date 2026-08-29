# Issue 04: Tree Ingestion Deepening in Store Package

Status: ready-for-agent

## Context
`crates/wt-cli/src/hydrate.rs` is over 1,000 lines long and contains low-level directory walking (`ingest_dir`, `ingest_dir_walk`), `ValidationCache` file management, and macOS `bulkwalk` integration. Ingesting trees into the content-addressed store is fundamentally a storage engine responsibility.

## Requirements
- Move directory tree ingestion, validation cache management, and bulk walker coordination into `wt-store::ingest`.
- Expose a clean `Store::ingest_tree` method that scans source directories, updates the validation cache, hashes new content, and returns an `IngestedTree` summary.
- Refactor `HydrationEngine` in `wt-cli` to call `Store::ingest_tree`, simplifying `hydrate.rs`.
- Ensure all store ingestion and cache invalidation unit and integration tests pass.

## Files Owned
- `crates/wt-store/src/ingest.rs`
- `crates/wt-store/src/lib.rs`
- `crates/wt-cli/src/hydrate.rs`
- `crates/wt-store/tests/store.rs`
