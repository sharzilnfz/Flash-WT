# Ticket 08: Benchmark and Packaging Streamlining

Status: ready-for-agent

## Description

Benchmark scripts (`benchmarks/run.sh`, `eval.sh`, `v2-bench.sh`) duplicate clock, stage parsing, and storage calculation logic. `install.sh` and `smoke-install.sh` contain redundant shell completion loops and platform branching. GitHub Actions workflows contain redundant build steps.

## Requirements

1. **Benchmark Helper Unification**:
   - Extract shared timing parsers and storage calculators into `benchmarks/eval_metrics.sh` and `benchmarks/eval_storage.sh`.
   - Source shared helpers in `run.sh` and `v2-bench.sh`, removing legacy tolerance counters and redundant parsing awk scripts.

2. **Streamline Distribution Scripts**:
   - Parameterize shell completion discovery loops in `install.sh` over an array of candidate target paths.
   - Unify `sha256sum` vs `shasum` utility detection into a shared helper function in `install.sh` and `smoke-install.sh`.

3. **Streamline CI Workflows**:
   - In `.github/workflows/ci.yml`, remove redundant standalone `cargo build` steps that duplicate work performed by `cargo test`.
   - In `.github/workflows/release.yml`, remove redundant `setup-version` job and extract the tag version directly within matrix jobs.

## Verification

- Run `bash scripts/smoke-install.sh` to verify installer and packaging flows.
- Run `bash benchmarks/eval.sh --quick` to verify benchmark metric parsing.
