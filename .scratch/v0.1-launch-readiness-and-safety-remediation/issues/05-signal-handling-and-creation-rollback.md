Status: ready-for-agent

# Issue 05: Process Signal Handling and Worktree Creation Rollback

## Problem
1. The CLI has no `SIGINT`/`SIGTERM` handlers. Pressing `Ctrl+C` terminates the process immediately, bypassing RAII drops and leaving orphaned scratch worktrees, git branches, and lease files on disk.
2. `wt new` creates the Git branch and worktree before store ingestion and hydration. Any subsequent error exits immediately, leaving an unhydrated orphan worktree behind.

## Requirements
1. Register `SIGINT`/`SIGTERM` signal handlers in `main.rs` that execute cleanup for active scratch workspaces and temporary resources before exiting.
2. Implement transactional creation in `commands/create.rs`: if store opening, manifest reading, ingestion, or hydration fails, automatically prune and delete the newly created Git worktree and branch.

## Verification
- Add integration test creating a worktree with an invalid manifest / hydration failure, asserting that the Git branch and directory are cleaned up.
- Add test verifying signal cleanup for scratch sandboxes.
