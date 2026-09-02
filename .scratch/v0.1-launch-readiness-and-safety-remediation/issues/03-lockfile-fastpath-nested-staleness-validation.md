Status: ready-for-agent

# Issue 03: Lockfile Fast-Path Nested Staleness Validation

## Problem
On macOS/APFS, `try_lockfile_hit` skips the tree walk if the lockfile hash matches and the top-level directory's `mtime` hasn't changed. If a nested file inside `node_modules/` or `target/` is modified without changing the lockfile, the top-level directory mtime remains unchanged, and the engine serves a stale snapshot without detecting the nested edits.

## Requirements
1. Enhance fast-path cache validation to verify directory contents or perform a shallow bulk stat scan before declaring a snapshot cache hit.
2. Invalidate and re-scan the tree if any nested file modification timestamp is newer than the snapshot manifest creation time.

## Verification
- Add integration test modifying a nested file in `node_modules` with an unchanged lockfile, creating a new worktree, and asserting that the modified file content is correctly materialized in the destination.
