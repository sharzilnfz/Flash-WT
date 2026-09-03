# Whole-directory snapshots on APFS

We considered keeping per-file placement as the only hydration path, but a
warm create pays an open + `fclonefileat` train for every file — thousands
of syscalls where the filesystem offers one. APFS can clone a whole
directory tree in a single recursive `clonefile(2)`, so `flashwt` now keeps
rebuildable whole-directory snapshots in the store and clones one out per
heavy directory on a hit. This implements Phase 2 of
AGENT_HANDOFF_PLAN_REVISED.md (fast-hydration ticket 08).

## Decision

- **One snapshot per heavy directory**, keyed by the SHA-256 of its
  canonical manifest: `<store>/snapshots/<manifest-hash>/{manifest.tsv,
  .complete, tree/}`. The clonable subtree lives under `tree/` so metadata
  files never leak into a worktree (a deliberate deviation from the plan's
  flat sketch, needed because `clonefile` would otherwise copy the
  manifest into every hydrated directory).
- **The manifest is the contract.** Typed TSV entries (`file`, `symlink`,
  `dir`) with percent-escaped paths, normalized modes (0755/0644 by
  executable-ness), sorted by raw path bytes, hash computed over entry
  bytes only. Identical content therefore shares one snapshot across
  worktrees and machines-with-identical-trees.
- **Integrity at publish.** Every file blob is proven before it is
  hardlinked into the staging tree (verified-ledger trust, or full hashing
  under `FLASHWT_VERIFY=1`, which also bypasses hits entirely). After the
  atomic rename, a hit performs zero blob reads — the same trust model as
  verified-ledger materialization, not a weaker one.
- **Publish is the only write.** Staging under `snapshots/tmp/`, then one
  rename. A concurrent loser validates the winner and uses it; invalid
  debris is never overwritten.
- **Snapshots are cache, not truth.** The store's objects remain the only
  durable content; a snapshot is rebuilt from blobs any time it is missing.
  GC (ADR-0004) collects unreferenced snapshots after the grace period.

## Consequences

- A snapshot hit costs one syscall chain per heavy directory plus one
  mirror write, instead of N file placements.
- Linux gets nothing yet: there is no single-call recursive reflink, so
  the gate is a no-op there rather than a disguised per-file copy.
- Opt-in via `FLASHWT_SNAPSHOTS=1` until parity and benchmark gates pass;
  default stays off.

## Amendment (2026-08-26)

The per-file fallback ladder no longer skips symlinks or normalize away
permission bits: ingest records symlink targets and file/directory modes
for every entry, and materialize restores them exactly (hardlinked files
whose exec bits disagree with the record are replaced by private copies
rather than chmod-ing the shared blob). Snapshot parity and ladder parity
are now identical; the benchmark suite's gap tolerance exists only as a
regression tripwire. Unreferenced snapshots are additionally bounded by
an LRU retention cap (`FLASHWT_SNAPSHOT_CAP`, default 64).

## Amendment (2026-08-29)

Snapshots are no longer opt-in. The original gate kept the default off
until parity and benchmark gates passed; those gates passed, so `flashwt` now
probes the host at startup and enables snapshot hydration by default on
macOS APFS. `FLASHWT_SNAPSHOTS=0` opts out and forces the per-file ladder;
`FLASHWT_VERIFY=1` still bypasses hits entirely. The parity tripwire and the
LRU/disk caps above carry over unchanged.
