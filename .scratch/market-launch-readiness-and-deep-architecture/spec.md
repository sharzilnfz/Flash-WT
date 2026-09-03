# Spec: Market Launch Readiness, Deep Architecture & Ponytail Consolidation

Status: ready-for-agent

## Problem Statement

As `flashwt` approaches its v0.1.0 public release, several friction points in distribution packaging, developer experience, architecture depth, accumulated migration baggage, and test performance must be addressed before launch:

1. **Release Packaging Blockers and Platform Support**: Release archive naming in CI workflows (`flashwt-0.1.0-` vs `flashwt-v0.1.0-`), absence of native Linux ARM64 (`aarch64-unknown-linux-gnu`) release binaries, and strict version tag handling in `install.sh` will cause automated release pipelines and Homebrew installation to fail upon tagging.
2. **Missing Shell Autocompletions**: Developers using modern shells (Bash, Zsh, Fish, Elvish, PowerShell) expect tab-completions for subcommands, flags, and branch names out of the box. Currently, typing `flashwt <TAB>` offers no completion candidates.
3. **Fragmented Presentation and Formatting Logic**: Terminal output, byte scaling, duration formatting, number grouping, and aligned receipt tables are independently re-implemented across four distinct command handlers (`create.rs`, `clean.rs`, `list.rs`, `demo.rs`) with subtle mathematical discrepancies.
4. **Shallow Worktree Discovery and Git Lifecycle Seams**: Worktree enumeration, gitdir resolution, porcelain parsing, and merge-base ancestor checks are scattered across command handlers rather than encapsulated behind a cohesive workspace module.
5. **Leaked Ingestion Invariants**: Ingesting directory trees, executing bulk walker syscalls, and maintaining validation cache metadata reside in the CLI crate (`crates/flashwt-cli/src/hydrate.rs`) rather than within the content-addressed storage engine (`flashwt-store`) where storage invariants belong.
6. **Accumulated Migration Baggage and Over-Engineering**: The codebase retains over 2,000 lines of obsolete migration scaffolding, including dual-write per-blob refcounting machinery, multi-file WAL journal index compaction, single-implementation traits, dead forwarder shims, speculative backend safety contracts, and hand-rolled byte codecs.
7. **CLI Boilerplate and Unnecessary Dependencies**: Duplicate subcommand enum variants (`New`, `Isolate`, `TestDrive`) duplicate handler code, repetitive JSON serialization blocks clutter command dispatch, and `flashwt-cli` pulls in direct cryptographic hashing dependencies when the storage engine already encapsulates hashing.
8. **Outdated Launch Documentation**: The primary README documents legacy manual snapshot flags (`FLASHWT_SNAPSHOTS=1`) as prerequisites rather than highlighting automated macOS APFS defaults and the modern 3-verb workflow (`flashwt new`, `flashwt clean`, `flashwt list`, `flashwt demo`).
9. **Integration Test Suite Link Overhead and Execution Drag**: Twenty-six separate integration test binaries in the CLI crate cause excessive Cargo link times, and unoptimized debug-mode benchmark tests synthesize tens of thousands of real files on disk, causing local and CI test runs to drag on for minutes.

## Solution

A targeted, zero-breaking-change consolidation program delivering complete launch readiness across six architectural pillars:

1. **Release Packaging, Multi-Platform Distribution & Documentation**: Standardize release archive naming in CI workflows, add native Linux ARM64 support, normalize version tags in installer scripts, bundle shell completions in Homebrew formulas, and realign the README to the modern 3-verb workflow and automated APFS defaults.
2. **Storage Engine Simplification & Tree Ingestion Deepening**: Purge legacy per-blob refcounting and `GcMode` transitions from `flashwt-store`, simplify snapshot metadata persistence to atomic TSV manifests via tempfile-rename, replace hand-rolled codecs with standard library equivalents, and relocate directory tree ingestion and bulk walking from `flashwt-cli` into `flashwt-store::ingest`.
3. **Copy Engine & Backend Simplification**: Delete speculative backend safety states (`Safety::UnsafePending`), eliminate redundant file materialization delegators and dead methods, replace manual copy loops with `std::io::copy`, and adopt `Path::ancestors()`.
4. **Deep CLI Architecture & Presentation Module**: Establish a dedicated presentation module (`crates/flashwt-cli/src/output.rs`) for uniform byte, duration, count, and table formatting. Deepen `gitops` into a cohesive `workspace` module encapsulating repository detection, gitdir resolution, porcelain parsing, and merge-ancestor validation.
5. **Shell Autocompletions & Developer Experience**: Add `clap_complete` to `flashwt-cli` to provide `flashwt completions <shell>` for Bash, Elvish, Fish, PowerShell, and Zsh. Update `install.sh` to auto-detect active shells and install completion scripts automatically. Convert duplicate subcommand variants to native Clap aliases on `Create`, `Scratch`, and `Demo`. Drop direct `sha2` crate dependency from `flashwt-cli`.
6. **Test Suite Consolidation & Benchmark Hardening**: Group the 26 fragmented CLI integration test executables into ~5 logical test suites to slash Cargo link times. Centralize fixture creation into `tests/common/mod.rs`. Scale debug-mode test fixtures for sub-30-second test runs. Unify benchmark parsing and verification under `benchmarks/eval.sh`, and add process liveness checks to chaos fault injection tests.

## User Stories

1. As a macOS developer installing `flashwt` via Homebrew, I want `brew install flashwt` to download valid binary tarballs from GitHub Releases, so that installation succeeds seamlessly without checksum mismatch errors.
2. As a Linux developer on an ARM64 cloud instance (e.g. AWS Graviton), I want `install.sh` to download a native `aarch64-unknown-linux-gnu` binary, so that I can run `flashwt` without cross-architecture emulation.
3. As a developer running the standalone installer with `FLASHWT_VERSION=0.1.0` (with or without the leading `v`), I want the installer to normalize the tag, so that I never encounter a 404 download error.
4. As a developer evaluating `flashwt` from the README, I want the quickstart to feature modern verbs (`flashwt new`, `flashwt clean`, `flashwt list`, `flashwt demo`), so that I learn the recommended workflow first.
5. As a macOS developer reading the README, I want the documentation to state that directory snapshots and diff rebuilds are active by default on APFS, so that I do not needlessly configure redundant environment variables.
6. As a developer installing `flashwt` on launch day, I want `flashwt completions zsh` to generate shell completion scripts, so that I can tab-complete commands, flags, and branches in my terminal.
7. As a developer running `install.sh`, I want my shell completions to be detected and installed automatically, so that autocompletion works without manual configuration.
8. As a CLI user running `flashwt new`, `flashwt isolate`, or `flashwt test-drive`, I want subcommand aliases to be handled natively by Clap, so that documentation, shell completions, and help messages are unified without boilerplate code.
9. As a developer creating a worktree with `flashwt new`, I want byte counts and duration numbers formatted with clean, standard unit scaling, so that receipts are immediately readable.
10. As a developer inspecting active worktrees with `flashwt list`, I want the output table to use the same unit conversion precision and alignment rules as other commands, so that disk savings and hydration metrics are consistent.
11. As a maintainer, I want git worktree porcelain parsing and path resolution encapsulated in a deep workspace module, so that command handlers do not duplicate raw git command invocations.
12. As a maintainer, I want worktree merge-base status checks centralized in the workspace module, so that cleanup commands can reliably detect merged branches across repositories.
13. As an open-source contributor exploring `flashwt-store`, I want the storage engine to exclusively use the atomic store mirror mark-and-sweep GC model, so that I do not have to navigate obsolete per-blob refcounting code or dual-mode migration states.
14. As an agent orchestrator launching parallel worktrees, I want the snapshot index to use atomic single-file manifest persistence without complex WAL journal file locking, so that lockfile cache lookups remain fast, simple, and crash-resilient.
15. As a non-CLI consumer or background worker, I want to ingest directory trees directly through the store interface, so that content ingestion does not depend on CLI modules.
16. As a developer running `cargo test`, I want integration tests to compile and link in seconds across consolidated test suites, so that my local feedback loop is fast.

## Implementation Decisions

### Decision 1: Release Packaging & Multi-Platform Distribution
- Standardize all release archive naming in CI workflows to `flashwt-v<VERSION>-<TARGET>.tar.gz` to ensure strict alignment with Homebrew formula generation and installer scripts.
- Add `aarch64-unknown-linux-gnu` target matrices across GitHub Actions release workflows, Homebrew formula templates, and installer scripts.
- Implement version normalization in installer scripts to strip and restore `v` prefixes cleanly when constructing GitHub release download URLs.
- Update `Formula/flashwt.rb` and `scripts/gen-formula.sh` to package and install shell completions.

### Decision 2: Store GC & Index Simplification
- Purge all legacy per-blob refcount file operations, directory locks, and dual-mode migration markers from `flashwt-store`, establishing mark-and-sweep via store mirrors as the sole GC implementation.
- Simplify snapshot metadata persistence (`SelectionIndex`) to write an atomic TSV manifest via temporary file rename, eliminating multi-file WAL journal appending (`journal.tsv`), compaction passes, and compaction locking.
- Replace hand-rolled byte decoding loops in batch walking with standard library `u32::from_le_bytes` slice conversions.
- Unify timestamp serialization and parsing (`format_mtime`, `parse_mtime`) between `ValidationCache` and `VerifiedLedger`.

### Decision 3: Relocate Tree Ingestion into Store Package
- Move directory tree scanning, bulk walker dispatch, and validation cache synchronization from `crates/flashwt-cli/src/hydrate.rs` into `crates/flashwt-store/src/ingest.rs`.
- Expose a clean `DiskStore::ingest_tree` method accepting an ingestion path, exclusion patterns, and configuration options, returning an `IngestedTree` summary.
- Update `HydrationEngine` in `flashwt-cli` to call `DiskStore::ingest_tree`, simplifying `hydrate.rs` by several hundred lines.

### Decision 4: Copy Engine & Backend Cleanup
- Remove obsolete backend safety enum variants (`Safety::UnsafePending`, `Error::UnsafeBackend`) and validation assertions since all shipped backends are proven safe.
- Eliminate redundant file materialization delegators from the high-level copy engine and remove dead method aliases from `Materializer`.
- Replace manual buffer copy loops in `sys::buffered_copy_file` with `std::io::copy`.
- Replace hand-rolled ancestor directory traversal loops with `Path::ancestors()`.
- Deduplicate hardlink creation and readonly permission stripping into `hardlink_readonly`.

### Decision 5: CLI Streamlining, Clap Aliases & Presentation Module
- Delete dead `HydrationFilter` struct and forwarder shim module `manifest.rs`.
- Convert duplicated subcommand enum variants (`New`, `Isolate`, `TestDrive`) to native Clap aliases on canonical commands (`Create`, `Scratch`, `Demo`).
- Extract a unified JSON envelope emission helper (`emit_json`) to remove repetitive serialization blocks across command dispatch branches.
- Derive `Default` on `CleanData` in `envelope.rs` to eliminate zeroed struct literals in `clean.rs`.
- Simplify scratch worktree ID generation to timestamp-PID bit mixing and use `flashwt_store` content hashing in `demo.rs`, dropping the direct `sha2` crate dependency from `flashwt-cli`.
- Create a dedicated presentation module (`crates/flashwt-cli/src/output.rs`) exporting `HumanBytes`, `HumanDuration`, `HumanCount`, and aligned table rendering helpers.
- Replace duplicate formatting helpers across `create.rs`, `clean.rs`, `list.rs`, and `demo.rs` with calls to the presentation module.

### Decision 6: Deep Workspace Lifecycle Module
- Deepen the `workspace` module in `flashwt-cli` to encapsulate:
  - Repository root detection and gitdir resolution.
  - Worktree creation, checkout, and destination path derivation.
  - Porcelain worktree record parsing and metadata mapping (`RawGitWorktree`).
  - Merge-base ancestor inspection for identifying merged branches.
  - Worktree and branch removal execution.
- Command handlers for listing, cleanup, creation, and scratch isolation interact exclusively with the workspace module interface.

### Decision 7: Shell Autocompletions & Documentation Realignment
- Add `clap_complete` to `flashwt-cli` dependencies and implement `flashwt completions <shell>` supporting Bash, Elvish, Fish, PowerShell, and Zsh.
- Update `install.sh` to auto-detect active shell directories and install completion scripts automatically.
- Realign `README.md` to highlight the modern 3-verb workflow (`flashwt new`, `flashwt clean`, `flashwt list`) and zero-setup test drive (`flashwt demo`).
- Correct the snapshot configuration table in `README.md` to reflect automatic APFS defaults on macOS.

### Decision 8: Test Suite Consolidation & Benchmark Unification
- Group fragmented integration test files in `crates/flashwt-cli/tests/` into ~5 logical test suites to slash Cargo link times.
- Centralize fixture creation and git helper routines into `crates/flashwt-cli/tests/common/mod.rs`.
- Adjust debug-mode test fixture sizes to prevent multi-minute disk copy stalls while preserving mutation isolation and scorecard verification.
- Consolidate duplicate stage-timing parsing and tree verification routines into `benchmarks/eval.sh`.
- Add process liveness checks in `benchmarks/chaos.sh` for SIGKILL fault injection tests.

## Testing Decisions

- Only external command behaviors, file contents, JSON envelopes, and exit codes will be asserted; internal helper structures and traits will not be tested directly.
- Existing integration test assertions (verifying APFS clonefile speedups, CoW mutation isolation, base branch tracking, garbage collection, and shell completions) must remain 100% green without regressions.
- Automated release formula generation and installer script smoke tests will be validated against simulated release directories.
- Presentation unit tests will cover boundary transitions, zero values, and decimal scaling precision.

## Out of Scope

- Graphical User Interface (GUI) or desktop menu bar apps.
- Background daemon processes or systemd / launchd services.
- Windows file system clone optimizations.
- Changing the on-disk storage layout for objects (`objects/xx/yyyy...`), mirrors (`worktrees/*.tsv`), or snapshots (`snapshots/<hash>/`).
- Modifying the public CLI interface, flags, or version 1 JSON envelope schemas.

## Further Notes

- Completely preserves backward compatibility for all existing flags, subcommands (`create`, `remove`, `sweep`, `scratch`), and versioned JSON envelope formats.
- Maintains compliance with all project ADRs (ADR-0001 through ADR-0006).
