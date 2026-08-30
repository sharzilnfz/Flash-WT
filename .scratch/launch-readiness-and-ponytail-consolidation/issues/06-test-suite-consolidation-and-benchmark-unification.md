# 06: Test Suite Consolidation and Benchmark Unification

**What to build:**
Accelerate the test and benchmarking feedback loop by grouping integration test executables, eliminating duplicate fixture helpers, and standardizing benchmark scripts:
1. Consolidate the 26 separate integration test files in `crates/wt-cli/tests/` into ~5 logical test suites (e.g. `commands.rs`, `snapshots.rs`, `gc.rs`, `isolation.rs`, `format.rs`) to slash Cargo linking overhead.
2. Centralize duplicate `TestFixture` and `fn git` implementations across test files into `tests/common/mod.rs`.
3. Adjust debug-mode test fixture sizes in `tests/demo.rs` and `tests/cli.rs` (or support test-environment scaling) so unoptimized test runs do not hang for minutes copying 40,000 files.
4. Consolidate duplicate stage-timing parsing and tree verification logic in `benchmarks/run.sh` and `benchmarks/v2-bench.sh` by delegating to `benchmarks/eval.sh`.
5. Add process liveness checks in `benchmarks/chaos.sh` to ensure SIGKILL fault injection tests do not race past short test runs on fast CPUs.

**Blocked by:**
- 01: Release Packaging, Homebrew Formula & Linux ARM64 Support
- 02: Purge Legacy Store Refcounting and Mode Transitions
- 03: Simplify Snapshot Index and Unify Store Codecs
- 04: CLI Dead Code Elimination, Native Clap Aliases & Dependency Pruning
- 05: Copy Engine, Backend Safety & Materializer Simplification

**Status:** ready-for-agent

- [ ] `wt-cli` integration tests are grouped into consolidated suites with fast link times
- [ ] Shared fixture constructors in `tests/common/mod.rs` eliminate duplicate `fn git` declarations
- [ ] Full integration test suite completes under 30 seconds in debug mode
- [ ] Benchmark parsing and tree verification are unified under `eval.sh`
- [ ] All test and benchmark suites pass with 100% fidelity
