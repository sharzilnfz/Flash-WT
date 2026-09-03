Status: ready-for-agent

# Issue 02: Lease Sweep Refcount Deduplication & Inode Protection

## Problem
During ephemeral lease cleanup, `StoreReclaimer::sweep_leases` iterates line-by-line over `flashwt-hydrated.tsv` and calls `release_ref(cid)` for every entry without deduplication. When a worktree contains duplicate files (e.g. duplicate license files or headers), refcounts are decremented multiple times for a single blob, draining reference counts to 0 while other active worktrees still share the blob. In addition, snapshot tree staging uses `fchmodat` on hardlinked files, which mutates shared CAS inode permissions in `objects/`.

## Requirements
1. Collect all `ContentId`s into a deduplicated set (`BTreeSet<ContentId>`) before invoking `release_ref` during lease sweeps.
2. Ensure every unique blob referenced by a lease-backed worktree is decremented exactly once upon retirement.
3. Eliminate in-place permission mutation on shared CAS inodes during snapshot staging.

## Verification
- Add integration test creating scratch worktrees with duplicate files, retiring them, and verifying that shared blobs in sibling worktrees remain accessible and valid.
