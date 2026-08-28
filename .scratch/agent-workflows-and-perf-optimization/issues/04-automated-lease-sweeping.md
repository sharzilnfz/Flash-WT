# 04: Automated lease sweeping for dead or expired sandbox worktrees

**What to build:** Extend `wt sweep` to inspect all active `.lease` files under `<store>/worktrees/`. Reclaim scratch worktree trees, git references, and store mirror files whose owning process is dead, whose start time has shifted (detecting PID reuse after reboot or kill), or whose lease TTL expiration has passed. Ensure agent crashes and unmaskable termination signals leave zero orphaned disk usage.

**Blocked by:** 03: Ephemeral scratch and isolate worktrees with lease persistence.

**Status:** ready-for-agent

- [ ] `wt sweep` scans and parses all `.lease` files in `<store>/worktrees/`.
- [ ] Process liveness and start time fingerprint verification checks whether the owning process is still alive.
- [ ] Orphaned, dead-process, and expired lease worktrees are cleanly removed from disk and git worktree tracking.
- [ ] Associated store mirrors and unreferenced blobs are queued for collection during store GC.
- [ ] Sweep summaries and `--json` outputs report counts and reclaimed disk metrics for expired leases.
- [ ] Concurrency and recovery integration tests verify clean reaping under simulated dead process IDs.
