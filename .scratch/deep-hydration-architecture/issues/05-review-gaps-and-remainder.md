# Issue 05: Review gaps and remainder from C1-C4 deepening slices

Status: ready-for-agent

## Context

Candidates C1-C4 landed as an uncommitted worktree diff on top of HEAD `dfc8c72`.
Lib tests are green: 36 passed in `flashwt-cli`, 23 passed in `flashwt-copy`, 83 passed in `flashwt-store`.
The end-of-run two-axis code review found zero hard Standards breaches but real Spec partials plus three Standards smells.
This ticket owns every remaining gap so the deepening can be finished later in small verifiable units.

Spec sources: `/var/folders/y9/hnkm2lv91n5chc4116wp_hf40000gn/T/architecture-review-1788381666.html` candidates 01-04, plus `.scratch/deep-hydration-architecture/spec.md`.

## Requirements

### C1 remainder: ingest still lives in CLI
- Move `ingest_tree` behind `DiskStore::hydrate` so CLI keeps only `collect_matches`, printing, and timings.
- Blocker named in run: the volatile-cache exclude closure. Thread it into the store as data, not a callback, or record why it stays out.
- Narrow the internal 14-field `SnapshotProjectionRequest` once ingest moves.
- Route the empty-worktree mirror publish through the same store entry point instead of originating in CLI.

### C2 remainder: WorkspaceEngine not deepened
- Replace free helpers `remove_worktree_files` and `cleanup_pending` in `crates/flashwt-cli/src/gc.rs` with `WorkspaceEngine` methods that own remove, prune, branch delete, size, and fs sweep.
- Unify the three divergent retirement sequences: `gc::remove`, sweep dead leases, `ScratchGuard::cleanup`.
- Fix the stale ordering comment in `crates/flashwt-cli/src/gc.rs:18-25` to match actual parse-first, remove-then-release order.

### C4 remainder: six maps remain
- Finish the unify: `ingest_tree` returns `Manifest` directly and `Ingested` with six parallel maps goes away.
- Delete `manifest_from_parts` once the direct return lands. It currently takes eight travelling params and carries `#[allow(clippy::too_many_arguments)]`.
- Delete the uncalled `into_snapshot_manifest` forwarder or give it its first caller. No second publish path: fold `publish_manifest` into `stage_and_publish` or record why both exist.

### Correctness gaps
- Restore parallel verify: hydrate currently verifies blobs serially before `materialize_batch`. Old code verified inside workers. Put verify back inside the batch or present measurements justifying serial.
- Make dir-mode handling as strict as file-mode handling, or document the asymmetry. Files error on missing mode or size. Dirs fall back with `unwrap_or`.
- Stop swallowing cleanup errors: `cleanup_pending` ignores git and rm failures, then `sweep_objects` marks. Surface failures so a failed rm cannot read as live on retry.
- Document the two-phase sweep protocol (`sweep_leases`, caller deletions, `sweep_objects`, caller byte counts) at the call site. It is new mechanism the spec did not ask for.

### Standards smells
- Data Clumps: `manifest_from_parts` eight params. Bundle behind `&Ingested` or finish C4 direct return.
- Duplicated Code: `remove_worktree_files` vs `cleanup_pending` share remove plus prune plus `remove_dir_all`. Extract one helper.
- Speculative Generality: `into_snapshot_manifest` has no caller. Delete or use.

## Files Owned
- `crates/flashwt-store/src/hydrate.rs`
- `crates/flashwt-cli/src/hydrate.rs`
- `crates/flashwt-store/src/ingest.rs`
- `crates/flashwt-store/src/snapshot/projection.rs`
- `crates/flashwt-store/src/snapshot/publish.rs`
- `crates/flashwt-store/src/gc.rs`
- `crates/flashwt-cli/src/gc.rs`
- `crates/flashwt-copy/src/materialize.rs`
- `crates/flashwt-store/tests/hydrate.rs`
- `crates/flashwt-copy/tests/engine.rs`

## Acceptance
- `cargo test --lib` stays green across all three crates.
- Targeted suites stay green: store hydrate tests, CLI `create_hydrates`, lockfile tests, snapshot tests, storage tests, gc tests, command tests, presentation tests.
- `cargo clippy -p flashwt-copy -p flashwt-store --all-targets` clean.
- No JSON envelope change. No `Manifest` wire-format change. Ledger stays 3-column `rel blob id` everywhere with single mirror publish per dir.
- Each bullet above lands as its own small commit so a single revert undoes one slice.

## Comments
- Filed after the C1-C4 run. Standards axis: zero hard breaches, three smells. Spec axis: three partials, two creeps, three wrong-looking items. Worst items: serial verify parallelism loss and `manifest_from_parts` data clump.
