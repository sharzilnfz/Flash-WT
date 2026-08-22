# Handoff: `wt` hydration performance problem

Date: 2026-08-23. Repo: `/Users/sharzilnafis/Desktop/Project/dumps/idea1`, branch `master`, HEAD `b652c0c`. Everything below is self-contained; no other context needed.

## 1. What this project is

`wt` is a free, open-source CLI that makes agentic coding fast by ending small-file churn around git worktrees. Agents spin up many worktrees per day; each normally pays full dependency installs and rebuilds. `wt create` makes a git worktree and fills ("hydrates") its heavy untracked directories (`node_modules`, `target`, caches) from a per-machine content-addressed store, so the second and later worktrees cost near nothing and share physical blocks.

Domain vocabulary (from CONTEXT.md):

- **Store**: the single place where every unique file content lives exactly once, addressed by its SHA-256 (like git's object database).
- **Materialize**: producing tree files from store content without rewriting bytes (links or native clones).
- **Hydrate**: filling a fresh worktree with heavy content by linking/cloning instead of reinstalling/rebuilding.

Authoritative docs in-repo: `CONTEXT.md`, `docs/adr/0001..0004`, `.scratch/instant-worktrees/spec.md` (original feature), `.scratch/fast-hydration/spec.md` (current performance effort), tickets under `.scratch/*/issues/`.

## 2. Architecture as it stands (all merged, 90 tests green, clippy clean)

Rust workspace, three crates:

- `wt-store`: `DiskStore`. Layout inside a root dir (default `~/.cache/wt/store`, override `$WT_STORE`):
  - `objects/<2 hex>/<62 hex>` — immutable blobs named by SHA-256 of contents, written temp-file + atomic rename.
  - `refs/<64 hex>` — one decimal refcount file PER BLOB (legacy layout, load-bearing).
  - `ingest-cache.tsv` — path -> (size, mtime, id) so warm ingest skips reading unchanged source files (ticket 02).
  - `verified.tsv` — id -> (size, mtime) fingerprint recorded whenever a blob's hash was last checked; lets materialization trust previously verified blobs (ticket 05). `WT_VERIFY=1` forces full hashing every run.
  - GC (ticket 06): age-based sweep; blobs with refcount > 0 survive; delete order (ref file first, then object) chosen so a kill mid-delete leaves states the next sweep finishes. Per-worktree references recorded in `<worktree gitdir>/wt-hydrated.tsv`.
- `wt-copy`: directory-shaped backends (`clonefile(2)` whole-tree on APFS, Linux reflink, hardlink walker) plus the file-shaped `FileMaterialize` trait used by store hydration:
  - `CloneOut` (macOS default): per-file `fclonefileat(2)` — destination gets a private inode sharing the blob's blocks until first write. Blobs carry normal writable modes at rest so clones inherit them (no per-file chmod).
  - `HardlinkOut`: opt-in (`WT_HARDLINK=1`), experimental, strips write bits from shared inodes.
  - Byte-copy fallback everywhere else; `WT_NO_HARDLINK=1` forces plain copies.
- `wt-cli`: `wt create|remove|sweep`. `create` = `git worktree add` -> read `.wtinclude` patterns -> per heavy directory: `ingest_dir` (walk + validation cache + `put`) -> `claim_references` (+1 ref per distinct blob, append sidecar ledger) -> `materialize` (verify/trust each blob, place via selected strategy, silent byte-copy fallback on fs refusal).

Benchmark suite: `benchmarks/run.sh --verify` — three scenarios on generated fixtures (default 4,000 small files, 500 package dirs): (A) `git worktree add` + file-by-file install simulation, (B) direct recursive CoW clone of the heavy tree (`cp -Rc`; one mechanism per platform), (C) `wt create` against cold and warm stores. All runs byte-verified.

## 3. The problem, with the full measurement history

**`wt create` is still slower than both the naive baseline and the trivial alternative on warm runs.** Same machine (Apple Silicon, APFS, Darwin 25.6.0), 4,000-file fixture, medians:

| Milestone | wt cold | wt warm | baseline | direct CoW |
|---|---|---|---|---|
| Tickets 01–09 done (CAS + hardlinks, always re-hash) | 3.79 | 3.26 | 1.69 | not measured yet |
| + ticket 03 (CoW materialization default) | 2.81 | 2.37 | 1.74 | 0.95 |
| + ticket 05 (verify-once ledger, no pre-read, no chmod, EAFP dirs) | ~3.0 | **2.15** | 1.66 | 1.00 |

Warm-run cost breakdown after ticket 05 (measured/estimated by instrumentation):

1. `git worktree add` itself — shared with the baseline scenario (~0.6–0.7s).
2. Ingest validation walk — ~4,000 stat calls, cheap (~50ms).
3. Placement train — per file: open blob fd + `fclonefileat` + closes (~4 syscalls x 4,000). This is the irreducible-looking floor for per-file placement.
4. **Refcount writes — ~0.5s.** `claim_references` does one temp-write-rename cycle per distinct blob (4,000 per create) because refcounts live as one file per blob.
5. Ledger saves (verified.tsv, ingest-cache.tsv, sidecar TSV) — single-digit ms each, negligible.

Reference points from outside:

- Whole-directory `clonefile(2)` clones a 10k-file tree in ~100–150ms in ONE syscall (kernel iterates internally; measured in an external research spike). Our benchmark's "direct CoW" uses `cp -Rc`, which clones per file and lands at ~1.0s — meaning even our best alternative leaves 10x on the table versus the primitive the filesystem offers.
- bun's warm-install flamegraph: 96.5% of time is `fclonefileat` — per-file placement saturates on that syscall; accepted mitigation is thread pools over blocking fs calls.
- nativelink found post-clonefile chmod walks were 46% of materialization (~33µs/file) — we already eliminated ours.
- pnpm/pacquet found per-file stat probes and per-file `create_dir_all` dominate warm imports — we already eliminated ours.

## 4. Solutions tried and why they stopped helping

1. **Ingest validation cache (mtime+size)** — killed re-reading source files on warm runs. Kept. Correctness net: hash verification still happened at materialize, so lying cache entries fail loudly.
2. **CoW materialization replacing hardlinks** — fixed writability/semantics, kept block sharing. Neutral-to-positive on speed.
3. **Verify-once trust ledger** — expected to be the big win (skip read+SHA per blob); delivered only ~10% because at this fixture size hashing was never dominant. Kept anyway: it scales with real trees (20k–200k files) and removes O(bytes) reads per run.
4. **Syscall hygiene** (no redundant mkdir/stat/chmod per file) — landed as part of ticket 05 following the pnpm/nativelink findings.

Diagnosis: we are now down to two structural costs — the per-file placement syscall train and the per-blob refcount writes — plus one strategic question (below).

## 5. Open problems for you to attack

### P1: Can the warm path structurally beat whole-tree `clonefile`?

Our store holds individual blobs, so hydration places files one at a time and pays N syscalls. The filesystem offers a one-syscall whole-tree primitive we cannot use because there is no tree to clone — only scattered blobs. Candidate directions we have NOT tried:

- **Staging-tree + single clonefile**: assemble a throwaway staging directory whose contents hardlink/copy the needed blobs, then one recursive `clonefile(2)` into the destination, then discard staging. Note: hardlink assembly is itself per-file `link(2)` calls — cheaper than clone trains? Unknown. Measure.
- **Tree snapshots in the store**: persist hydrated *directories* (not just blobs) as clonable units — i.e., remember the last materialized tree per (project, include-set, revision) and clonefile it directly, falling back to blob materialization when absent/stale. This is "Path A inside the tool": keep the CAS for dedup/GC/integrity, but make the hot path a single kernel call. Staleness detection could reuse the ingest validation walk.
- **Batched/placement-free strategies on other axes**: `renamex_np(RENAME_SWAP)`, `copyfile(3)` with clone flags, or Linux `FICLONE` equivalents — likely dead ends but cheap to rule out.

### P2: Refcount writes (~0.5s/create)

Per-blob refcount files mean 4,000 atomic temp-write-renames per create. Options:

- **Single refcount ledger file** (one atomic rewrite per run). Format change; requires migration for existing stores AND re-analysis of GC crash safety (the current delete-ordering argument depends on per-blob files).
- **Mark-and-sweep instead of refcounts**: the per-worktree `wt-hydrated.tsv` sidecars already name exactly which blobs each worktree uses. A sweep could walk live sidecars, mark reachable blobs, and reclaim the rest — eliminating `add_ref` entirely from the create path. Needs its own crash-safety story (torn sidecar appends, worktrees deleted out from under us) but would remove cost 4 completely.

### P3: Wall-clock parallelism

Thread-pool the placement loop (std::thread::scope). Blocked today only on error-reporting semantics (fail-fast vs collect) and Mutex'd ledger updates. Expected 2–4x on the placement portion.

### P4: Strategic constraint to respect

The project's own spec says: if optimized store hydration cannot beat direct CoW cloning where it matters, the CAS must justify itself through persistence (survives `cargo clean` in the donor tree), integrity verification, dedup accounting, and GC — not raw speed. Any solution should either win the hot path OR strengthen those justifications. Real-world `node_modules` shapes (20k–200k files, heavy duplication across packages) favor the store more than our 4,000-file micro-fixture shows; a second benchmark fixture at realistic scale would sharpen the verdict.

## 6. Constraints any solution must honor

- Corruption must never land silently in a fresh tree (loud failure before placement; `WT_VERIFY=1` paranoid mode exists).
- Existing stores on disk must keep working (migration allowed, breakage not). Blob layout is sacred; sidecar ledgers are fair game.
- Hydrated files must be private, writable, normal-permission inodes.
- GC must remain crash-safe: any kill mid-operation leaves states a later run reconciles.
- No daemon/watcher scope creep; single binary, one-shot commands.
- Platform priority: macOS/APFS first, Linux later (reflink currently unexercised).

## 7. Reproducing everything

```sh
cd /Users/sharzilnafis/Desktop/Project/dumps/idea1
cargo test                      # 90 tests
./benchmarks/run.sh --verify    # three-scenario table, byte-verified
```

Isolated experiments: set `$WT_STORE` to a scratch dir; strategy flags `WT_HARDLINK=1`, `WT_NO_HARDLINK=1`, `WT_VERIFY=1`. Fixture generator: `benchmarks/fixture.sh`.
