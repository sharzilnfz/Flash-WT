# 10: Test Suite Consolidation and Benchmark Unification

Status: ready-for-agent

Blocked by:
- 01: Release Packaging, Homebrew Formula & Linux ARM64 Support
- 02: Purge Legacy Store Refcounting and GC Mode Transitions
- 03: Simplify Snapshot Index and Unify Store Codecs
- 04: Tree Ingestion Deepening in Store Package
- 05: Copy Engine, Backend Safety & Materializer Simplification
- 06: CLI Dead Code Elimination, Native Clap Aliases & Dependency Pruning
- 07: Deep Presentation and Formatting Module
- 08: Deep Workspace Lifecycle Module
- 09: Shell Autocompletions and Documentation Realignment

## Problem

Twenty-six separate integration test files in `crates/flashwt-cli/tests/` cause excessive Cargo linking overhead and slow local development feedback loops. Fixture setup routines (`TestFixture`, `fn git`) are duplicated across test files. Unoptimized debug-mode benchmark tests synthesize tens of thousands of real files on disk, causing CI test runs to drag on for several minutes. Benchmark scripts (`benchmarks/run.sh`, `benchmarks/v2-bench.sh`) duplicate stage timing and verification logic.

## Work

1. Consolidate the 26 separate integration test files in `crates/flashwt-cli/tests/` into ~5 logical test suites (e.g. `commands.rs`, `snapshots.rs`, `gc.rs`, `isolation.rs`, `format.rs`, `completions.rs`) to slash Cargo linking overhead.
2. Centralize duplicate `TestFixture` and `fn git` implementations across test files into `crates/flashwt-cli/tests/common/mod.rs`.
3. Adjust debug-mode test fixture sizes in `tests/demo.rs` and `tests/cli.rs` (or support test-environment scaling) so unoptimized test runs do not hang for minutes copying 40,000 files.
4. Consolidate duplicate stage-timing parsing and tree verification logic in `benchmarks/run.sh` and `benchmarks/v2-bench.sh` by delegating to `benchmarks/eval.sh`.
5. Add process liveness checks in `benchmarks/chaos.sh` to ensure SIGKILL fault injection tests do not race past short test runs on fast CPUs.

## Files Owned

- `crates/flashwt-cli/tests/`
- `crates/flashwt-cli/tests/common/mod.rs`
- `benchmarks/run.sh`
- `benchmarks/v2-bench.sh`
- `benchmarks/eval.sh`
- `benchmarks/chaos.sh`

## Done When

- [ ] `flashwt-cli` integration tests are grouped into consolidated suites with fast link times.
- [ ] Shared fixture constructors in `tests/common/mod.rs` eliminate duplicate test helper declarations.
- [ ] Full integration test suite completes under 30 seconds in debug mode.
- [ ] Benchmark parsing and tree verification are unified under `eval.sh`.
- [ ] All test and benchmark suites pass with 100% fidelity.
