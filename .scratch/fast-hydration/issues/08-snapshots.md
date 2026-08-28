# 08: Whole-directory snapshots (macOS/APFS, opt-in)

Implements Phase 2 of AGENT_HANDOFF_PLAN_REVISED.md. One snapshot per heavy directory behind `WT_SNAPSHOTS=1`: canonical manifest, hardlink-to-blob tree, atomic publish, single recursive `clonefile(2)` on hits, integrity at publish, `WT_VERIFY=1` bypasses hits.

**Status:** done (merged; default remains off pending real-world soak)

- [x] All Phase 2 tests from the plan pass (round trip, races, healing, fallbacks)
- [x] Hit = one clone call per heavy dir plus one mirror write
- [x] Linux/unsupported filesystems: gate is a no-op, existing ladder intact
- [x] Warm benchmark numbers recorded before any default-on decision

## Measured numbers (Darwin 25.6.0 arm64, APFS, release build)

Large fixture: 40,000 files, 800 packages, 96% duplicate content (1,648 unique blobs).

| Scenario | Cold | Warm |
|---|---|---|
| fresh install baseline | 11.52s | 11.35s |
| direct recursive CoW (`cp -Rc`) | 7.97s | 7.95s |
| wt per-file ladder (dual-write GC) | 14.99s | 11.78s |
| wt mark-sweep cutover only | — | 11.7s |
| **wt `WT_SNAPSHOTS=1` (miss builds / hit clones)** | 24.1s | **6.5s** |
| **wt snapshots + mark-sweep-no-refs** | — | **6.2s** |

Stage split, snapshot cold: ingest 5.7s + references 2.6s + build/publish 15.4s.
Snapshot hits also preserve symlinks and executable modes that the per-file
ladder drops (the suite's fidelity-gap report drops to zero under the gate).

## Phase 3 decision (parallelize miss path): declined, with data

Raw measurements on this machine: `link(2)` costs ~300µs/file and is
kernel-serialized on APFS — an 8-thread link loop improves only 12.05s →
9.05s (~25%), while adding thread-pool error semantics to the builder.
Instead the build now skips chmod when the freshly linked inode already
carries the entry's normalized mode (most files need no change), which is
equivalent output for a fraction of the complexity. Revisit if a future
macOS release changes link(2) throughput or if v2 subtree snapshots make
builds more frequent.

## Follow-ups

- Default-on decision after a soak period on real agent workloads.
- Per-subtree snapshots and diff-based rebuilds (v2).
