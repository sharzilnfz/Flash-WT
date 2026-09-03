# 05: Wire hydration end to end through the store

**What to build:** The product moment. Hydration stops copying from the
source checkout directly and flows through the store instead: heavy
directories are ingested into the store, then materialized into new worktrees
from it. Two worktrees of the same project share content and disk usage drops.
One agent owns this ticket alone because it merges three parallel branches and
needs a single mind resolving conflicts.

**Blocked by:** 02 (worktree command), 03 (copy backends), 04 (store).

**Status:** ready-for-agent

- [x] `flashwt create` hydrates via store ingest plus materialize, not direct copies
- [x] Second worktree of the same project adds near-zero duplicate bytes on disk
- [x] End-to-end test proves dedupe across two worktrees through the CLI seam
- [x] Hash mismatch during materialize fails loudly
- [x] Full suite green after merging branches from tickets 02-04

## Comments

### What was built (orchestrator, ticket 05)

`crates/flashwt-cli/src/hydrate.rs`: ingest walks a heavy directory and
puts every file into the store; materialize recreates the tree from
hash-verified blobs. Store location is `$FLASHWT_STORE`, else XDG cache
(`~/.cache/flashwt/store`). Each worktree claims one ref per distinct blob
and writes `<gitdir>/flashwt-hydrated.tsv` (`<relpath>\t<contentid>`) as
the ledger for ticket 06. "Near-zero duplicate bytes" is asserted as
an unchanged store footprint (object count + byte total) after the
second create — the honest CLI-seam observable, since APFS clones do
not preserve inodes and `du` overcounts shared blocks.

The direct-copy path from ticket 02 (`backends()`/`hydrate_one`) was
removed; flashwt-cli no longer depends on flashwt-copy. Ticket 02's e2e output
contract ("hydrated <dir>" plus source path) is preserved, now with
"via store" appended.
