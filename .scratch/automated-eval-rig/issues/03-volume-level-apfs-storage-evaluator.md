# 03: Volume-level APFS and Linux storage footprint evaluator

**What to build:** A storage accounting subsystem that queries volume-level free space (`statvfs`) on isolated test volumes or roots before and after hydration, mutation, and garbage collection, distinguishing true private physical disk consumption from shared clonefile blocks.

**Blocked by:** 01: JSON metrics schema and stage metrics collector

**Status:** ready-for-agent

- [ ] Volume free-space probe implementing OS-level `statvfs` / `fstatvfs` queries to capture available block and fragment counts before and after operations.
- [ ] Physical storage accounting engine measuring net private disk footprint after multiple worktree creations on APFS and Linux reflink filesystems.
- [ ] GC reclamation evaluator verifying that unreferenced blob and snapshot pruning reclaims the expected physical bytes after `wt sweep`.
- [ ] Deduplication efficiency calculator reporting actual physical storage multiplier across N identical and divergent worktrees.
- [ ] Automated assertions validating that APFS clonefile shared extents consume zero additional physical data blocks upon initial creation.
