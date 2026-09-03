# Ponytail Simplification & Whole-Repository Architecture Cleanup

Status: ready-for-agent

## Problem Statement

Following the completion and hardening of the v0.1.0 launch readiness requirements, the codebase contains approximately 3,400 lines of over-engineered boilerplate, single-caller wrappers, dead pre-v0.1 compatibility shims, duplicate test fixtures, and fragmented test binaries.

Specifically:
1. **Compilation & Link-Time Friction**: The workspace compiles 26 discrete integration test executables in `flashwt-cli`, 5 in `flashwt-store`, and 2 in `flashwt-copy`. Each binary independently links large dependencies (`clap`, `serde`, `libc`), causing CI test times to exceed 5 minutes and bloating debug artifact caches.
2. **Maintenance Overhead & Redundancy**: Core routines (such as entry ingestion across macOS and portable platforms, JSON envelope serialization, directory placement, and flock acquisition) are repeated across multiple files.
3. **Speculative Abstractions**: Layers like `Safety::UnsafePending` in `flashwt-copy`, `HydrationFilter` struct wrappers in `flashwt-cli`, and single-implementation `Store` / `WorkspaceCleaner` traits in `flashwt-store` add indirection without providing multi-backend flexibility.
4. **Duplicate Fixture & Benchmark Harnesses**: Test files independently implement custom Git runners, temporary fixtures, and store footprint assertions; benchmark scripts duplicate clock, stage parsing, and storage calculation logic.

Developers and agent contributors face increased cognitive load when navigating redundant layers, while CI pipelines pay unnecessary build and test overhead.

---

## Solution

Execute a structured, zero-breaking-change ponytail cleanup across `crates/flashwt-store`, `crates/flashwt-cli`, `crates/flashwt-copy`, integration tests, benchmarks, and distribution scripts:

1. **Delete Dead Code & Speculative Layers**: Remove unused wrapper structs (`HydrationFilter`), dead method forwarders, obsolete pre-v0.1 legacy refcount tests, and speculative safety gates.
2. **Collapse Single-Caller Forwarders & Aliases**: Use native Clap alias attributes for duplicate subcommand variants, inline single-caller wrappers directly into call sites, and remove dead re-export modules.
3. **Leverage Standard Library & Platform Primitives**: Replace hand-rolled byte decoders, ancestor traversals, and buffer copying loops with standard Rust equivalents (`u32::from_le_bytes`, `Path::ancestors()`, `std::io::copy`, `derive(Default)`).
4. **Consolidate Integration Test Binaries**: Merge 26 discrete test files in `flashwt-cli/tests/` into 5 cohesive module targets with a unified `common::Fixture` harness, reducing Cargo link overhead and cutting CI runtime by over 80%.
5. **Unify Benchmark Metrics & Packaging Helpers**: Share metric parsing and volume storage calculation scripts across benchmark runners, and streamline shell installer completion discovery loops.

All public CLI commands, JSON envelope schemas, hydration performance, and safety invariants remain strictly preserved.

---

## User Stories

1. As a CLI user, I want `flashwt` to have zero dead code and no unnecessary crate dependencies like `sha2`, so that binary footprint and compile times are minimized.
2. As a CLI contributor, I want duplicate subcommand variants (`New`/`Create`, `Isolate`/`Scratch`, `TestDrive`/`Demo`) to use native Clap alias attributes, so that command definitions and dispatch logic are concise and maintainable.
3. As a developer writing agent automation, I want consistent NDJSON envelope serialization handled by a centralized helper, so that diagnostic formats and exit code mappings remain strictly uniform across all subcommands.
4. As a developer using `flashwt clean`, I want empty data receipts constructed via standard `Default` derivations, so that early exit paths are concise and consistent.
5. As a developer, I want `CreateGuard` to rely on standard RAII drop semantics for rollback rather than manual method invocations, so that transactional worktree rollback on error is clean and idiomatic.
6. As a store maintainer, I want duplicate tree ingestion logic between macOS bulk walking and portable walking unified, so that ingestion bugfixes and performance enhancements apply everywhere.
7. As a store maintainer, I want snapshot publication variants consolidated into a single options struct, so that the snapshot creation API surface is compact.
8. As a store developer, I want snapshot verification during scrub to reuse core tree verification logic, so that corruption detection rules never diverge.
9. As a store contributor, I want unified file locking helpers for advisory locks, so that lock acquisition and drop semantics are centralized without duplicate flock wrapper structs.
10. As a store contributor, I want single-implementation store traits inlined onto the concrete disk store, so that indirection is removed until multiple storage adapters are required.
11. As a developer using `flashwt-copy`, I want dead batch materialization wrappers and speculative safety gates removed, so that the copy engine has a clear and minimal public API.
12. As a developer on Linux or macOS, I want materializer construction to compute backend and strategy in a unified expression, so that platform selection logic is concise.
13. As an engineer running test suites, I want 26 discrete CLI test binaries consolidated into 5 cohesive module targets, so that Cargo linking overhead and CI test execution time are reduced by over 80%.
14. As an engineer adding new integration tests, I want a shared test fixture helper in `common`, so that repository setup, Git subprocess runners, and store footprint assertions do not need to be re-implemented in every test file.
15. As a maintainer, I want obsolete legacy per-blob refcount test suites removed, so that the test suite reflects mark-and-sweep store mirrors as the single source of truth.
16. As a benchmark author, I want monolithic benchmark runners to share common stage timing and storage calculation scripts, so that performance metrics are parsed consistently.
17. As an installer script maintainer, I want shell completion discovery loops and sha256 checksum platform branching simplified to standard loops, so that installation scripts remain lightweight and auditable.
18. As a CI operator, I want redundant build steps and duplicate test jobs in GitHub Actions workflows eliminated, so that PR validation is fast and resource-efficient.
19. As a developer inspecting code, I want standard library routines like `Path::ancestors`, `u32::from_le_bytes`, and `std::io::copy` used instead of hand-rolled loops, so that code readability and performance are optimal.
20. As a maintainer, I want all public CLI command behavior, JSON envelope schemas, and filesystem hydration performance to remain completely identical before and after cleanup, so that no external consumers experience regression.

---

## Implementation Decisions

### 1. CLI Subcommand Aliasing & Forwarder Inlining
- Use Clap derive `#[command(alias = "...")]` attributes on primary commands (`Create`, `Scratch`, `Demo`) to eliminate duplicated enum variants and duplicate match arms in command dispatch.
- Delete the dead `HydrationFilter` struct wrapper in `hydration_filter.rs` and its trivial aliases; callers use free functions (`load_patterns`, `collect_matches`) directly.
- Delete deprecated forwarder module `manifest.rs` in favor of direct imports from `hydration_filter`.
- Replace scratch ID generation with standard library formatting (`format!("{:08x}", ...)`) or stdlib hasher, dropping the external `sha2` crate dependency from `flashwt-cli`.

### 2. Centralized NDJSON Envelope Emission & Error Mapping
- Extract a unified `emit_json` helper function in `commands/mod.rs` to handle serialization, error logging, and standard output printing across all 10 subcommand dispatch arms.
- Derive `Default` on `CleanData` and `ScratchData` structs to eliminate redundant manual struct initialization in early-return and error paths.

### 3. RAII Rollback Simplification in Transactional Worktree Creation
- Rely strictly on `CreateGuard::drop` RAII semantics for transactional rollback upon errors during store opening, ingestion, and hydration.
- Replace repetitive `match` error handling with the standard `?` operator in `create.rs`.

### 4. Store Ingestion & Placement Deduplication
- Extract a shared `ingest_entry` closure/helper in `flashwt-store` to eliminate duplicated validation cache checks, file stat processing, and CAS blob hashing between macOS `getattrlistbulk` walking and portable recursive walking.
- Unify directory entry placement helpers between absolute and relative directory fd paths in `snapshot/tree.rs`.

### 5. Snapshot Publication Consolidation & Scrub Reuse
- Consolidate combinatorial `publish_snapshot_*` variants into a single options struct with default parameters.
- Reuse `DiskStore::find_snapshot` and `paranoid_verify_tree` in snapshot scrubbing to eliminate duplicate tree traversal and hash verification logic.
- Unify advisory file locking into a single `FlockGuard` helper in `fsutil.rs`, removing triplicate flock wrapper structs.
- Inline single-implementation `Store` and `WorkspaceCleaner` trait methods directly onto concrete implementations.

### 6. Copy Engine & Materializer Simplification
- Delete dead `CopyEngine` batch and single-file materialization wrappers (`materialize_file`, `materialize_files`) and `BatchPlacementReceipt`, directing callers to `Materializer` directly.
- Remove speculative `Safety::UnsafePending` enum variant, `Error::UnsafeBackend`, and associated gating methods.
- Refactor `Materializer::select` to compute `(backend, strategy)` tuples in a concise expression before instantiating the struct once.
- Replace manual heap buffer allocation and read/write loops in `buffered_copy_file` with `std::io::copy`.

### 7. Integration Test Binary Consolidation (26 -> 5 targets)
- Consolidate the 26 discrete test files in `crates/flashwt-cli/tests/` into 5 cohesive module targets:
  - `commands.rs` (CLI command surface, arguments, help, and exit codes)
  - `snapshots.rs` (snapshot creation, lockfile fastpath, incremental rebuilds, APFS clones)
  - `gc.rs` (store mirrors, lease sweep, grace periods, corruption recovery)
  - `storage.rs` (CoW materialization, hardlink safety, cache flows)
  - `presentation.rs` (NDJSON envelopes, diagnostics, human output, completions)
- Delete obsolete legacy refcount tests (`gc.rs` in `flashwt-cli/tests` and refcount lock tests in `flashwt-store/tests/store.rs`) that targeted deleted pre-v0.1 `refs/` plumbing.

### 8. Shared Integration Test Fixture & Assertion Library
- Expand `crates/flashwt-cli/tests/common/mod.rs` to provide a unified `Fixture` struct with shared repository initialization, Git command execution (`fn git`), and store footprint inspection helpers.
- Eliminate duplicate `TestFixture`, `RichFixture`, `V2Fixture`, and `LockfileFixture` definitions across test files.

### 9. Benchmark Script & Metric Parser Unification
- Extract shared stage-timing parsing, high-resolution clocks, and APFS volume storage calculations into `benchmarks/eval_metrics.sh` and `benchmarks/eval_storage.sh`.
- Source shared functions from `run.sh`, `eval.sh`, and `v2-bench.sh`, eliminating redundant timing awk scripts and legacy gap tolerance counters.

### 10. CI Workflow & Shell Installer Streamlining
- Parameterize shell completion search loops in `install.sh` over an array of candidate paths.
- Unify `sha256sum` vs `shasum` platform branching in `install.sh`, `smoke-install.sh`, and `chaos.sh`.
- Remove redundant `cargo build` steps in `.github/workflows/ci.yml` that duplicate work performed by `cargo test`.

---

## Testing Decisions

- **Good Test Criteria**: Tests must verify only external observable behavior (CLI return codes, stdout/stderr envelopes, filesystem directory contents, inode sharing, and storage liveness). Never assert on private internal helper functions or ephemeral state.
- **Modules Tested**:
  - `flashwt-cli` integration test suite across all subcommands (`new`, `clean`, `list`, `hydrate`, `init`, `scratch`, `scrub`, `demo`).
  - `flashwt-store` snapshot projection, validation cache, lockfile discovery, and mark-and-sweep GC.
  - `flashwt-copy` materializer fallback chains, clonefile, reflink, and `copy_file_range`.
- **Prior Art**: Existing integration tests in `crates/flashwt-cli/tests/` (e.g., `json_output.rs`, `cow_materialization.rs`, `clean.rs`) and test fixtures in `tests/common/mod.rs`.

---

## Out of Scope

- **New CLI Features**: No new subcommands, flags, or public options beyond the completed v0.1.0 contract.
- **Breaking Changes to NDJSON Schema**: Version 1 NDJSON envelopes and diagnostic codes must remain identical.
- **APFS Snapshot Protocol Changes**: Core snapshot tree layouts (`snapshots/<hash>/tree/`), `.complete` tokens, and `manifest.tsv` formats remain unchanged.
- **Package Management & Build Resolvers**: `flashwt` remains a tool-agnostic filesystem hydration cache; dependency resolution and compiling remain external.

---

## Further Notes

- Expected net codebase reduction: **~3,426 lines of Rust and shell code**, plus removal of **1 external crate dependency** (`sha2` from `flashwt-cli`).
- Cargo test compile and link time is projected to drop from ~5 minutes to under 25 seconds in local and CI runs.
