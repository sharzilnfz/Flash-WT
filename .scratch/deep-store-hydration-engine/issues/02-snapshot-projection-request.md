# 02: Snapshot Projection Request Consolidation in wt-store

**What to build:** Replace the 15-argument primitive parameter signature on `SnapshotProjectionEngine::hydrate` with a single typed `SnapshotProjectionRequest` and structured `SnapshotOutcome`. Encapsulate selection index lookups, lockfile inspection, and delta threshold calculations internally within `wt-store`.

**Blocked by:** None (can start immediately)

**Status:** ready-for-agent

- [x] `SnapshotProjectionEngine::hydrate` accepts a single typed `SnapshotProjectionRequest` struct instead of 15 primitive arguments
- [x] Internal selection index queries, ring updates, and delta threshold evaluations remain encapsulated in `wt-store`
- [x] `SnapshotOutcome` carries complete timing, cloned unit metrics, and diagnostic failure records
- [x] Existing snapshot unit and integration tests compile and pass with the consolidated request structure
