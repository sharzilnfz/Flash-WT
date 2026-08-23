# Handoff: state of `wt` after the hydration performance build-out

Date: 2026-08-23. Branch `master`, HEAD `fa1866f`. Working tree clean.
135 tests green (`cargo test`), clippy clean. This document is
self-contained; an external agent or a future you needs nothing else.

## 1. What `wt` is

`wt` is a CLI that makes agentic coding cheap around git worktrees.
Agents create many worktrees per day; each one normally pays full
dependency installs and rebuilds of untracked heavy directories
(`node_modules`, `target`, caches). `wt create` makes the git worktree
and fills ("hydrates") those heavy directories from a per-machine
content-addressed store instead of reinstalling.

Domain terms (full glossary in CONTEXT.md): the **store** keeps every
unique file content exactly once, addressed by SHA-256. **Materialize**
means producing tree files from store content without rewriting bytes.
**Hydrate** means filling a fresh worktree from the store. A **mirror**
is a store-local record naming which blobs or snapshots one live
worktree depends on; mirrors are what garbage collection trusts.

## 2. How it all works now

Rust workspace, three crates:

### wt-store

`DiskStore`, rooted at `~/.cache/wt/store` (override with `$WT_STORE`).
Everything inside one root directory:

```
objects/<2hex>/<62hex>   immutable blobs named by SHA-256 of contents;
                         written temp-file then atomic rename; normal
                         writable modes at rest (0644, 0755 if exec)
refs/<64hex>             legacy per-blob refcounts; still maintained
                         until explicit cutover (see GC below)
worktrees/<key>.tsv      THE GC roots. One mirror per hydrated worktree,
                         key = SHA-256 of "version=1\0<worktree path>\0<gitdir>".
                         Typed records: v1 header, file <blob>, snapshot <hash>.
                         Published by write-temp-then-rename.
gc-mode                  absent = legacy refcount sweep (default);
                         mark-sweep = collect from mirrors;
                         mark-sweep-no-refs = same, plus stop touching refs/
ingest-cache.tsv         path -> (size, mtime, blob id) so unchanged source
                         files are not re-read on later ingests
verified.tsv             blob id -> (size, mtime) fingerprint recorded when
                         its hash was last proven; materialization trusts
                         these without re-hashing. WT_VERIFY=1 disables trust.
snapshots/<hash>/        whole-directory snapshot: manifest.tsv (canonical),
                         .complete marker, tree/ = hardlinks to blobs
snapshots/tmp/           staging for builds; debris collected after grace
```

### wt-copy

Placement strategies behind the `FileMaterialize` trait: `CloneOut`
(macOS default, per-file `fclonefileat(2)`), `HardlinkOut` (opt-in,
experimental), byte copy as universal fallback.

### wt-cli — the `wt create` flow

1. `git worktree add` into `<repo>-<name>` (or `--dir`).
2. Read `.wtinclude` patterns, walk each heavy directory.
3. **Ingest**: every regular file is hashed and stored once (validation
   cache skips unchanged files). Symlinks and empty dirs are recorded in
   the ingest result. Non-regular files fail loudly under snapshots.
4. **References**: publish one store-local mirror naming every blob (or
   one snapshot); legacy refcount updates happen too unless the store is
   in `mark-sweep-no-refs` mode.
5. **Materialize** per heavy directory:
   - With `WT_SNAPSHOTS=1` on macOS/APFS: compute the canonical manifest
     hash. Hit (valid published snapshot): verify policy already
     satisfied at publish, so clone the whole `tree/` with ONE recursive
     `clonefile(2)` into place. Miss: verify each blob per policy
     (ledger trust, full hash under `WT_VERIFY=1`), hardlink into a
     staging tree, apply normalized modes, publish atomically, then
     clone. Any clonefile refusal falls back to the per-file ladder.
   - Without the gate: per-file verify-then-place through the strategy
     ladder (verify first so corruption never lands).
6. Print a summary; with `WT_TIMING=1` emit `wt-stage ingest=N`,
   `references=N`, `materialize=N`, `snapshot=N`, `total=N` (ms) on stderr.

### Garbage collection (`wt sweep`)

Three modes, chosen by the `<store>/gc-mode` marker:

- **legacy** (default): collects by refcount age exactly as before, but
  also computes mirror marks and reports disagreements (audit parity).
- **mark-sweep** (after `wt store migrate --activate-mark-sweep`):
  liveness comes from valid mirrors only. A mirror is live when its
  recorded worktree path exists, gitdir exists, and either the
  `wt-hydrated.tsv` sidecar exists or the mirror is younger than the
  grace period. Blobs, unreferenced snapshots, and dead mirrors are
  collected only after the grace period (`WT_GC_GRACE`, default 15m).
  refs/ files still get maintained but are ignored.
- **mark-sweep-no-refs** (after `--drop-legacy-refs`, prints a loud
  warning): refcount files stop being maintained entirely. Pre-cutover
  binaries must not touch such a store.

No `git worktree list` calls anywhere in GC; liveness is pure filesystem
existence, which survives out-of-band `rm -rf` of worktrees.

## 3. What this effort built (commits, newest last)

- `cbc4c02` Phase 1: mark-and-sweep GC transition (mirrors, audit,
  migrate subcommand, crash tests). ADR-0004.
- `fa0e628` Phase 0: realistic Scenario D benchmark fixture plus stage
  timing capture.
- `197b07a` Phase 2: APFS whole-directory snapshots behind
  `WT_SNAPSHOTS=1`. ADR-0005.
- `25aac28` Benchmark fixes found at scale (SIGPIPE-safe line capture;
  honest single-level fixture nesting).
- `fa1866f` Snapshot build skips no-op chmods; measured numbers recorded.
- `5d34edc` Step 0: fine-grained WT_TIMING stage attribution.
- `fdba0df` Step 0 follow-up: getattrlistbulk bulk walk, dirty-flag
  cache saves, buffered ledgers (ingest -85%, references -86%).
- `9f1f0e6` v2: diff-based incremental snapshot rebuilds behind
  `WT_SNAPSHOTS_V2=1` (selection index, sorted-merge diff). Ticket 09.
- `4053f07` v2 hardening: crash, eviction-race, GC-interaction tests;
  fixed sweep collecting `snapshots/index.tsv` as debris.
- `ed2c3ee` benchmarks: reproducible `v2-bench.sh`; Linux CI job.
- `f92e112` v2: whole-tree clone plus in-place delta replaces per-unit
  cloning (post-bump rebuild 3.4x faster than v1).

External-library evaluation (snapdir, clonetree/parcopy, pnpm-style
nlink GC) rejected with evidence: docs/adr/0006.

Earlier context: tickets 01–09 built the original tool; fast-hydration
tickets 02/03/05 added the ingest cache, CoW materialization, and the
verified-blob ledger. All documented under `.scratch/*/issues/`.

Phase 3 (parallelizing the snapshot miss path) was evaluated and
declined: raw measurements showed `link(2)` costs ~300µs/file and is
kernel-serialized on APFS (8 threads bought only ~25%), while skipping
no-op chmods got equivalent output for free. Rationale lives in ticket
08 and ADR-0005.

## 4. Measured performance (release build, Darwin arm64, APFS)

Large fixture: 40,000 files, 800 packages, 96% duplicate content
(only 1,648 unique blobs). Two measurement sessions: the first on an
idle machine (5am), the second with desktop load present; ratios are
consistent across both.

| Warm create | idle machine | loaded machine |
|---|---|---|
| fresh install baseline | 11.35s | — |
| direct recursive CoW clone (`cp -Rc`) | 7.95s | — |
| wt per-file ladder | 11.78s | ~13s |
| **wt, WT_SNAPSHOTS=1** | **6.5s** | **~1.6s** |
| wt, snapshots + no-refs cutover | 6.2s | ~1.5s |

The warm-hit jump between sessions is the Step 0 follow-up work:
ingest and references stages carried ~6s of syscall overhead that the
bulk walk, dirty-flag cache saves, and buffered ledgers removed.
Recursive clonefile was measured directly: ~0.45s for 40k files.

### Cold builds and v2 incremental rebuilds

Cold full build (first create after content change): ~15-19s of which
the link train dominates (APFS serializes link(2) at ~300us/file).
With `WT_SNAPSHOTS_V2=1`, a rebuild after small changes costs one
whole-tree clonefile plus O(changed) delta work:

| Scenario (loaded machine) | v1 rebuild | v2 incremental |
|---|---|---|
| dependency bump: 3 of 800 packages | 17.8s | **5.3s** |
| hit-rate poison: one `.DS_Store` | 18.0s | **5.7s** |

v2's residual cost is ingest (~3.3s loaded / ~0.6s warm-cache walk) +
references + git worktree add. Reproduce: `WT_BENCH_SAMPLES=2
./benchmarks/v2-bench.sh` (every hydrated tree diff -r'd against the
donor; mismatch aborts).

Small fixture (4k files): snapshots do not pay yet, wt warm is ~2.2s vs
baseline 1.9s.

Fidelity: snapshot-hydrated trees preserve symlinks, executable bits,
and empty directories exactly. The per-file ladder drops exec bits and
symlinks (counted and reported as known gaps by the suite's `--verify`;
bytes and presence are exact everywhere and any mismatch fails the run).
Under v2, `WT_VERIFY=1` hashes every staged file before publish.

Reproduce anything with:

```sh
cargo test                       # 167 tests
./benchmarks/run.sh --verify     # four scenarios, deep-verified
./benchmarks/v2-bench.sh         # v1-vs-v2 bump/poisoning table
```

Environment knobs: `WT_STORE`, `WT_SNAPSHOTS=1`, `WT_SNAPSHOTS_V2=1`,
`WT_VERIFY=1`, `WT_HARDLINK=1`, `WT_NO_HARDLINK=1`, `WT_GC_GRACE`
(e.g. 15m), `WT_TIMING=1`.

## 5. Known limitations

- Snapshots are macOS/APFS only and opt-in. Linux has no recursive
  reflink primitive, so the gate is a no-op there by design.
- The per-file ladder's fidelity gaps above; use the gate when they matter.
- A same-size, same-mtime bit flip can slip past the verified ledger
  between checks. That is the accepted trust model; `WT_VERIFY=1` exists
  for paranoid runs. A scrub command is the future answer, not more
  hashing on hot paths.
- Cold snapshot builds are expensive (~24s at 40k scale) because APFS
  serializes `link(2)`. Amortized over repeated creates this wins big,
  but a first create after large dependency changes feels slower than
  the ladder.
- Reflink materialization on Linux is implemented but never exercised
  by CI.

## 6. What to do next, in order

1. **Soak `WT_SNAPSHOTS=1` on real agent workloads.** Use it daily on
   actual repositories. If hit/miss behavior, fidelity, and speed hold,
   decide whether to flip the default on. Ticket 08 holds the numbers
   this decision should rest on.
2. **Run the GC cutover on your real store**, only after you are sure
   no pre-cutover binary will touch it again:
   ```sh
   wt store migrate --activate-mark-sweep
   wt store migrate --drop-legacy-refs   # loud warning; irreversible stance
   ```
   Until then everything works in dual-write mode; it is safe, just not
   yet faster at the references stage.
3. **Finish distribution leftovers from original ticket 09**: push the
   repo, tag v0.1.0 to exercise release CI, set signing secrets, verify
   a fresh-machine install from a real release artifact.
4. **Linux validation pass**: exercise the reflink backend and the
   snapshot gate no-op path in CI, ideally with a Linux job running the
   benchmark suite in `--quick --verify` mode.
5. **v2 ideas, deliberately deferred**: per-subtree snapshots with
   diff-based rebuilds, bounded LRU retention for unreferenced snapshots,
   a scrub command for continuous corruption detection, faster cold
   builds if Apple ever makes `link(2)` less serialized.

## 7. Where everything lives

- Specs and tickets: `.scratch/fast-hydration/issues/01..08`,
  `.scratch/fast-hydration/spec.md`, `perf-handoff.md` (the earlier
  problem brief that kicked this off).
- Decision records: `docs/adr/0001..0005`.
- Glossary: `CONTEXT.md`.
- Benchmarks: `benchmarks/run.sh --verify` (scenarios a,b,c,d),
  fixture generators in `benchmarks/fixture.sh`.
- Codebase-memory index: project `instant-worktrees`, current as of
  HEAD (976 nodes / 4,094 edges).
