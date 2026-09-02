# Ticket 03: Store Ingestion & Placement Deduplication

Status: ready-for-human

## Description

In `crates/wt-store/src/ingest.rs`, tree scanning logic is duplicated between the macOS `getattrlistbulk` loop and the portable recursive walk loop (stat cache checking, validation cache hit handling, CAS blob hashing, mode parsing). In `crates/wt-store/src/snapshot/tree.rs`, directory entry placement logic is duplicated between `place_entry` and `place_entry_relative`.

## Requirements

1. **Unify Entry Ingestion in `wt-store`**:
   - Extract a shared `ingest_entry` closure/helper in `crates/wt-store/src/ingest.rs` to process an individual entry (`ValidationCache` lookup, blob hashing via `DiskStore`, CAS insertion, symlink recording).
   - Use `ingest_entry` in both `bulk_walk_tree` (macOS) and portable `ingest_tree_walk`.

2. **Unify Snapshot Entry Placement**:
   - Consolidate `place_entry` and `place_entry_relative` in `crates/wt-store/src/snapshot/tree.rs` into a single placement helper taking an optional directory file descriptor (`Option<&DirFd>`).
   - Eliminate duplicated hardlink/copy/chmod branches between absolute and relative placement paths.

3. **Standard Library String Splitting**:
   - In `snapshot/tree.rs`, replace hand-rolled parent/filename splitter `split_parent_and_filename` with `str::rsplit_once('/')`.
   - In `lockfile.rs`, replace hand-rolled directory ancestor loop with `std::path::Path::ancestors()`.

## Verification

- Run `cargo test -p wt-store`.
- Verify tree ingestion and snapshot materialization pass all integrity assertions on macOS and Linux.
