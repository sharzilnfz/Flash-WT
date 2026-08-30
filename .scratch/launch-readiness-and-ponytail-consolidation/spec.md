# Spec: Launch Readiness & Ponytail Codebase Simplification

Status: ready-for-agent

## Problem Statement

As `wt` approaches its v0.1.0 public release, the codebase carries three distinct classes of technical friction:

1. **Release Packaging Blockers**: Inconsistencies in release archive naming (`wt-0.1.0-` vs `wt-v0.1.0-`), absence of Linux ARM64 (`aarch64-unknown-linux-gnu`) release builds, and strict version tag expectations in installation scripts will cause automated release pipelines and Homebrew installation to fail immediately upon tagging.
2. **Accumulated Migration Baggage & Over-Engineering**: The codebase retains over 2,000 lines of obsolete migration scaffolding, including dual-write per-blob refcounting machinery, multi-file WAL journal index compaction, single-implementation traits, dead forwarder shims, and hand-rolled standard library routines that increase cognitive load for maintainers without delivering user value.
3. **Integration Test Suite Link Overhead & Execution Drag**: Twenty-six separate integration test binaries in the CLI crate cause excessive Cargo link times, and unoptimized debug-mode benchmark tests synthesize tens of thousands of real files on disk, causing CI test suites to drag on for several minutes.

## Solution

A targeted, zero-breaking-change consolidation program that:
1. Resolves all release workflow, Homebrew formula, and install script blockers for both macOS and Linux (x86_64 and ARM64).
2. Executes a whole-repo Ponytail cleanup sweep across the store, CLI, copy backends, and benchmark scripts, deleting over 2,000 lines of dead code and dropping unneeded dependencies.
3. Consolidates integration test executables and fixture generators, slashing CI test suite duration to under 30 seconds while preserving 100% functional and behavioral test coverage.

## User Stories

1. As a macOS developer installing `wt` via Homebrew, I want `brew install wt` to download valid binary tarballs from GitHub Releases, so that installation succeeds seamlessly without checksum mismatch errors.
2. As a Linux developer on an ARM64 cloud instance (e.g. AWS Graviton), I want `install.sh` to download a native `aarch64-unknown-linux-gnu` binary, so that I can run `wt` without cross-architecture emulation.
3. As a developer running the standalone installer with `WT_VERSION=0.1.0` (with or without the leading `v`), I want the installer to normalize the tag, so that I never get a 404 download error.
4. As an open-source contributor exploring `wt-store`, I want the storage engine to exclusively use the atomic store mirror mark-and-sweep GC model, so that I do not have to understand obsolete per-blob refcounting code or dual-mode migration states.
5. As an agent orchestrator launching parallel worktrees, I want the snapshot index to use atomic single-file manifest persistence without complex WAL journal file locking, so that lockfile cache lookups remain fast, simple, and crash-resilient.
6. As a maintainer auditing dependencies, I want `wt-cli` to avoid pulling in direct cryptographic hashing dependencies when the storage engine already encapsulates content addressing, so that binary dependency trees remain minimal.
7. As a CLI user running `wt new`, `wt isolate`, or `wt test-drive`, I want subcommand aliases to be handled natively by Clap, so that documentation, shell completions, and help messages are unified without boilerplate code.
8. As a developer modifying copy backends in `wt-copy`, I want the copy engine to omit speculative safety checks and single-method wrapper structs, so that adding new platform backends requires minimal boilerplate.
9. As a developer running `cargo test`, I want integration tests to compile and link in seconds across consolidated test suites, so that my local feedback loop is fast.
10. As a developer running test suites in unoptimized debug mode, I want test fixtures to scale appropriately, so that integration tests do not stall for minutes writing tens of thousands of byte-copied files.
11. As a CI maintainer, I want `eval.sh` to serve as the unified regression benchmark harness, so that performance gating is standardized across pull requests and local benchmarking.

## Implementation Decisions

### 1. Release Packaging & Multi-Platform Distribution
- Standardize all release archive naming in CI workflows to `wt-v<VERSION>-<TARGET>.tar.gz` to ensure strict alignment with Homebrew formula generation and installer scripts.
- Add `aarch64-unknown-linux-gnu` target matrices across GitHub Actions release workflows, Homebrew formula templates, and installer scripts.
- Implement version normalization in installer scripts to strip and restore `v` prefixes cleanly when constructing GitHub release download URLs.

### 2. Store GC & Index Simplification
- Purge all legacy per-blob refcount file operations, directory locks, and dual-mode migration markers from the storage engine, establishing mark-and-sweep via store mirrors as the sole GC implementation.
- Simplify the snapshot selection index to write an atomic TSV manifest via temporary file rename, eliminating multi-file WAL journal appending, compaction passes, and compaction locking.
- Replace hand-rolled byte decoding loops in batch walking with standard library slice conversions.
- Unify timestamp serialization and parsing across stat caches and verified ledgers.

### 3. CLI & Command Layer Streamlining
- Delete the dead hydration filter struct and remove deprecated forwarder modules, using direct free functions and imports.
- Convert duplicated subcommand enum variants (`New`, `Isolate`, `TestDrive`) to native Clap aliases on their canonical command counterparts (`Create`, `Scratch`, `Demo`).
- Extract a unified JSON envelope emission helper to remove repetitive serialization and printing blocks across command dispatch branches.
- Derive standard default traits on clean data structures to eliminate repetitive zeroed struct literals.
- Remove redundant direct cryptographic hashing dependencies from the CLI package manifest.

### 4. Copy Engine & Backend Cleanup
- Remove obsolete backend safety enum variants and validation assertions since all shipped backends are proven safe.
- Eliminate redundant file materialization delegators from the high-level copy engine and remove dead method aliases from the materializer.
- Replace manual buffer copy loops and manual ancestor directory traversal loops with standard library equivalents.

### 5. Test Suite Consolidation & Benchmark Unification
- Group fragmented integration test files into consolidated suites to reduce Cargo link times.
- Centralize fixture creation and git helper routines into shared test modules.
- Adjust debug-mode test fixture sizes to prevent multi-minute disk copy stalls while preserving mutation isolation and scorecard verification.
- Unify benchmark parsing and verification routines into the regression evaluation script.

## Testing Decisions

- Only external command behaviors, file contents, JSON envelopes, and exit codes will be asserted; internal helper structures and traits will not be tested directly.
- Existing integration test assertions (verifying APFS clonefile speedups, CoW mutation isolation, base branch tracking, garbage collection, and shell completions) must remain 100% green without regressions.
- Automated release formula generation and installer script smoke tests will be validated against simulated release directories.

## Out of Scope

- Introducing new storage backends or virtual filesystem drivers (e.g. FUSE/NFS).
- Changing the on-disk storage layout for objects (`objects/xx/yyyy...`), mirrors (`worktrees/*.tsv`), or snapshots (`snapshots/<hash>/`).
- Modifying the public CLI interface, flags, or version 1 JSON envelope schemas.

## Further Notes

This spec unifies the findings of the whole-repository ponytail audit, yielding a cleaner, faster, and more maintainable foundation ready for public release.
