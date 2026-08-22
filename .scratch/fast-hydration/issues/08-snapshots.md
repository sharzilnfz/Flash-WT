# 08: Whole-directory snapshots (macOS/APFS, opt-in)

Implements Phase 2 of AGENT_HANDOFF_PLAN_REVISED.md. One snapshot per heavy directory behind `WT_SNAPSHOTS=1`: canonical manifest, hardlink-to-blob tree, atomic publish, single recursive `clonefile(2)` on hits, integrity at publish, `WT_VERIFY=1` bypasses hits.

**Status:** blocked-by: 07 (needs mirror `snapshot` records and snapshot-aware GC)

- [ ] All Phase 2 tests from the plan pass (round trip, races, healing, fallbacks)
- [ ] Hit = one clone call per heavy dir plus one mirror write
- [ ] Linux/unsupported filesystems: gate is a no-op, existing ladder intact
- [ ] Warm benchmark numbers recorded before any default-on decision
