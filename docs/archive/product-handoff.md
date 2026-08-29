# wt — whole-product handoff

Date: 2026-08-23. This document is self-contained and separate from the
working handoffs under `.scratch/`. It answers: what is this product,
how does it work, why does it matter, where does it stand, and what is
left to do.

---

## 1. What the product is

`wt` is a single, dependency-free binary that creates git worktrees with
heavy untracked directories already in place.

```
wt create my-feature
```

One command produces a new worktree at `<repo>-my-feature` on a new
branch, with `node_modules/`, `target/`, caches, and whatever else the
project's `.wtinclude` manifest lists — hydrated instantly through a
content-addressed store. Nothing is re-downloaded, re-installed, or
rewritten byte-by-byte; files are hardlinked or copy-on-write cloned
out of a local object store, the same way git shares objects between
clones.

- One static Rust binary. No runtime dependencies beyond git itself.
- Works on any ecosystem: JavaScript, Rust, Python, anything that
  produces heavy directories.
- Free, open source (MIT), installs in one command.
- Humans keep working exactly as before; only environment setup
  changes.

## 2. The problem

Agentic coding multiplies environments. Every agent session, feature
branch, or parallel task wants its own checkout — and every checkout
pays the same tax:

- `npm install` into a fresh `node_modules`: minutes and hundreds of
  megabytes, repeated per worktree.
- `cargo build` from zero into an empty `target/`: many minutes.
- Caches (`.venv`, build outputs) rebuilt from nothing each time.

Humans tolerate this cost a few times a day. Agents pay it constantly:
spawning five parallel workers means five reinstalls. The bottleneck is
not computation, it is small-file churn — creating tens of thousands of
files whose contents already exist somewhere else on the same disk.

## 3. The solution

A content-addressed store (`~/.cache/wt/store` by default, overridable
with `WT_STORE`) sits beside your projects:

1. **Ingest** — on first use, unique file contents from heavy
   directories are hashed (SHA-256) and stored once as immutable
   blobs. Duplicate content is stored once (a typical JS project is
   ~96% duplicates).
2. **Hydrate** — a new worktree's heavy directories are materialized
   from blobs via copy-on-write clones (`fclonefileat` on APFS) or
   hardlinks. Files share the store's physical blocks until first
   write; they are private, fully writable, and indistinguishable from
   real files to editors and build tools.
3. **Snapshot cache** (opt-in, macOS/APFS, `WT_SNAPSHOTS=1`) — whole
   directory trees are cached as snapshot images; a matching hydrate
   becomes one recursive `clonefile(2)` call (~0.45s for 40,000
   files).
4. **v2 incremental rebuilds** (opt-in, `WT_SNAPSHOTS_V2=1`) — when
   dependencies change slightly, the previous snapshot is diffed
   against the new state and the rebuild is one whole-tree clone plus
   an in-place delta of just the changed paths. No full relink train.
5. **GC** — mark-and-sweep collection with a grace period deletes only
   content no live worktree references. Crash tests (SIGKILL sweeps)
   prove a kill at any instant can leak reclaimable cache data but
   never destroy live data.

Integrity model: blobs are hash-verified once and then trusted while
size+mtime stay unchanged (a verified-blob ledger tracks this);
`WT_VERIFY=1` forces full re-hashing for paranoid runs, and under v2
it hashes every staged file before publishing a snapshot.

## 4. Measured benefits (40k files / 800 packages fixture)

| Scenario | Baseline | With wt |
|---|---|---|
| Warm environment create | 11.35s fresh install | **~1.5s** |
| Same vs raw APFS clone (`cp -Rc`) | 7.95s | ~1.5s |
| Rebuild after dep bump (3/800 pkgs changed) | 17.8s full rebuild | **5.3s** |
| Rebuild after junk file poisons the tree | 18.0s full rebuild | **5.7s** |

Additional benefits that don't show in wall time:

- **Disk**: duplicate content stored once across all projects.
- **Fidelity**: snapshots preserve symlinks, exec bits, and empty
  dirs exactly; the fallback ladder counts and reports its known gaps.
- **Safety**: GC never collects referenced data; publishes are atomic
  renames; 167 tests including SIGKILL chaos coverage.
- **Reproducibility**: every benchmark verifies hydrated trees
  byte-for-byte against donors; mismatches abort.

## 5. Where it stands in the market

**Category**: developer tooling for environment management; riding the
agentic-coding wave where parallel machine environments are becoming
the default workflow.

**Adjacent tools and why wt is different:**

| Alternative | Gap wt fills |
|---|---|
| Plain `git worktree add` + reinstall | Worktrees share tracked files but not heavy untracked dirs; every worktree still pays the install. wt makes the untracked bulk free. |
| pnpm / yarn PnP | Dedupe within one JS package manager's layout. wt is ecosystem-agnostic and works at the filesystem level for any heavy directory. |
| `cp -Rc` / clonefile scripts | A bare recursive APFS clone measures ~8s at 40k files and has no dedup across projects, no integrity checking, no GC, no CLI. wt warm path is faster (~1.5s) because it skips unchanged subtrees entirely. |
| Docker / devcontainers | Solve reproducibility, not iteration speed on the same host; heavyweight and a workflow change. wt changes nothing about how you work. |
| Build caches (turborepo, nx, sccache) | Cache build *outputs* keyed by inputs. wt solves environment *materialization* — a complementary layer, not a competitor. |

**Honest positioning statement**: wt is fastest on macOS/APFS where
clonefile exists; Linux works via reflink/fallback copies but without
the snapshot fast path. It is v0.1.0-maturity: feature-complete for
its core promise, hardened by tests, not yet soaked on third-party
workloads.

## 6. Current state (as of this handoff)

- **Code**: three crates — `wt-cli` (command surface, hydration
  orchestration), `wt-store` (object store, snapshots, snapdiff/snapindex,
  bulk walker, GC, verified ledger), `wt-copy` (copy backends: clonefile,
  hardlink, deep copy). Workspace builds with LTO, stripped release.
- **Quality gates**: 167 tests green; `cargo fmt` and
  `cargo clippy --all-targets -D warnings` clean; CI runs these on both
  macOS and ubuntu.
- **Distribution**: release workflow cuts signed/notarized macOS
  binaries plus unsigned Linux tarballs, generates SHA-256 checksums
  and a Homebrew formula; `install.sh` verifies checksums;
  `scripts/smoke-install.sh` exercises both install paths.
- **Commands**: `wt create`, `wt remove`, `wt sweep`,
  `wt store migrate` (GC cutover).
- **Design records**: six ADRs in `docs/adr/` covering store-as-truth,
  tool-agnostic hydration, single-binary stance, mark-and-sweep GC,
  directory snapshots, and the evaluation (and rejection, with
  evidence) of external libraries (snapdir, clonetree, pnpm-style
  refcount GC).
- **Benchmarks**: `benchmarks/run.sh` (four scenarios, deep verify)
  and `benchmarks/v2-bench.sh` (v1-vs-v2 comparison), both
  reproducible and committed.

### Known limitations (documented, accepted)

- Snapshot fast paths are macOS/APFS only; Linux takes slower fallbacks.
- Cold builds after large dependency changes still pay a serialized
  link train (~15–19s at 40k scale) once per materially changed tree.
- A same-size, same-mtime bit flip can slip past the trust model
  between checks; scrub command is future work.
- Both performance gates are opt-in pending real-world soak.

## 7. What comes next, in order

1. Push to GitHub (`github.com/sharzilnfz/wt`) and cut `v0.1.0` so
   release CI produces real artifacts; verify curl and brew installs
   end-to-end.
2. Soak both gates (`WT_SNAPSHOTS=1 WT_SNAPSHOTS_V2=1`) on daily
   workloads; decide default-on based on tickets 08/09 data.
3. Run the GC cutover on the live store (`wt store migrate
   --activate-mark-sweep`, later `--drop-legacy-refs`).
4. Deferred ideas: LRU retention cap for unreferenced snapshots,
   scrub command, cold-build speedup if APFS link serialization ever
   improves.

## 8. Map of the repository

```
crates/
  wt-cli/      CLI, hydration orchestration, integration tests
  wt-store/    object store, snapshots, diff/index, bulk walk, GC, ledger
  wt-copy/     clonefile/hardlink/deep-copy backends
benchmarks/    run.sh (scenarios a–d), v2-bench.sh, fixture.sh
docs/adr/      architecture decision records 0001–0006
Formula/       Homebrew formula (generated, carries checksums)
scripts/       smoke-install.sh, gen-formula.sh
.github/       ci.yml (mac+linux checks), release.yml (sign+publish)
CONTEXT.md     domain glossary
README.md      public-facing overview and quick start
```
