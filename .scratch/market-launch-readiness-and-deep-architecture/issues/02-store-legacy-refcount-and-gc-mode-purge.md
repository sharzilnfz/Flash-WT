# 02: Purge Legacy Store Refcounting and GC Mode Transitions

Status: ready-for-agent

Blocked by: None (can start immediately).

## Problem

`flashwt-store` retains legacy per-blob refcounting machinery and three-way GC mode migration scaffolding (`GcMode::Legacy`, `GcMode::MarkSweep`, `GcMode::MarkSweepNoRefs`). This creates unnecessary file I/O overhead (dual-writing ref counts during hydration), directory lock contention (`flock(refs/)`), and complex migration branches that increase maintainer cognitive load without providing any functional benefit over the atomic mark-and-sweep store mirror model.

## Work

1. Delete per-blob refcount file operations (`add_ref`, `release_ref`, `ref_count`, `ref_path`, `write_ref_count`, `read_ref_count`) and `Error::RefCountUnderflow` in `flashwt-store`.
2. Delete obsolete refcount sweep method `DiskStore::sweep` and `struct Swept`.
3. Purge three-way `GcMode` transition machinery (`GcMode::Legacy`, `GcMode::MarkSweep`, `GcMode::MarkSweepNoRefs`), `gc-mode` file markers, `audit_marks_against_refs`, and dual-write `add_ref` branches in `hydrate.rs`.
4. Remove `flock(refs/)` locks and `RefsLock` / `RefsDirLock` structs.
5. Inherent `DiskStore` methods directly, removing the single-implementation `Store` trait and `WorkspaceCleaner` generic type propagation on `StoreReclaimer`.

## Files Owned

- `crates/flashwt-store/src/lib.rs`
- `crates/flashwt-store/src/disk.rs`
- `crates/flashwt-store/src/gc.rs`
- `crates/flashwt-store/src/error.rs`
- `crates/flashwt-cli/src/hydrate.rs`
- `crates/flashwt-cli/src/gc.rs`
- `crates/flashwt-store/tests/store.rs`

## Done When

- [ ] All per-blob `refs/` directory writes, reads, and locks are removed from `flashwt-store`.
- [ ] `StoreReclaimer` and `compute_marks` exclusively own store garbage collection.
- [ ] Three-way `GcMode` migration branching is eliminated in favor of direct Mark-and-Sweep.
- [ ] `DiskStore` exposes inherent methods without the redundant `Store` trait.
- [ ] Store unit and integration tests compile and pass without refcount dependencies.
