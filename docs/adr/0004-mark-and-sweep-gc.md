# Store-local mark-and-sweep garbage collection

We considered keeping refcount-driven collection (ticket 06) as the only
scheme, but `flashwt create` pays one temp-write-rename refcount update per
distinct blob — roughly half a second on a 4,000-file fixture — and
refcounts cannot answer "which worktrees still need this blob?" without
scanning every repository on the machine. Instead, each successful create
publishes one store-local mirror naming its blobs, and sweep collects from
those mirrors. This implements Phase 1 of AGENT_HANDOFF_PLAN_REVISED.md
(fast-hydration ticket 07).

## Decision

- **Mirrors are the GC roots.** `<store>/worktrees/<key>.tsv`, where key is
  SHA-256 of `version=1 \0 <canonical worktree path> \0 <canonical gitdir
  path>`. Typed TSV records (`v1 worktree / file / snapshot`), paths
  percent-escaped, published by write-temp-then-rename. One atomic write
  per successful create replaces thousands of per-blob ref writes.
- **Root validation is filesystem-existence based**: recorded worktree path
  is a directory, gitdir exists, and either the `flashwt-hydrated.tsv` sidecar
  survives or the mirror is younger than the grace period. No
  `git worktree list`: git's administrative records outlive `rm -rf` until
  pruned, so they are not a liveness oracle.
- **Grace period gates everything**: default 15 minutes (`FLASHWT_GC_GRACE`),
  overridable per sweep with an explicit `--age`. Blobs, snapshots,
  snapshot temp data, and stale mirrors are all deletable only past the
  grace period. A mirror inside the grace window keeps protecting its
  recorded blobs even when root validation fails — waiting costs disk;
  collecting early could cost a live tree.
- **Snapshots are caches, not roots** (Phase 2 rule applied up front): a
  snapshot survives only while referenced by a live mirror or inside the
  grace period; unresolvable snapshot records mark through nothing.
- **Malformed mirrors are quarantined, never emptied**: an unparsable
  mirror younger than the grace window defers all deletion that pass and
  is reported; it is never silently treated as having zero roots.

## Dual-write transition and downgrade safety

An old binary reading a store whose ref files vanished would treat missing
as zero and collect live data. So refs/ maintenance stays untouched until
an explicit cutover:

1. **Now (dual-write)**: creates/removes keep maintaining `refs/` exactly
   as before; mirrors are additive. Legacy-mode sweeps run a mark-vs-refs
   audit and print disagreements as `flashwt-gc-audit:` lines — parity evidence,
   silent when the two schemes agree.
2. **`flashwt store migrate --activate-mark-sweep`**: sets `gc-mode=mark-sweep`.
   Sweep collects from live-mirror marks plus the grace period; refs/ stay
   maintained but ignored for liveness. Pre-change binaries remain safe.
3. **`flashwt store migrate --drop-legacy-refs`**: loud warning, purges every
   ref file, sets `gc-mode=mark-sweep-no-refs`; creates stop touching
   refs/ entirely. One-way; pre-cutover binaries must not use the store.

## Consequences

- Warm create performs one atomic mirror write instead of one ref write per
  distinct blob; the full timing win lands only after the explicit cutover.
- Sweep stays store-local: no machine-wide scan, no daemon, no repo-global
  registry.
- A kill before mirror rename leaves no new root (object-age grace protects
  fresh content); a kill after rename leaves a complete root; a kill
  mid-sweep leaves states the next sweep reconciles.
- Two metadata systems exist during dual-write; they are removed only at
  step 3, which the operator must invoke deliberately.
