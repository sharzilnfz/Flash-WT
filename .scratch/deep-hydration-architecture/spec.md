# Spec: Deep Architecture for Hydration & Storage Reclamation

Status: ready-for-human

## Problem Statement

As `flashwt` has expanded with whole-directory snapshots, mark-and-sweep garbage collection, ephemeral scratch leases, and platform copy acceleration, the CLI orchestrators have become shallow and wide. High-level commands (`flashwt create`, `flashwt scratch`, `flashwt isolate`, `flashwt sweep`) are forced to manually coordinate low-level lifecycle steps: ingesting files, probing filesystem strategies, managing thread pools, verifying blob checksums, generating sidecar ledgers, recording mirrors, and polling process trees.

This procedural fragmentation creates critical friction:
- **Shallow modules**: Callers must know six or more distinct functions in precise sequence to hydrate a worktree without corrupting storage invariants.
- **Leaked seams**: GC and lease cleanup logic in the CLI layer performs raw storage sweeps and git worktree removals simultaneously.
- **Scattered error recovery**: Sweep-race healing, snapshot cache fallbacks, and chmod repairing on shared hardlinks are duplicated across command entry points.
- **Testing friction**: Testing hydration or garbage collection requires spinning up full CLI subprocesses or mocking intricate internal pipelines.

## Solution

Deepen the core modules across the codebase by establishing high-leverage seams:
1. **Unified `HydrationEngine`**: A deep module presenting a single unified operation to callers (`hydrate`) that encapsulates directory scanning, snapshot routing, multi-threaded verified placement, atomic mirror publishing, and sidecar ledger updates.
2. **Autonomous `StoreReclaimer`**: A deep storage reclaimer that unifies object mark-and-sweep, snapshot retention budgeting, and ephemeral lease expiration behind a clean interface, using an explicit adapter for workspace cleanup.
3. **Encapsulated `SnapshotProjectionEngine`**: Relocate snapshot selection heuristics, v2 delta rebuild calculations, and self-healing blob retries into `flashwt-store`.
4. **Encapsulated `Materializer`**: Consolidate OS copy primitives (APFS clonefile, Linux reflink, copy_file_range, byte fallback) and permission mode normalization within `flashwt-copy`.

## User Stories

1. As a developer running `flashwt create`, I want worktree creation to be atomic and reliable, so that interrupted commands never leave half-initialized mirrors or corrupt cache entries behind.
2. As an AI agent creating ephemeral worktrees via `flashwt scratch` or `flashwt isolate`, I want hydration and lease tracking to execute with zero boilerplate, so that sandbox provisioning is fast and durable.
3. As a developer using `flashwt`, I want heavy directory hydration (`node_modules`, build targets) to automatically select the fastest materialization backend (APFS clonefile, reflink, or copy_file_range) without manual configuration flags.
4. As a maintainer, I want invariant enforcement (e.g. mirror TSVs matching placed files and snapshots) to live in a single module, so that bug fixes in hydration automatically benefit all command entry points.
5. As an automated test writer, I want to test the entire hydration lifecycle through a single method call against a test store, so that integration tests are robust and readable.
6. As an operator running `flashwt sweep`, I want dead ephemeral leases and unreferenced objects to be swept deterministically under configured grace periods and storage budgets without spurious warnings.
7. As a developer working in a multi-tenant or shared repository, I want process liveness checks and lease expirations to safely reclaim orphan sandboxes without interfering with active worktrees.
8. As a contributor extending `flashwt-store`, I want snapshot cache selection and v2 delta rebuild heuristics to be internal storage algorithms, so that the CLI doesn't need to know storage-specific cache rings.
9. As a Linux developer, I want copy acceleration (reflink and copy_file_range) and permission bits to be applied transparently during hydration, so that executables retain executable bits on disk.
10. As a toolchain integrator, I want post-hydration path relocations to run predictably after tree materialization, so that virtual environments and package managers work immediately out of the box.
11. As a system administrator, I want storage reclamation to respect configured grace periods (`FLASHWT_GC_GRACE`), so that concurrent creates and sweeps never race or delete active blobs.
12. As an AI agent debugging a failed command, I want structured JSON diagnostics that pinpoint exact storage or filesystem refusals without scraping arbitrary terminal prose.

## Implementation Decisions

### Decision 1: Deep `HydrationEngine` Module
- A single `HydrationEngine` module in `flashwt-cli` / `flashwt-store` acts as the primary seam for all tree materialization.
- The interface is reduced to one primary method taking a declarative `HydrationRequest` and returning a `HydrationReport`.
- All sub-operations (directory walk, validation caching, blob verification, placement strategy fallback, multi-worker thread coordination, sidecar logging, and atomic mirror publishing) are internal implementation details hidden behind the seam.
- The deletion test succeeds: deleting individual helper functions eliminates scattered coordination from callers.

### Decision 2: Autonomous `StoreReclaimer` with Clean Workspace Adapter
- Storage reclamation is consolidated inside a deep `StoreReclaimer` module.
- The reclaimer presents a single `sweep` method taking a `SweepPolicy` (grace duration, snapshot retention cap, maximum disk byte budget) and returns a structured `SweepSummary`.
- Ephemeral lease evaluation (PID liveness via `/proc` or `sysctl`, TTL expiration, orphaned directory detection) is unified with mark-and-sweep object and snapshot garbage collection.
- File system and git workspace side effects are delegated to a cleanly typed `WorkspaceCleaner` adapter, allowing 100% deterministic unit testing with in-memory test doubles.

### Decision 3: Relocation of Snapshot Projection to `flashwt-store`
- `SnapshotProjectionEngine` moves entirely to `flashwt-store`.
- The storage engine owns lockfile hash inspection, selection index rings, LRU touches, delta threshold calculations (`changed_count * 2 < total`), and ENOENT blob-healing retries.
- The CLI layer only specifies whether snapshot acceleration is enabled and provides the target workspace paths.

### Decision 4: Self-Contained `Materializer` in `flashwt-copy`
- The `Materializer` module in `flashwt-copy` encapsulates filesystem capability detection, backend priority resolution (clonefile -> reflink -> copy_file_range -> buffered copy), and permission mode normalization.
- Mode repair logic (substituting private copies for hardlinked blobs whose exec bits differ from source metadata) is completely internal to `flashwt-copy`.

## Testing Decisions

- **Good Tests Only Exercise the Interface**: Tests will exercise the public methods of `HydrationEngine`, `StoreReclaimer`, and `Materializer`. No tests will assert private intermediate structs or private helper functions.
- **End-to-End Invariant Verification**: Hydration tests will assert that a materialized worktree has exact file byte parity, accurate permission modes, a valid `flashwt-hydrated.tsv` sidecar, and an atomic mirror in the store.
- **Race Condition & Crash Resilience**: Tests will simulate ENOENT blob disappearance mid-flight to verify that self-healing and two-attempt retry loops recover gracefully.
- **Prior Art**: Existing test suites in `crates/flashwt-cli/tests/` (`cow_materialization.rs`, `gc_mirror.rs`, `snapshots.rs`, `lease_sweep.rs`) will be ported to use the deepened interfaces directly.

## Out of Scope

- Implementing asynchronous (Tokio / async-await) I/O pipelines (ADR-0003 mandates single-binary synchronous design).
- Modifying the on-disk format of canonical snapshot manifests or mirror TSVs (retains backward compatibility with existing stores).
- Adding background daemon watcher processes (deferred per ADR-0003).

## Further Notes

- All changes maintain strict adherence to ADR-0001 through ADR-0006.
- The glossary defined in `CONTEXT.md` (Store, Tree, Materialize, Hydrate, Mirror, Snapshot, Grace period) and `codebase-design` (Module, Interface, Depth, Seam, Adapter, Leverage, Locality) must be maintained across all pull requests and documentation.
