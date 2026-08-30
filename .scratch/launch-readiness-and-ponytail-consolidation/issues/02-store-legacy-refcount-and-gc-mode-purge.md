# 02: Purge Legacy Store Refcounting and Mode Transitions

**What to build:**
Eliminate legacy per-blob refcounting machinery and dual-mode migration states from `wt-store` so that store-local mirrors and mark-and-sweep GC become the sole, unbranched garbage collection model:
1. Delete per-blob refcount file operations (`add_ref`, `release_ref`, `ref_count`, `ref_path`, `write_ref_count`, `read_ref_count`) and `Error::RefCountUnderflow` in `wt-store`.
2. Delete obsolete refcount sweep method `DiskStore::sweep` and `struct Swept`.
3. Purge three-way `GcMode` transition machinery (`GcMode::Legacy`, `GcMode::MarkSweep`, `GcMode::MarkSweepNoRefs`), `gc-mode` file markers, `audit_marks_against_refs`, and dual-write `add_ref` branches in `hydrate.rs`.
4. Remove `flock(refs/)` locks and `RefsLock` / `RefsDirLock` structs.
5. Inherent `DiskStore` methods directly, removing the single-implementation `Store` trait and `WorkspaceCleaner` generic type propagation on `StoreReclaimer`.

**Blocked by:** None (can start immediately).

**Status:** ready-for-agent

- [ ] All per-blob `refs/` directory writes, reads, and locks are removed from `wt-store`
- [ ] `StoreReclaimer` and `compute_marks` exclusively own store garbage collection
- [ ] Three-way `GcMode` migration branching is eliminated in favor of direct Mark-and-Sweep
- [ ] `DiskStore` exposes inherent methods without the redundant `Store` trait
- [ ] Store unit and integration tests compile and pass without refcount dependencies
