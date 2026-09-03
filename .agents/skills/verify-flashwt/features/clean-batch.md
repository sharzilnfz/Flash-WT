# Batch clean worktrees

`flashwt clean --all` automatically discovers, verifies, and removes all eligible stale or merged git worktrees and runs store garbage collection in a single atomic invocation.

## Sub-features

- `clean-all-merged` finds and deletes all worktrees whose branches are fully merged into HEAD and clean.
- `clean-force-unmerged` removes unmerged or dirty worktrees when combined with `--force` (`-f`).
- `clean-protect-main` preserves the repository's main worktree from deletion under all flags.
- `clean-unified-gc` runs store GC sweep immediately following batch worktree removal.
- `clean-age-override` passes `--age <dur>` to control store object sweep retention during cleanup.

## How to get to it (user POV)

- Run `flashwt clean --all` to non-interactively remove all clean, merged worktrees and sweep the store.
- Run `flashwt clean --all --force` to delete all linked worktrees regardless of merge status or uncommitted changes.
- Run `flashwt clean --all --age 0s` to instantly reclaim store objects with no GC grace period.

## Driving it with the shell fixture

Preconditions:

- Fixture loaded, cwd `$FLASHWT_ORIGIN`.
- Create two worktrees:
  `flashwt --json new merged-worktree --dir "$FLASHWT_FIXTURE/merged-worktree"`
  `flashwt --json new unmerged-worktree --dir "$FLASHWT_FIXTURE/unmerged-worktree"`
- In `$FLASHWT_FIXTURE/unmerged-worktree`, commit an unmerged change:
  `echo "unmerged work" >> "$FLASHWT_FIXTURE/unmerged-worktree/src.txt"`
  `git -C "$FLASHWT_FIXTURE/unmerged-worktree" commit -am "unmerged commit"`

- **Batch clean merged worktrees.** `flashwt --json clean --all --age 0s`. Envelope `status` is `ok`; `command` is `clean`; `data.removed_worktrees` contains `$FLASHWT_FIXTURE/merged-worktree`; `data.branches_removed` contains `merged-worktree`; `data.removed_worktrees` does NOT contain `unmerged-worktree`; `data.sweep_examined` and `data.sweep_reclaimed` report GC counts.
- **Verify disk state.** `test ! -e "$FLASHWT_FIXTURE/merged-worktree"` succeeds; `test -d "$FLASHWT_FIXTURE/unmerged-worktree"` confirms the unmerged worktree was spared.
- **Batch clean with force.** `flashwt --json clean --all --force --age 0s`. Envelope reports `unmerged-worktree` in `data.branches_removed`.
- **Verify git worktree list.** `git -C "$FLASHWT_ORIGIN" worktree list` only shows the main worktree.
- **Verify main tree protection.** The origin repo directory `$FLASHWT_ORIGIN` is never included in `data.removed_worktrees`.
- **Proof.** Save the clean envelope and `git worktree list` output to `artifacts/verify-flashwt/<run-id>/`.

## Gotchas

- `flashwt clean --all` without `--force` silently skips dirty worktrees (uncommitted edits) and branches with unmerged commits.
- In an interactive terminal without `--all` or a branch name, `flashwt clean` enters an interactive selector prompting for worktree numbers. Always supply `--all` in automation and verification scripts.
- `--age 0s` is essential in verification to bypass the store's default GC grace period and confirm storage reclamation.
