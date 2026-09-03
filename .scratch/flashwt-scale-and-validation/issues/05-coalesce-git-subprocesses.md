# 05 - Coalesce git subprocesses

Status: ready-for-agent

## Problem

Warm create spawns rev parse plus worktree add plus gitdir lookup separately, each paying process startup.

## Solution

Cache rev parse per process through the WorkspaceEngine seam. Keep real git calls. No reimplementation of git semantics.

## Acceptance

- Warm create is faster with identical resulting worktree.
- Stage timing shows fewer git spawns.

## Verification

- `cargo test -p flashwt-cli --lib` plus `FLASHWT_TIMING=1` before and after.

## Comments

- Never reimplement worktree add. Divergence risk dwarfs the gain.
