# Batch clean worktrees

`wt clean --all` automatically discovers, verifies, and removes all eligible stale or merged git worktrees and runs store garbage collection in a single atomic invocation.

## Sub-features

- `clean-all-merged` finds and deletes all worktrees whose branches are fully merged into HEAD and clean.
- `clean-force-unmerged` removes unmerged or dirty worktrees when combined with `--force` (`-f`).
- `clean-protect-main` preserves the repository's main worktree from deletion under all flags.
- `clean-unified-gc` runs store GC sweep immediately following batch worktree removal.
- `clean-age-override` passes `--age <dur>` to control store object sweep retention during cleanup.

## How to get to it (user POV)

- Run `wt clean --all` to non-interactively remove all clean, merged worktrees and sweep the store.
- Run `wt clean --all --force` to delete all linked worktrees regardless of merge status or uncommitted changes.
- Run `wt clean --all --age 0s` to instantly reclaim store objects with no GC grace period.

## Driving it with the shell fixture

Preconditions:

- Fixture loaded, cwd `$WT_ORIGIN`.
- Create two worktrees:
  `wt --json new merged-wt --dir "$WT_FIXTURE/merged-wt"`
  `wt --json new unmerged-wt --dir "$WT_FIXTURE/unmerged-wt"`
- In `$WT_FIXTURE/unmerged-wt`, commit an unmerged change:
  `echo "unmerged work" >> "$WT_FIXTURE/unmerged-wt/src.txt"`
  `git -C "$WT_FIXTURE/unmerged-wt" commit -am "unmerged commit"`

- **Batch clean merged worktrees.** `wt --json clean --all --age 0s`. Envelope `status` is `ok`; `command` is `clean`; `data.removed_worktrees` contains `$WT_FIXTURE/merged-wt`; `data.branches_removed` contains `merged-wt`; `data.removed_worktrees` does NOT contain `unmerged-wt`; `data.sweep_examined` and `data.sweep_reclaimed` report GC counts.
- **Verify disk state.** `test ! -e "$WT_FIXTURE/merged-wt"` succeeds; `test -d "$WT_FIXTURE/unmerged-wt"` confirms the unmerged worktree was spared.
- **Batch clean with force.** `wt --json clean --all --force --age 0s`. Envelope reports `unmerged-wt` in `data.branches_removed`.
- **Verify git worktree list.** `git -C "$WT_ORIGIN" worktree list` only shows the main worktree.
- **Verify main tree protection.** The origin repo directory `$WT_ORIGIN` is never included in `data.removed_worktrees`.
- **Proof.** Save the clean envelope and `git worktree list` output to `artifacts/verify-wt/<run-id>/`.

## Gotchas

- `wt clean --all` without `--force` silently skips dirty worktrees (uncommitted edits) and branches with unmerged commits.
- In an interactive terminal without `--all` or a branch name, `wt clean` enters an interactive selector prompting for worktree numbers. Always supply `--all` in automation and verification scripts.
- `--age 0s` is essential in verification to bypass the store's default GC grace period and confirm storage reclamation.
