# 04: Tree Ingestion Deepening in Store Package

Status: ready-for-agent

Blocked by: `03-store-snapshot-index-simplification-and-stdlib-codecs.md`

## Problem

`crates/wt-cli/src/hydrate.rs` is over 1,000 lines long and contains low-level directory walking (`ingest_dir`, `ingest_dir_walk`), `ValidationCache` file management, and macOS `bulkwalk` integration. Ingesting filesystem trees into the content-addressed store is fundamentally a storage engine responsibility and belongs inside `wt-store`.

## Work

1. Move directory tree ingestion (`ingest_dir`, `ingest_dir_walk`), `ValidationCache` file management, and macOS bulk walker coordination into `crates/wt-store/src/ingest.rs`.
2. Expose a clean `DiskStore::ingest_tree` method that scans source directories, updates the validation cache, hashes new content, and returns an `IngestedTree` summary.
3. Refactor `HydrationEngine` in `wt-cli` (`crates/wt-cli/src/hydrate.rs`) to call `DiskStore::ingest_tree`, simplifying `hydrate.rs` by several hundred lines.
4. Ensure all store ingestion, bulk walking, and cache invalidation unit and integration tests pass.

## Files Owned

- `crates/wt-store/src/ingest.rs`
- `crates/wt-store/src/lib.rs`
- `crates/wt-cli/src/hydrate.rs`
- `crates/wt-store/tests/store.rs`

## Done When

- [ ] Directory tree ingestion logic lives in `crates/wt-store/src/ingest.rs`.
- [ ] `DiskStore::ingest_tree` serves as the primary ingestion entry point.
- [ ] `HydrationEngine` delegates tree ingestion to `DiskStore::ingest_tree`.
- [ ] `hydrate.rs` is simplified and stripped of raw filesystem walking loops.
- [ ] All store ingestion and cache invalidation tests compile and pass.
