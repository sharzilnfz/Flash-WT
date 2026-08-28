# 06: Garbage collection

**What to build:** Removing worktrees releases their references, and an
age-based sweep reclaims unreferenced store entries so the store never grows
without bound. Proven end to end: delete everything, run the sweep, watch the
store shrink through the CLI seam.

**Blocked by:** 05 (wire hydration through store).

**Status:** ready-for-agent

- [x] `wt remove` or equivalent releases worktree references in the store
- [x] Sweep deletes only unreferenced entries past an age threshold
- [x] Referenced entries survive aggressive sweeping
- [x] End-to-end test: full lifecycle create-create-remove-sweep leaves a minimal store
- [x] Sweep is interruptible and leaves the store consistent if killed mid-run

## Comments

### What was built (agent, ticket 06)

`crates/wt-cli/src/gc.rs`: `wt remove NAME` resolves the worktree's
linked git dir while it still exists, reads `wt-hydrated.tsv`, releases
one reference per distinct blob (underflow tolerated so an interrupted
remove can be rerun), drops the ledger, then runs `git worktree
remove`. Releasing must precede removal because git deletes the git dir —
the ledger with it. `wt sweep --age <dur>` (default 7d; `0s`, `90s`,
`10m`, `24h`, `7d` accepted) reclaims entries whose ref count is zero
and whose object mtime predates the cutoff.

`crates/wt-store`: `DiskStore::{ids, delete, sweep}` plus
`ContentId::from_hex`. The sweep's deletion order — ref file first,
object second — makes interruption safe at any point: a kill between
the two leaves an unreferenced object, a state the next sweep already
reclaims; a ref file never outlives its object. The age floor protects
content that hydration has put but not yet claimed references on.

Tests live in `crates/wt-cli/tests/gc.rs`, asserted only through the
CLI seam and files on disk: reference release observable as ref-count
files deleted or at zero; aggressive sweep (`--age 0s`) spares
surviving worktrees and the store still hydrates byte-identical trees;
create-create-remove-remove-sweep empties the store to zero files; a
simulated kill (orphaned object) is reclaimed exactly once and the
store stays usable.

