# Architecture Review: Deepening Opportunities for `flashwt`

**Project:** `flashwt` (Instant Worktrees)  
**Scope:** `flashwt-cli`, `flashwt-store`, `flashwt-copy`  
**Focus:** Turning shallow modules into deep ones, eliminating seam leaks, and sharpening domain leverage.

---

## 1. Executive Summary & Top Recommendation

The `flashwt` codebase has successfully implemented high-performance primitives (content-addressed store, whole-directory APFS snapshots, mark-and-sweep GC, and ephemeral lease management). However, recent feature additions have created **architectural friction** primarily through **shallow orchestration modules in `flashwt-cli`**.

### Top Recommendation: Candidate 1 — Unified `HydrationEngine`
We recommend tackling **Candidate 1 (Unified `HydrationEngine`)** first.
- **Why first:** Hydration is the single most critical hot path in `flashwt`. Currently, `flashwt-cli/src/hydrate.rs` and `flashwt-cli/src/commands/create.rs` manually coordinate six distinct lifecycle steps (ingest, strategy selection, parallel placement, blob verification, sidecar logging, and atomic mirror publishing). 
- Consolidating this into a deep `HydrationEngine` directly eliminates the largest source of code duplication and bug risk between `flashwt create`, `flashwt scratch`, and future daemon/agent paths.

---

## 2. Architectural Candidates

### Candidate 1: Unified Hydration Engine
* **Recommendation Strength:** `Strong`
* **Involved Modules:** `crates/flashwt-cli/src/hydrate.rs`, `crates/flashwt-cli/src/commands/create.rs`, `crates/flashwt-cli/src/commands/scratch.rs`
* **Problem:** `hydrate.rs` (800+ lines) exposes a wide, shallow interface of procedural helpers (`ingest_dir`, `materialize`, `claim_references`, `claim_snapshot_references`, `publish_mirror`, `ensure_published`). Callers in `commands/create.rs` and `commands/scratch.rs` must manually orchestrate these low-level phases in lockstep. If a caller forgets a step (such as updating mirror records or flushing verification caches), storage invariants break.
* **Solution:** Encapsulate ingestion, snapshot vs per-file ladder selection, multi-threaded blob verification, atomic mirror publishing, and sidecar ledger updates behind a single deep **`HydrationEngine`** interface:
  ```rust
  pub struct HydrationEngine<'a> {
      store: &'a mut DiskStore,
      config: &'a RunConfig,
  }

  impl<'a> HydrationEngine<'a> {
      pub fn hydrate(&mut self, req: HydrationRequest) -> Result<HydrationReport>;
  }
  ```
* **Benefits:**
  * **Leverage:** Callers invoke one method with high-level parameters (`dest`, `heavy_dirs`, `base_ref`) rather than managing six lifecycle stages.
  * **Locality:** Invariant maintenance (e.g. mirror atomicity and sidecar ledger synchronization) is locked inside one module.
  * **Testability:** Full hydration scenarios can be tested with a single call against a test store without mocking internal stages.

---

### Candidate 2: Autonomous Store & Lease Reclamation Engine
* **Recommendation Strength:** `Strong`
* **Involved Modules:** `crates/flashwt-cli/src/gc.rs`, `crates/flashwt-cli/src/commands/sweep.rs`, `crates/flashwt-store/src/gc.rs`, `crates/flashwt-store/src/lease.rs`
* **Problem:** Store GC and lease cleanup logic are heavily fragmented across `flashwt-cli` and `flashwt-store`. In `flashwt-cli/src/gc.rs`, `sweep_leases` manually parses leases, inspects process tables, calculates directory disk usage, parses hydration ledgers, releases individual blob refcounts, removes mirrors, and shells out to `git worktree remove` and `git branch -D`. Meanwhile, `flashwt-store/src/gc.rs` independently sweeps objects and snapshots.
* **Solution:** Create a deep **`StoreReclaimer`** module in `flashwt-store` (with an injectable `GitWorkspaceCleaner` adapter) that encapsulates both lease lifecycle management and mark-and-sweep object collection:
  ```rust
  pub struct StoreReclaimer<'a, G: WorkspaceCleaner> {
      store: &'a mut DiskStore,
      cleaner: G,
  }

  impl<'a, G: WorkspaceCleaner> StoreReclaimer<'a, G> {
      pub fn sweep(&mut self, policy: SweepPolicy) -> Result<SweepReport>;
  }
  ```
* **Benefits:**
  * **Leverage:** CLI, background sweep daemons, and programmatic agents call `reclaimer.sweep(policy)` with uniform semantics.
  * **Locality:** Grace period arithmetic, lease PID validation, and tombstone removal are contained in a single cohesive place.
  * **Testability:** Lease expiration and GC can be verified deterministically with simulated clocks and fake processes.

---

### Candidate 3: Deep Snapshot Projection Engine
* **Recommendation Strength:** `Worth exploring`
* **Involved Modules:** `crates/flashwt-cli/src/snapshots.rs`, `crates/flashwt-store/src/snapshot/`, `crates/flashwt-store/src/snapindex.rs`
* **Problem:** `flashwt-cli/src/snapshots.rs` (717 lines) contains storage-layer caching policies: v2 delta rebuild heuristics, selection ring rotation, sweep-race retry loops, lockfile fast-path routing, and blob healing (`heal_blob`). These belong in the storage engine, not in the CLI layer.
* **Solution:** Relocate snapshot routing, incremental delta planning, and self-healing blob rebuilds into a deep **`SnapshotProjectionEngine`** in `flashwt-store`. `flashwt-cli` only requests a projection at a given destination path.
* **Benefits:**
  * **Leverage:** Non-CLI consumers (e.g. IDE sidecars or daemon sync engines) get snapshot caching and delta rebuilds automatically.
  * **Locality:** Snapshot selection heuristics and self-healing retry logic live right beside snapshot storage.
  * **Testability:** Snapshot diffing, lockfile matching, and recovery from missing blobs can be unit-tested without CLI harness dependencies.

---

### Candidate 4: Encapsulated Materialization & Capability Matrix
* **Recommendation Strength:** `Worth exploring`
* **Involved Modules:** `crates/flashwt-copy/src/materialize.rs`, `crates/flashwt-copy/src/selection.rs`, `crates/flashwt-cli/src/hydrate.rs`
* **Problem:** Strategy negotiation (CoW clone vs Linux reflink vs copy_file_range vs byte-copy) is implemented in `flashwt-cli/src/hydrate.rs::select_strategy`. Furthermore, `finalize_mode` in `flashwt-cli` manually checks `shares_inode_with_source` to work around chmod issues on shared hardlinks by replacing them with private byte copies.
* **Solution:** Deepen `flashwt-copy`'s **`Materializer`** interface to handle capability negotiation, fallback ladders, and permission normalization internally:
  ```rust
  pub struct Materializer {
      policy: StrategyPolicy,
  }

  impl Materializer {
      pub fn place_file(&self, src: &Path, dest: &Path, mode: u32) -> Result<PlacementResult>;
  }
  ```
* **Benefits:**
  * **Leverage:** Callers specify source, destination, and desired permission mode; the materializer handles physical filesystem mechanics.
  * **Locality:** Inode sharing edge cases (e.g. exec bit adjustments on hardlinks) stay strictly within `flashwt-copy`.
  * **Testability:** Strategy selection and fallback execution can be validated across synthetic filesystem mocks.

---

## 3. Comparison Matrix

| Candidate | Seam Location | Interface Surface | Implementation Depth | Leverage Gain | Locality Gain |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **1. HydrationEngine** | `flashwt-cli` / `flashwt-store` | 1 method (`hydrate`) | Scans, verifies, CoW clones, mirrors, sidecars | Very High | Very High |
| **2. StoreReclaimer** | `flashwt-store` | 1 method (`sweep`) | Lease inspection, PID checks, GC, mark-and-sweep | High | High |
| **3. SnapshotProjection** | `flashwt-store` | 2 methods (`project`, `invalidate`) | Lockfile check, v2 delta diff, self-healing | High | Medium |
| **4. Deep Materializer** | `flashwt-copy` | 2 methods (`place_file`, `copy_dir`) | Probing, clone/reflink, fallback, chmod repair | Medium | High |
