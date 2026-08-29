# Issue 03: Workspace Lifecycle Consolidation

Status: ready-for-agent

## Context
`gitops.rs` is currently a shallow wrapper over raw git subprocess execution. Worktree discovery, porcelain output parsing, active worktree detection, gitdir path resolution, and merge-ancestor validation are fragmented across `commands/list.rs` and `commands/clean.rs`.

## Requirements
- Deepen `gitops` into a cohesive `workspace` module in `wt-cli`.
- Move porcelain parsing, worktree metadata mapping, and merge-ancestor checks behind the workspace interface.
- Provide a typed `WorkspaceEngine` for discovering, creating, validating, and deleting git worktrees.
- Refactor `clean.rs`, `list.rs`, `create.rs`, and `gc.rs` to consume the deep workspace module.
- Ensure all worktree listing and cleanup integration tests pass.

## Files Owned
- `crates/wt-cli/src/gitops.rs`
- `crates/wt-cli/src/commands/clean.rs`
- `crates/wt-cli/src/commands/list.rs`
- `crates/wt-cli/src/commands/create.rs`
- `crates/wt-cli/src/gc.rs`
- `crates/wt-cli/tests/list.rs`
- `crates/wt-cli/tests/clean.rs`
