# 07: Subtree-partitioned parallel snapshot construction

**What to build:** Refactor cold snapshot creation into two distinct phases to eliminate parent directory lock contention on APFS and ext4. Phase 1 builds directory entries sequentially in sorted order. Phase 2 groups file and symlink entries by parent directory into subtree batches and dispatches them across a worker pool bounded by available CPU parallelism using `std::thread::scope`. Use relative directory operations with `openat` and close handles eagerly to bound file descriptor usage.

**Blocked by:** None (can start immediately).

**Status:** ready-for-agent

- [x] Directory skeleton creation runs sequentially in sorted order before file linking starts.
- [x] File and symlink entries are grouped by parent directory into subtree batches.
- [x] Batches are dispatched across worker threads scoped by `std::thread::scope`.
- [x] Worker tasks link files using relative `openat` operations without contending on shared parent directory locks.
- [x] File descriptors are closed eagerly within each batch.
- [x] Snapshot ingestion benchmarks verify multi-core speedup and tree integrity matching serial ingestion.
