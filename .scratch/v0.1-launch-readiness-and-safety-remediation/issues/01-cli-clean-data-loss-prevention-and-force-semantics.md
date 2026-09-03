Status: ready-for-agent

# Issue 01: CLI Clean Data Loss Prevention and Force Semantics

## Problem
`flashwt clean <name>` and `flashwt clean --all` currently use forced removal semantics (`git worktree remove --force` and `git branch -D`) without checking for uncommitted files or unmerged branches. Removal errors are swallowed, and false success receipts are emitted to stdout/JSON. Additionally, store GC mirrors are deleted before verifying filesystem removal.

## Requirements
1. Inspect worktree dirty state using porcelain Git status before removal.
2. For targeted single-worktree removal (`clean <name>`), require `--force` if dirty files or unmerged commits exist.
3. For batch cleanup (`clean` / `clean --all`), restrict candidate selection to merged worktrees by default. Unmerged worktrees must require explicit `--force`.
4. Fail cleanly with non-zero exit codes if Git or filesystem removal fails, and report truthful JSON/human receipts.
5. Retire the store GC mirror only after Git and directory removal have completed successfully.

## Verification
- Add integration tests verifying that `clean` refuses to delete dirty or unmerged worktrees without `--force`.
- Add integration tests verifying that failed deletions emit error diagnostics and do not claim successful removal in JSON envelopes.
