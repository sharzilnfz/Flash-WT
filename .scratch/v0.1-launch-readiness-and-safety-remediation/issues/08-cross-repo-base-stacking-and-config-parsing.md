Status: ready-for-agent

# Issue 08: Scoped Branch Stacking Diagnostics & Boolean Config Parsing

## Problem
1. `base.rs` reads store mirrors across all repositories on the machine and matches basenames, triggering false-positive `BASE_BRANCH_MOVED` diagnostics when other unrelated repositories have branches with the same name.
2. `config.rs` evaluates environment variable strings by checking `value != "0"`, which treats `"false"`, `"no"`, and empty strings as `true`.

## Requirements
1. Scope store mirror scans in `base.rs` to only match mirrors belonging to the active repository root.
2. Update boolean environment variable parser in `config.rs` to recognize `"0"`, `"false"`, `"no"`, and `"off"` (case-insensitive) as `false`, and treat empty strings as omitted/default.

## Verification
- Add test verifying that `FLASHWT_SNAPSHOTS=false` correctly disables snapshot projection.
- Add test asserting that branch stacking checks ignore worktree mirrors from other repositories.
