# Spec: Automated Verification and Evaluation Rig

Status: ready-for-agent

## Problem Statement

Developers and autonomous coding agents modifying `wt` need a fast, deterministic, and fully automated way to verify that changes improve or maintain performance without introducing silent regressions, filesystem corruption, or storage bloat.

Today, evaluation relies on manual invocation of separate shell scripts (`benchmarks/run.sh` and `benchmarks/v2-bench.sh`), visual inspection of terminal output, and manual baseline comparisons. There is no unified harness that checks out two git revisions, executes a standardized evaluation matrix across multiple ecosystems, isolates true physical APFS disk allocation, injects fault conditions, and programmatically gates merge-readiness with structured statistical pass/fail assertions.

## Solution

An automated verification and evaluation rig that autonomous agents and CI pipelines can execute in a single command. The rig automates:

1. **Differential Evaluation**: Compiles and compares a baseline binary and a candidate binary across standardized scenarios.
2. **Multi-Axis Metrics Collection**: Captures wall-clock latency, fine-grained `wt-stage` breakdowns, volume-level physical disk deltas, and process memory.
3. **Triple-Axis Parity Verification**: Validates byte-for-byte fidelity, POSIX file modes/executable bits, and symlink target resolution after every hydration.
4. **Multi-Ecosystem Fixture Coverage**: Generates realistic fixtures for JavaScript (`node_modules`), Rust (`target/`), Python (`.venv`), and concurrent agent fan-out workloads.
5. **Volume-Level Storage Accounting**: Isolates true private physical disk consumption from APFS cloned block sharing using volume-level free space probes.
6. **Automated Chaos and Resilience Testing**: Injects process termination (`SIGKILL`) across critical store mutation phases to verify crash safety and self-healing.
7. **Regression Gating and Report Card Generation**: Evaluates results against configurable regression thresholds and emits machine-readable JSON artifacts plus PR-ready markdown summary cards.

## User Stories

1. As an autonomous coding agent implementing a performance optimization, I want to run a single evaluation command against `main`, so that I receive immediate, statistical before-and-after proof of my change.
2. As a CI workflow author, I want the evaluation harness to exit with non-zero status when any stage time regresses beyond configured thresholds, so that performance regressions cannot merge silently.
3. As a maintainer reviewing pull requests, I want a standardized markdown report card attached to PRs, so that I can review cold, warm, and incremental timings side-by-side with baselines.
4. As an engineer refactoring store internals, I want automated triple-axis fidelity checks after every test run, so that missing symlinks or altered file modes are caught immediately.
5. As an agent testing copy backends, I want true physical disk measurements that separate APFS cloned block sharing from dirty private blocks, so that I do not rely on misleading `st_blocks` sums.
6. As a developer using `wt` on non-JS projects, I want the evaluation matrix to test Rust build artifacts and Python virtual environments, so that cross-ecosystem hydration speed is verified.
7. As an agent testing garbage collection changes, I want the harness to measure disk space reclamation across multiple create-modify-remove cycles, so that storage leaks are detected automatically.
8. As a developer working in multi-agent workflows, I want concurrent worktree creation benchmarks simulating 10 to 50 parallel agents, so that lock contention and race conditions are identified under load.
9. As a safety-conscious engineer, I want automated SIGKILL fault injection during ingest, snapshot publishing, and GC sweeps, so that crash resilience is verified continuously.
10. As an autonomous agent generating benchmarks, I want structured JSON output containing system metadata and statistical distributions (median, p95, IQR), so that results can be analyzed programmatically.
11. As a contributor working in offline or constrained environments, I want isolated temp-directory execution that never touches the host machine's live store or git configurations, so that tests run safely anywhere.
12. As a developer evaluating snapshot caching, I want targeted benchmarks for tree poisoning and small dependency bumps, so that v2 incremental rebuild efficiency is measured accurately.

## Implementation Decisions

- **Single CLI Subcommand & Standalone Harness**: The evaluation rig is accessible both as an internal CLI command (`wt eval`) and as a standalone headless runner, operating strictly against isolated temporary roots (`WT_STORE` and throwaway git repositories).
- **Structured JSON Telemetry Schema**: All benchmark passes produce a unified JSON document capturing machine metadata (OS, kernel, CPU, architecture), commit SHAs, per-scenario latency distributions (mean, median, p95, stdev, IQR), per-stage timings, storage metrics, and fidelity verification status.
- **Volume-Level APFS Probe**: Storage measurements use filesystem volume free-space queries (`statvfs`) before and after operations on isolated test mounts/directories, providing accurate private block consumption without APFS clone sharing bias.
- **Multi-Ecosystem Fixture Generators**: Reusable generator modules produce deterministic fixtures for:
  - Scenario JS: 40k files, 800 packages, 96% deduplication, symlinks, executables.
  - Scenario Rust: Multi-crate workspace `target/` tree with incremental compilation artifacts and dependency rlibs.
  - Scenario Python: Python `.venv` with site-packages and nested binary symlinks.
  - Scenario Fan-Out: 10 to 50 concurrent worktree creations on a shared store.
- **Automated Differential Orchestrator**: The runner automates worktree checkout of baseline and candidate git revisions, builds release binaries, executes randomized or interleaved warm/cold test iterations to minimize cache bias, and computes delta statistics.
- **Configurable Regression Policy**: Configurable thresholds (e.g. 5% latency regression budget, 0% fidelity error tolerance, 0% uncollected store leak budget) determine pass/fail verdicts.
- **Fault Injection Engine**: A dedicated chaos test harness spawns worker processes executing `wt create` and `wt sweep`, fires timed signals (`SIGKILL`, `SIGTERM`) at precise execution stages, and verifies that subsequent operations succeed with zero corrupted data.

## Testing Decisions

- **Black-Box Testing Boundary**: The evaluation rig tests the software exclusively through the public CLI interface and filesystem side effects, verifying externally observable behavior rather than internal functions.
- **Triple-Axis Parity Invariant**: Every benchmark iteration includes automatic verification of:
  1. Regular file byte equality (`diff -r` or SHA-256 comparison).
  2. POSIX file mode and permission bits equality.
  3. Symlink target path identity.
- **Statistical Significance**: Benchmarks enforce a minimum sample size (N >= 5) with median and IQR reporting to filter out OS scheduler noise and background I/O interference.
- **Prior Art**: Builds on the foundation of `benchmarks/run.sh`, `benchmarks/v2-bench.sh`, and `crates/wt-cli/tests/gc.rs` chaos tests, elevating them into an integrated automated framework.

## Out of Scope

- Remote or distributed cross-machine benchmarking.
- Cloud telemetry ingestion or centralized SaaS dashboard reporting.
- Windows ReFS benchmarking (deferred to future Windows support).
- Automatic kernel tuning or host OS configuration modifications.

## Further Notes

- The rig enables autonomous coding agents to run full-loop hillclimbing: formulate optimization hypothesis -> apply code change -> run automated eval -> review differential report card -> commit or revert.
- The JSON output schema is designed to be compatible with standard CI benchmark action formatters.
