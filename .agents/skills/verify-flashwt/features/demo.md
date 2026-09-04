# Zero-setup demonstration and benchmark

`flashwt demo` (and its alias `flashwt test-drive`) executes an automated end-to-end benchmark in a self-contained temporary sandbox: it synthesizes a realistic 800-file project across 8 packages, warms the store with an untimed cold ingest, compares standard recursive file copying against warm `flashwt` Copy-on-Write snapshot hydration, verifies mutation isolation, and prints a performance scorecard.

## Sub-features

- `demo-synthetic-fixture` synthesizes 800 files across 8 packages (JavaScript, TypeScript, declaration files, and manifests) without external dependencies. Bulk package content is shared verbatim across packages, mimicking duplicated transitive dependencies and letting the content-addressed store dedupe blobs.
- `demo-cold-warmup` populates the store with an untimed cold ingest so the timed step measures a warm snapshot hit, never cold ingestion.
- `demo-baseline-benchmark` measures multi-threaded standard recursive filesystem copying time and byte duplication.
- `demo-cow-benchmark` measures warm `flashwt` Copy-on-Write snapshot-hit hydration time and block-sharing efficiency.
- `demo-mutation-isolation` verifies that mutating a hydrated worktree file does not alter the donor repo or store blobs.
- `demo-scorecard` calculates speedup ratio and renders an aligned performance scorecard in human mode.
- `demo-cleanup` tears down all temporary repositories, worktrees, and files before exit.

## How to get to it (user POV)

- Run `flashwt demo` or `flashwt test-drive` in any terminal to watch the 5-step test drive and view the terminal scorecard.
- Run `flashwt demo --json` to receive full benchmark measurements as a single-line JSON envelope.

## Driving it with the shell fixture

Preconditions:

- `FLASHWT_BIN` executable and available.
- Can run anywhere (no existing git repository required; creates its own temporary fixture).

- **Run demo benchmark.** `flashwt demo --json`. Envelope `status` is `ok`; `command` is `demo`; `data.files_count` is in `700..900`; `data.baseline_copy_duration_ms` is present; `data.flashwt_hydration_duration_ms` is present; `data.speedup_ratio` is greater than `0.0`; `data.hydration_method` is `"clone"`; `data.bytes_shared_cow` is greater than `0` with `data.bytes_copied` of `0` on a CoW hit; `data.isolation_verified` is `true`; `data.cleaned_up` is `true`.
- **Verify human scorecard output.** Run `flashwt demo` without `--json`. Verify stdout contains: `Step 1/5: Synthesizing realistic fixture...`, `Step 2/5: Warming store (cold ingest, one-time cost, untimed)...`, `Step 3/5: Benchmarking standard filesystem recursive copy (baseline)...`, `Step 4/5: Benchmarking flashwt warm hydration...`, `Step 5/5: Verifying mutation isolation and cleaning up...`, `PERFORMANCE SCORECARD`, `Warm Hydration`, and `ALL CHECKS PASSED (5/5)`.
- **Verify cleanup.** `data.cleaned_up` is `true`, and temporary benchmark directories are completely removed from the filesystem.
- **Proof.** Save the JSON envelope and scorecard text to `artifacts/verify-flashwt/<run-id>/`.

## Gotchas

- `demo` needs roughly 10 MB of temporary disk space for the 800-file fixture and completes in a few seconds.
- On non-APFS filesystems (e.g. Linux without reflink support or tmpfs), the hydration step falls back to byte copies; the scorecard then reports duplicated bytes with a fallback note instead of CoW savings, but all 5 steps still succeed and pass.
- `flashwt test-drive` is a direct alias for `flashwt demo` and executes the identical 5-step pipeline.
