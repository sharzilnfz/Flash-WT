# Spec: performance hardening

Branch: `arch/hardening-and-simplify`. Findings come from a post-refactor
performance audit against `master`. Scale model: scenario D in
`benchmarks/run.sh` (40k files, node_modules shape, mostly unique on cold
ingest).

## Decisions

1. Blob puts return to master's durability level: write-temp + atomic rename,
   no fsync. Rationale: blobs are written from live sources during ingest;
   losing one costs a re-put or a heal-and-retry (`MissingBlob` path already
   exists); per-blob fsync violates `fsutil.rs`'s own stated cost model
   ("once per create/publish, not per blob"); at ~1ms per F_FULLFSYNC this
   adds minutes to a cold create of 40k files. Record the decision in a new
   ADR (0007) so future audits don't re-add it. Mirrors, snapshot metadata,
   and gc-mode KEEP their durable writes — those guard GC roots and validity
   proofs.
2. Refcount updates batch: one flock acquisition and one pass over the batch,
   not per-blob lock/read/write/rename cycles.
3. Selection index skips the save when `record_hit` changed nothing.

## Tickets

- `issues/01-blob-put-best-effort-durability.md`
- `issues/02-batched-refcount-updates.md`
- `issues/03-snapindex-skip-noop-save.md`
- `issues/04-manifest-serialize-once.md`

Filed for later, not in this pass:
- Parallel per-file placement in the fallback ladder (needs A/B against
  run.sh scenario D materialize column; changes failure-order semantics).
- Paranoid mode double-hashes freshly placed files (opt-in mode, no harness).
