Status: ready-for-agent

# Issue 09: Standalone Hydration Command & Product Renaming

## Problem
1. The tool currently insists on managing the full Git worktree creation lifecycle, competing directly with tools like Worktrunk and agent orchestrators.
2. The binary name `flashwt` collides directly with *Worktrunk*, causing installation conflicts and positioning ambiguity.

## Requirements
1. Expose a standalone `hydrate <path>` command that discovers and hydrates heavy directories into an already-existing directory or worktree.
2. Define a canonical package/binary name (e.g. `flashwt`, `flashwt`, `dew`) to prevent conflicts with Worktrunk, while providing `flashwt` as an optional alias.
3. Stop writing `.flashwtinclude` automatically into the source checkout during `new`. Provide an explicit `init` command or use in-memory defaults.

## Verification
- Add integration test for `flashwt hydrate <existing-dir>` verifying that heavy directories are properly ingested and materialized into existing worktrees.
