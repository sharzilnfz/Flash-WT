# Spec: Deep Architecture and Storage Module Consolidation

Status: ready-for-agent

## Problem Statement

As `wt` has expanded, several core systems have fractured across crate and module seams, introducing cognitive load, code duplication, and leaked invariants:

1. **Hydration Logic Leaks Across Three Crates**: The command line layer directly orchestrates low-level storage mechanics. It parses lockfiles, selects snapshot heuristics, drives thread-pool file placement, verifies blob checksums, formats sidecar ledgers, and publishes garbage collection mirrors.
2. **Shallow Forwarding Shims**: A 172-line shallow module (`wt-cli::snapshots`) exists solely to unpack parameters into a 15-argument primitive function call on `SnapshotProjectionEngine`. Similarly, three command modules (`commands/remove.rs`, `commands/sweep.rs`, `commands/migrate.rs`) are 10-line shims that forward directly to garbage collection routines.
3. **Duplicated Copy Hierarchies**: File placement and directory copying are split across two parallel hierarchies in the copy crate (`CopyBackend` and `FileMaterialize`). Callers are forced to probe filesystem device identifiers and pass raw boolean capability flags instead of delegating strategy selection to the copy engine.
4. **Fractured Reclamation Seams**: Worktree removal splits sidecar ledger parsing in the command line interface crate from mark-and-sweep reclamation in the storage crate. Invariants governing how references and mirrors are unlinked leak into command handlers.
5. **Domain Name Collisions and Duplicated Exclusions**: Two unrelated concepts are named "manifest": user-configured `.wtinclude` patterns in the CLI crate, and content-addressed canonical TSV records in the storage crate. Furthermore, default `.wtinclude` patterns duplicate compiler cache exclusion rules implemented as hardcoded filters in the toolchain module.

## Solution

Consolidate these four focus areas into deep modules with clean seams:

1. **Deep Store Hydration Module (`DiskStore::hydrate`)**: Relocate hydration coordination into the storage crate behind a single deep interface. Callers supply a typed hydration request naming workspace paths, include patterns, and execution policies. The engine internally resolves lockfile matching, whole-directory snapshot projection, parallel verified file placement, sidecar ledger persistence, and garbage collection mirror publishing. Delete the intermediate snapshot forwarding shim.
2. **Unified Copy Engine (`CopyEngine`)**: Unify directory copying and file placement into a single deep copy engine in `wt-copy`. The engine accepts source and destination paths, automatically detects filesystem capabilities and volume boundaries, executes the optimal placement strategy, normalizes file permissions, and returns an execution report.
3. **Consolidated Worktree Retirement and Store Reclamation (`StoreReclaimer`)**: Deepen `StoreReclaimer` in `wt-store` to own worktree sidecar retirement, mirror unlinking, and garbage collection through a single reclamation seam. Delete the three shallow command forwarding modules.
4. **Unified Hydration Filter Module (`HydrationFilter`)**: Consolidate `.wtinclude` pattern parsing, starter manifest generation, and volatile compiler cache exclusions into a single deep module. Eliminate duplicate pattern matching rules and resolve the naming collision with snapshot manifests.

## User Stories

1. As a developer creating a worktree with `wt create`, I want heavy directories hydrated atomically, so that half-initialized worktrees or broken ledgers are never left behind.
2. As a developer creating an ephemeral sandbox with `wt scratch`, I want hydration to run through the exact same storage engine path as standard worktrees, so that sandbox behavior is identical.
3. As a maintainer, I want hydration orchestration to live in the storage crate, so that storage invariants do not depend on command line interface modules.
4. As a maintainer, I want the snapshot projection interface to take a single typed request, so that future parameter changes do not break multiple intermediate modules.
5. As a maintainer, I want shallow forwarding modules deleted, so that the codebase contains less boilerplate to maintain.
6. As a test writer, I want to test the entire hydration pipeline through a single method call against a disk store, so that integration tests are fast and do not require spawning binaries.
7. As a non-CLI consumer or background service, I want to hydrate a worktree directly through the storage interface, so that I do not need to link command-line parser code.
8. As a developer on macOS, I want the copy engine to automatically leverage APFS clonefile without requiring the caller to probe device identifiers, so that code is cleaner.
9. As a developer on Linux, I want the copy engine to automatically choose reflink or copy_file_range based on underlying filesystem capabilities, so that copies are hardware-accelerated.
10. As a developer on a filesystem without reflink support, I want the copy engine to fall back transparently to buffered byte copying, so that hydration always succeeds.
11. As an operator running garbage collection, I want sidecar ledgers and mirror files to be managed consistently by the storage engine, so that root validation never fails on valid checkouts.
12. As a maintainer, I want worktree retirement and reference releasing handled by `StoreReclaimer`, so that command handlers do not duplicate manual sidecar TSV parsing.
13. As a developer configuring `.wtinclude`, I want default patterns and volatile compiler cache exclusions to align in a single filter module, so that builds and cache artifacts are never accidentally hydrated.
14. As an engineer reading the codebase, I want clear domain terminology, so that snapshot manifests and hydration pattern rules are never confused.
15. As an automated test suite, I want to verify error recovery during hydration by injecting faults into the storage engine, so that edge cases are thoroughly validated.
16. As a developer inspecting command output, I want structured hydration receipts containing durations, file counts, and shared byte metrics, so that CLI output formatting remains clean and decoupled from execution.
17. As an AI agent reading the codebase, I want cohesive modules with high locality, so that tracing hydration and reclamation logic does not require navigating across multiple crates.
18. As a performance engineer, I want thread-pool materialization and blob verification to be encapsulated within the storage pipeline, so that concurrency tuning is localized to one place.

## Implementation Decisions

### Decision 1: Deep Hydration Interface on Disk Store
- Consolidate hydration coordination into the storage engine module.
- The interface presents a single primary operation taking a declarative request and returning a structured receipt.
- The request encapsulates repository paths, destination worktree paths, hydration include patterns, base branch references, and verification policies.
- The receipt returns processed file counts, copied file counts, shared copy-on-write bytes, copied bytes, execution strategy names, timing breakdowns, and diagnostics.
- Sidecar ledger formatting and mirror publishing execute internally during hydration.
- Deletion test result: deleting scattered hydration helpers in the CLI crate eliminates boilerplate across command handlers.

### Decision 2: Elimination of Shallow Forwarding Shims
- Remove the 172-line snapshot translation module in the CLI crate (`crates/wt-cli/src/snapshots.rs`).
- Remove the three 10-line command forwarders (`commands/remove.rs`, `commands/sweep.rs`, `commands/migrate.rs`), collapsing invocation directly into the CLI dispatcher.
- Callers interact directly with the unified storage hydration and reclamation interfaces.

### Decision 3: Unified Copy Engine
- Consolidate directory copying and per-file placement into a unified copy engine in the copy crate.
- The engine exposes two primary operations: whole-directory copying and batch file materialization.
- The interface accepts source paths, destination paths, file mappings, and copy strategy policies.
- Filesystem capability detection, volume boundary checks, and permission mode normalization are internal implementation details hidden behind the seam.
- Callers no longer probe device identifiers or supply manual boolean capability flags.

### Decision 4: Autonomous Worktree Retirement and Storage Reclamation
- Consolidate worktree retirement and reference releasing inside `StoreReclaimer` in the storage crate.
- `StoreReclaimer` parses the worktree sidecar ledger, releases blob and snapshot references, and removes the store-local mirror.
- An explicit workspace cleaner adapter executes the git worktree deletion.
- Command handlers invoke a single `retire_worktree` method rather than orchestrating file and storage updates manually.

### Decision 5: Unified Hydration Filter and Exclusion Module
- Consolidate `.wtinclude` pattern parsing, starter manifest generation, and volatile compiler cache detection into a single deep `HydrationFilter` module.
- Rename the module to resolve the naming collision with snapshot manifests.
- Expose a simple interface: `filter.should_hydrate(path) -> bool` and `filter.is_excluded(path) -> bool`.
- Eliminate duplicated pattern checks across pattern defaults and toolchain filters.

## Testing Decisions

- **Good Tests Target Public Seams**: Tests will target the public interface of the storage hydration engine, copy engine, reclaimer, and hydration filter. Tests will assert observable outcomes: destination directory contents, file contents, executable permissions, mirror files on disk, sidecar ledger lines, and returned receipt metrics. Tests will not assert private helper functions or intermediate variables.
- **Modules Tested**:
  - The storage hydration engine: testing snapshot hits, incremental rebuilds, fallback placement, ledger persistence, and mirror publishing.
  - The copy engine: testing APFS clonefile, reflink, copy_file_range, byte fallback, permission mode repair, and cross-device handling.
  - The store reclaimer: testing worktree retirement, reference decrements, mirror cleanup, and mark-and-sweep sweep policies.
  - The hydration filter: testing glob matching, negation rules, default starter creation, and compiler cache exclusions.
- **Prior Art**:
  - Existing store flow tests (`store_flow.rs`), GC tests (`gc.rs`, `gc_mirror.rs`), and snapshot integration tests (`snapshots.rs`, `snapshots_v2.rs`).
  - Copy backend tests (`backends.rs` in the copy crate).

## Out of Scope

- Changing the on-disk storage object hashing algorithm or content address scheme.
- Modifying the garbage collection mark-and-sweep algorithm or mirror TSV grammar.
- Modifying git porcelain parsing or workspace lifecycle operations in the workspace engine.
- Altering CLI command syntax, flags, or output formatting templates.

## Further Notes

- Maintains compliance with all project ADRs (ADR-0001 through ADR-0006).
- Follows the vocabulary defined in `CONTEXT.md` (Store, Tree, Materialize, Hydrate, Mirror, Snapshot) and the architecture vocabulary defined in `codebase-design` (Module, Interface, Depth, Seam, Adapter, Leverage, Locality).
