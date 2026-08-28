# 09: Tiered lockfile validation with mutable dependency safety classification

**What to build:** Add dependency safety classification to lockfile validation during `wt create`. When a lockfile contains local mutable references (`file:`, `link:`, `workspace:`, or unpinned git branch references), bypass the fast path and run full directory ingestion. When all dependencies are pinned, compare the lockfile SHA-256 with the snapshot manifest header and check root directory modification times, returning an O(1) hit without walking disk trees.

**Blocked by:** None (can start immediately).

**Status:** completed

- [x] Lockfile parser identifies mutable dependency references (`file:`, `link:`, `workspace:`, unpinned git branches).
- [x] Lockfiles with mutable dependencies trigger full directory ingestion and bypass the fast path.
- [x] Lockfiles with strictly pinned dependencies evaluate lockfile SHA-256 against snapshot manifest headers.
- [x] Snapshot validation returns O(1) cache hits when lockfile hashes match and root directory timestamps are unchanged.
- [x] Tests verify immediate fast-path hits on pinned lockfiles and correct fallback ingestion on local mutable paths.
