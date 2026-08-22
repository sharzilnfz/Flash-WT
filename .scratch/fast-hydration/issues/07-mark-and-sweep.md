# 07: Store-local mark-and-sweep (dual-write transition)

Implements Phase 1 of AGENT_HANDOFF_PLAN_REVISED.md. Authoritative GC roots move to `<store>/worktrees/<key>.tsv` mirrors (typed v1 records, atomic publish), written once per create instead of 4,000 refcount rewrites. Legacy `refs/` continues to be maintained until explicit cutover so older binaries never collect live data.

**Status:** ready-for-agent (blocked-by: none)

- [ ] Mirrors published atomically; torn/malformed mirrors handled per plan
- [ ] Root validation is filesystem-existence based; no `git worktree list`
- [ ] Grace period (`WT_GC_GRACE`, default 15m) gates all collection
- [ ] `WT_TIMING=1` emits `wt-stage <name>=<ms>` lines for ingest / references / materialize / total
- [ ] Gated modes: legacy sweep (default), `--activate-mark-sweep`, `--drop-legacy-refs` (explicit, warned)
- [ ] All Phase 1 tests from the plan pass, including out-of-band deletion and kill-recovery
