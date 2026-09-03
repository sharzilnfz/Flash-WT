# 03: Deep Hydration Engine and Storage Ledger in flashwt-store

**What to build:** Implement `DiskStore::hydrate(&mut self, req: HydrationRequest) -> Result<HydrationReceipt>`. Encapsulate the complete hydration workflow: lockfile fast-path evaluation, snapshot projection, fallback verified file materialization via `CopyEngine`, atomic sidecar ledger (`flashwt-hydrated.tsv`) persistence, and garbage collection mirror publication. Storage invariants and ledger formats reside entirely within `flashwt-store`.

**Blocked by:** 01: Unified Copy Engine in flashwt-copy, 02: Snapshot Projection Request Consolidation in flashwt-store

**Status:** ready-for-agent

- [x] `DiskStore::hydrate` coordinates the complete hydration lifecycle behind a single method call
- [x] Lockfile fast-path evaluation and snapshot projection are attempted through `SnapshotProjectionEngine`
- [x] Fallback materialization leverages `CopyEngine` across a worker pool with blob fingerprint verification
- [x] The sidecar hydration ledger (`flashwt-hydrated.tsv`) is written directly into the worktree git directory with buffered I/O
- [x] The store-local mirror is published atomically upon successful hydration
- [x] In-crate integration tests verify end-to-end hydration, snapshot hits, fallback ladder placement, and mirror persistence
