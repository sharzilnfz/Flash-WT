# Ticket 04: Snapshot Publication Consolidation & Scrub Reuse

Status: ready-for-human

## Description

`crates/flashwt-store/src/snapshot/publish.rs` exposes 8 combinatorial wrapper methods for snapshot publication. `crates/flashwt-store/src/scrub.rs` re-implements snapshot tree traversal and hash verification in `verify_snapshot_dir`, duplicating logic already present in `DiskStore::find_snapshot` and `paranoid_verify_tree`. Additionally, advisory flock handling is fragmented across triplicate lock guard structs (`RefsLock`, `RefsDirLock`, `MetadataLock`).

## Requirements

1. **Consolidate Snapshot Publication Variants**:
   - Define a `PublishOptions` struct in `snapshot/publish.rs` with sensible defaults.
   - Collapse combinatorial `publish_snapshot_*` methods into a single primary publication method.

2. **Reuse Tree Verification in Scrub**:
   - Refactor `verify_snapshot_dir` in `crates/flashwt-store/src/scrub.rs` to reuse `DiskStore::find_snapshot` and `paranoid_verify_tree`.
   - Delete duplicate `collect_tree_rels` helper in `scrub.rs` in favor of `collect_rels` in `snapshot/tree.rs`.

3. **Unify Advisory Flock Helper**:
   - Consolidate `RefsLock`, `RefsDirLock`, and `MetadataLock` into a single `FlockGuard` in `crates/flashwt-store/src/fsutil.rs`.
   - In `snapindex.rs`, delete pass-through wrappers `SelectionIndex::save` and `SnapshotLru::save` in favor of `save_durable`.

4. **Inline Single-Implementation Traits**:
   - Move methods from single-implementation `Store` trait directly onto `DiskStore` in `crates/flashwt-store/src/disk.rs`.
   - Remove single-implementation `WorkspaceCleaner` generic trait parameter in `StoreReclaimer` in favor of a concrete cleaner or function pointer.

## Verification

- Run `cargo test -p flashwt-store --test scrub` and `cargo test -p flashwt-store --test snapshots`.
- Verify scrub detection of corrupt blobs and invalid snapshot manifests remains exact.
