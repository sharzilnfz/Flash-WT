# 05: Wire hydration end to end through the store

**What to build:** The product moment. Hydration stops copying from the
source checkout directly and flows through the store instead: heavy
directories are ingested into the store, then materialized into new worktrees
from it. Two worktrees of the same project share content and disk usage drops.
One agent owns this ticket alone because it merges three parallel branches and
needs a single mind resolving conflicts.

**Blocked by:** 02 (worktree command), 03 (copy backends), 04 (store).

**Status:** ready-for-agent

- [ ] `wt create` hydrates via store ingest plus materialize, not direct copies
- [ ] Second worktree of the same project adds near-zero duplicate bytes on disk
- [ ] End-to-end test proves dedupe across two worktrees through the CLI seam
- [ ] Hash mismatch during materialize fails loudly
- [ ] Full suite green after merging branches from tickets 02-04
