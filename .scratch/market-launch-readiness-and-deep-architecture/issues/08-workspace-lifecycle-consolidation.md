# 08: Deep Workspace Lifecycle Module

Status: ready-for-agent

Blocked by: `06-cli-dead-code-clap-aliases-and-dep-pruning.md`

## Problem

`gitops.rs` is currently a shallow wrapper over raw git subprocess execution. Worktree discovery, porcelain output parsing (`RawGitWorktree`), active worktree detection, gitdir path resolution, and merge-ancestor validation are fragmented across `commands/list.rs` and `commands/clean.rs`.

## Work

1. Deepen `gitops` into a cohesive `workspace` module in `flashwt-cli`.
2. Move porcelain parsing, worktree metadata mapping, and merge-ancestor checks behind the workspace interface.
3. Provide a typed `WorkspaceEngine` for discovering, creating, validating, and deleting git worktrees.
4. Refactor `clean.rs`, `list.rs`, `create.rs`, and `gc.rs` to consume the deep workspace module.
5. Ensure all worktree listing, creation, and cleanup integration tests pass.

## Files Owned

- `crates/flashwt-cli/src/workspace.rs` (or `crates/flashwt-cli/src/gitops.rs`)
- `crates/flashwt-cli/src/commands/clean.rs`
- `crates/flashwt-cli/src/commands/list.rs`
- `crates/flashwt-cli/src/commands/create.rs`
- `crates/flashwt-cli/src/gc.rs`
- `crates/flashwt-cli/tests/list.rs`
- `crates/flashwt-cli/tests/clean.rs`

## Done When

- [ ] Git porcelain parsing and worktree inspection are encapsulated in the workspace module.
- [ ] Merge-base ancestor inspection is centralized behind the workspace interface.
- [ ] Command handlers interact with git lifecycle operations exclusively through the typed workspace API.
- [ ] Worktree listing and cleanup integration tests pass with zero regressions.
