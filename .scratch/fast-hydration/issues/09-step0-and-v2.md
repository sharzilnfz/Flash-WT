# 09: Step 0 instrumentation, overhead fixes, and v2 diff-based rebuilds

**Status:** in progress

## Step 0: resolve H1 vs H2 (done)

Fine-grained `WT_TIMING` stages landed (`5d34edc`): `git-worktree`,
`ingest`, `references`, `verify`/`place`, `snapshot-lookup`,
`snapshot-clonefile`, `snapshot-build-{verify,link-train,publish}`,
plus `snapshot-mode=hit|build|v2` and v2 counters later.

**Verdict: H1 true.** Recursive `clonefile(2)` for a 40k-file tree
costs ~0.45-0.7s. The "unexplained" seconds lived in:

- ingest: read_dir + per-file metadata stat (~80k syscalls) and an
  unconditional full rewrite of the validation-cache TSV;
- references: unbuffered line-at-a-time sidecar writes plus legacy
  refcount temp+rename churn.

Fixes (`fdba0df`): macOS `getattrlistbulk(2)` bulk walk (differential-
tested against the legacy walker), dirty-flag cache saves, buffered
sidecar writers.

Warm snapshot hit at 40k files: **6.5s -> ~1.4s** (clean machine).

## v2 incremental rebuilds (`9f1f0e6`, gate WT_SNAPSHOTS_V2=1)

Selection index (`snapshots/index.tsv`, ring of K=3 hashes),
tamper-evident manifest loader, sorted-merge diff with maximal
unchanged-subtree units, incremental publish with per-unit clonefile
and per-unit link fallback. Any failure falls back to the full v1
build; `WT_VERIFY=1` hashes cloned units before rename.

Measured (loaded machine; ratios trustworthy):

| scenario | v1 rebuild | v2 incremental |
|---|---|---|
| bump 3/800 packages | 17.8s | **5.3s** |
| one `.DS_Store` poison | 18.0s | **5.7s** |

## Whole-tree-clone delta (final mechanism, `f92e112`)

Per-unit cloning was measured wasteful: 813 package-dir clones x ~7ms
call overhead ~= 5.7s of snapshot stage even when ~20 files changed.
Replaced with ONE clonefile of the old tree into staging plus an
in-place delta inside the private CoW copy (deepest-first deletions,
verify-then-link for added/modified blobs, chmod for mode flips,
structural+hash pass under WT_VERIFY=1). Snapshot stage collapsed from
6.9s to well under 1s; post-bump totals landed at 5.3s loaded (~2.5s
clean-machine estimate).

Hardening (`4053f07`) found one real bug: mark-and-sweep collected
`snapshots/index.tsv` as debris, silently disabling v2 selection
store-wide after any sweep past grace. Fixed by exact-name exemption;
regression-tested. Crash/kill sweeps, eviction races, GC shared-subtree
survival, and hostile-index-layout resilience all covered (167 tests).

- [x] Step 0 breakdown published, H1 confirmed, overheads fixed
- [x] Selection index + diff + incremental rebuild behind gate
- [x] Crash, eviction-race, GC-interaction coverage (hardening pass)
- [x] Whole-tree-clone delta strategy replacing per-unit clones
- [x] Re-measure E/F after refactor; recorded in HANDOFF.md §4
- [x] GC validation sign-off on shared-subtree snapshots

**Status:** done
