# 03: CoW materialization as default

**What to build:** Hydrated worktrees behave like normal writable checkouts. Files materialize from the store as per-file CoW clones (`fclonefileat` on macOS): each destination file gets a fresh private inode sharing the blob's physical blocks until first write. Nothing strips write bits, so tools that rewrite files in place succeed silently and privately instead of failing with permission errors. Deduplication survives the change — before any write, a fleet of hydrated trees still shares every byte physically.

Hardlinks stop being the default. They stay available behind an explicit opt-in flag for maximum space sharing, documented as experimental, with their known failure mode (in-place rewrites fail loudly) stated in help text. Filesystems that refuse CoW fall back to byte copies; Linux reflink will join the front of the fallback order once validated there.

One design seam needs real work: the copy-backend trait is directory-shaped (clone or walk a whole source directory), but store-backed materialization is file-shaped (blob at address to destination path). This ticket introduces the file-shaped materialization interface — the hardlink path already has the right shape and becomes its first implementation; the new CoW path becomes the second.

**Blocked by:** None (can start immediately).

**Status:** ready-for-agent (implemented on `fleet/03-cow-materialize`)

- [x] Default hydration produces private, writable files with normal permissions on APFS
- [x] In-place rewrite of a hydrated file succeeds and stays private to that worktree
- [x] Before first write, hydrated trees share physical blocks with the store (measured, not asserted)
- [x] A second create against an unchanged store adds no new store content — dedup preserved
- [x] Corrupt store content still fails loudly before landing in a fresh tree
- [x] Filesystems refusing CoW fall back to byte copies without user-visible failure
- [x] Opt-in flag restores hardlink behavior; default path no longer links shared inodes
- [x] GC reference counting works unchanged: removal after `flashwt remove` releases what hydration claimed
- [x] e2e tests assert writability, privacy-after-write, dedup, corruption, and GC through the existing CLI seam
