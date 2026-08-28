# 11: Manifest logical size accounting and dual-budget snapshot sweep

**What to build:** Compute the sum of unique blob sizes during snapshot ingestion and record the total in `manifest.tsv`. Extend `wt sweep` to enforce dual budget limits using both snapshot count caps (`WT_SNAPSHOT_CAP`) and maximum disk byte limits (`WT_MAX_SNAPSHOT_BYTES`). Protect recently published snapshots younger than the grace period against budget eviction to prevent cache thrashing under concurrent workloads.

**Blocked by:** 10: Append-only snapshot write-ahead journal and sweep compaction.

**Status:** ready-for-agent

- [ ] Ingestion calculates unique uncompressed blob size totals and records them in `manifest.tsv`.
- [ ] `wt sweep` evaluates cumulative snapshot sizes using precomputed manifest values.
- [ ] Disk byte budget limits configured via `WT_MAX_SNAPSHOT_BYTES` evict oldest unreferenced snapshots once thresholds are exceeded.
- [ ] Snapshot count cap pruning (`WT_SNAPSHOT_CAP`) operates alongside byte limits.
- [ ] Anti-thrashing grace window protects young snapshots from eviction even when budgets are tight.
- [ ] Garbage collection tests verify dual-budget enforcement and anti-thrashing protection.
