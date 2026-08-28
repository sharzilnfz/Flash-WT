# 01: blob puts are best-effort durable again

Status: ready-for-agent

`DiskStore::put` (crates/wt-store/src/disk.rs ~343-354) currently does
sync_all + parent-dir fsync per NEW blob. On a cold ingest of 40k unique
files that is ~80k device flushes on the hot loop. It also contradicts the
cost model comment at the top of fsutil.rs ("once per create/publish, not
per blob").

## Work

1. Revert put() to write-temp + rename (master's semantics). Keep the
   permission normalization. No fsync on the blob path.
2. Fix the misleading comments: fsutil.rs cost-model text stays true once
   put() stops violating it; add a short doc on put(): "rebuildable cache
   entry; atomic but not crash-durable — see ADR-0007".
3. Write docs/adr/0007-blob-durability-is-best-effort.md following the style
   of existing ADRs (read ADR-0004 first; this extends its "a kill can only
   leak reclaimable cache data" argument to crash consistency). State what
   IS durable: mirrors, snapshot manifests/.complete, gc-mode. State the
   residual risk: power loss between rename and flush can lose recent blobs;
   recovery paths are re-ingest and the MissingBlob heal-and-retry.
4. Update CONTEXT.md glossary if needed (only if it currently implies blob
   durability; check).

## Done when

- cargo test --workspace green; clippy -D warnings green (macOS AND
  x86_64-unknown-linux-gnu cross-check); fmt clean.
- No fsync call remains on the per-blob path (grep verify).
- ADR committed.

## Comments

Recon from the halted first attempt (no code was changed): regression lives
in `DiskStore::put`, crates/wt-store/src/disk.rs ~343-354 (`sync_all` +
`sync_parent_dir` per NEW blob). The cost-model comment it violates is at
the top of fsutil.rs. Warm creates are unaffected (put short-circuits on
`path.exists()`).

For benchmark numbers later: `benchmarks/run.sh --scenario d --runs 1` runs
the large-fixture suite once (baseline/cow/cold/warm; no single-cell
selection without editing the script). Stage-level cold ingest timings need
`WT_TIMING=1`.
