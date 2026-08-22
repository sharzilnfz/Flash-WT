# 06: Garbage collection

**What to build:** Removing worktrees releases their references, and an
age-based sweep reclaims unreferenced store entries so the store never grows
without bound. Proven end to end: delete everything, run the sweep, watch the
store shrink through the CLI seam.

**Blocked by:** 05 (wire hydration through store).

**Status:** ready-for-agent

- [ ] `wt remove` or equivalent releases worktree references in the store
- [ ] Sweep deletes only unreferenced entries past an age threshold
- [ ] Referenced entries survive aggressive sweeping
- [ ] End-to-end test: full lifecycle create-create-remove-sweep leaves a minimal store
- [ ] Sweep is interruptible and leaves the store consistent if killed mid-run
