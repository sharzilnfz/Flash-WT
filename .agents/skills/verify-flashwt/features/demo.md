# Zero-setup demonstration and benchmark

`flashwt demo` (and its alias `flashwt test-drive`) executes an automated end-to-end benchmark in a self-contained temporary sandbox: it synthesizes a realistic 10,000-file project, compares standard recursive file copying against `flashwt` Copy-on-Write hydration, verifies mutation isolation, and prints a performance scorecard.

## Sub-features

- `demo-synthetic-fixture` synthesizes 10,000 files across 100 packages (JavaScript, TypeScript, declaration files, and manifests) without external dependencies.
- `demo-baseline-benchmark` measures multi-threaded standard recursive filesystem copying time and byte duplication.
- `demo-cow-benchmark` measures `flashwt` Copy-on-Write hydration time and block-sharing efficiency.
- `demo-mutation-isolation` verifies that mutating a hydrated worktree file does not alter the donor repo or store blobs.
- `demo-scorecard` calculates speedup ratio and renders an aligned performance scorecard in human mode.
- `demo-cleanup` tears down all temporary repositories, worktrees, and files before exit.

## How to get to it (user POV)

- Run `flashwt demo` or `flashwt test-drive` in any terminal to watch the 5-step test drive and view the terminal scorecard.
- Run `flashwt --json demo` to receive full benchmark measurements as a JSON envelope.

## Driving it with the shell fixture

Preconditions:

- `FLASHWT_BIN` executable and available.
- Can run anywhere (no existing git repository required; creates its own temporary fixture).

- **Run demo benchmark.** `flashwt --json demo`. Envelope `status` is `ok`; `command` is `demo`; `data.files_count` is `10000`; `data.baseline_copy_duration_ms` is present; `data.flashwt_hydration_duration_ms` is present; `data.speedup_ratio` is >= 1.0; `data.isolation_verified` is `true`; `data.cleaned_up` is `true`.
- **Verify human scorecard output.** Run `flashwt demo` without `--json`. Verify stdout contains: `Step 1/5: Synthesizing realistic fixture...`, `Step 4/5: Verifying Copy-on-Write mutation isolation...`, `PERFORMANCE SCORECARD`, and `ALL CHECKS PASSED (5/5)`.
- **Verify cleanup.** `data.cleaned_up` is `true`, and temporary benchmark directories are completely removed from the filesystem.
- **Proof.** Save the JSON envelope and scorecard text to `artifacts/verify-flashwt/<run-id>/`.

## Gotchas

- `demo` requires enough temporary disk space for 10,000 small files (approx. 50-100 MB).
- On non-APFS filesystems (e.g. Linux without reflink support or tmpfs), the hydration step falls back to byte copies; speedup ratio will be closer to 1.0x to 2.0x, but all 5 steps will still succeed and pass.
- `flashwt test-drive` is a direct alias for `flashwt demo` and executes the identical 5-step pipeline.
