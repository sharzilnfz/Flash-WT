# 08: Public benchmark suite

**What to build:** A script that reproduces the macOS-versus-Linux scenario
numbers: plain `git worktree add` plus fresh dependency install versus our
tool, on identical project fixtures. Produces a table of results suitable for
the README and launch post, so claims are verifiable by anyone.

**Blocked by:** 05 (wire hydration through store).

**Status:** ready-for-human

- [x] One command runs the full comparison and prints a results table
- [x] Fixtures match the published benchmarks' shape (thousands of small files)
- [x] Results include cold and warm worktree creation times
- [x] Suite runs unattended on macOS and Linux CI

## Comments

Implemented on `fleet/08-benchmarks`:

- `benchmarks/run.sh`: one command. Builds `wt` (or honors `WT_BIN`),
  generates a fixture repo in a throwaway tempdir, times plain
  `git worktree add` + fresh install (fixture tree written file by
  file — what an install does at the FS level) against `wt create`,
  cold (empty store) and warm (shared populated store). Every timed
  run is verified against the source fixture (byte-compare with
  `--verify`, file-count always) so unattended CI can't ship a fast
  wrong number. Prints a markdown table plus raw per-run times.
- `benchmarks/fixture.sh`: deterministic node_modules-shaped
  generator (default 4000 files / 500 package dirs), no network,
  works on stock bash 3.2 (macOS) and Linux.
- `.github/workflows/bench.yml`: `--quick --verify` smoke run on
  macos-latest + ubuntu-latest.

Verified locally on macOS arm64 (Darwin 25.6.0), full mode: baseline
cold 9.5s / warm 9.0s; wt cold 15.0s / warm 15.9s. Honest finding for
the launch post: hydration is currently store-ingest + full rewrite
(ticket 05's materialize writes every byte), so it loses to baseline
until clonefile-backed materialization is wired in. The suite will
show that improvement with zero changes to itself once it lands.
