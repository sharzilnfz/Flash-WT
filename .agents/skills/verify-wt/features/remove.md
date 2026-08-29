# Remove a worktree

`wt remove` tears down a worktree created by `wt create` and releases the
store references its hydration claimed, so sweep can eventually reclaim the
space.

## Sub-features

- `remove-worktree` deletes the worktree directory and unregisters it from git.
- `remove-refs` releases store references (refcount drops to zero for exclusive content).
- `remove-mirror` deletes the store-local worktree mirror.

## How to get to it (user POV)

- Run `wt remove <name>` from the original repo (worktree at the default sibling location).
- Or `wt remove <name> --dir <path>` when the worktree lives elsewhere.

## Driving it with the shell fixture

Preconditions:

- Fixture loaded, cwd `$WT_ORIGIN`.
- A worktree exists: `wt --json create demo --dir "$WT_FIXTURE/demo"` returned
  `ok` with `files_hydrated` > 0.

- **Remove.** `wt --json remove demo --dir "$WT_FIXTURE/demo"`. Envelope
  `status` is `ok`; `data.references_released` equals the file count from
  create; `data.mirror_removed` is `true`.
- **Verify the directory is gone.** `test ! -e "$WT_FIXTURE/demo" && echo gone`.
- **Verify git forgot it.** `git -C "$WT_ORIGIN" worktree list` no longer
  lists the demo path.
- **Verify refs released.** The refcount files under `$WT_STORE/refs/` for the
  hydrated blobs no longer count demo (compare the `find "$WT_STORE/refs"`
  listing captured at create time).
- **Proof.** Save the envelope, the `worktree list` output, and the store
  listing to `artifacts/verify-wt/<run-id>/`.

## Gotchas

- Deleting the worktree directory by hand first makes remove fail with
  `is not a working tree` and strands the store mirror. Always let wt do it.
- `references_released: 0` can be correct when another worktree shares the
  same content — a second create from the same fixture releases its half only.
- Removing a `scratch`/`isolate` worktree also requires its lease to expire or
  sweep to reclaim it; see scratch-isolate.md.
