# Remove a worktree

`wt clean` (and its lower-level primitive `wt remove`) tears down worktrees,
releases store references, and reclaims unreferenced store storage.

## Sub-features

- `clean-unified` removes worktrees and performs store garbage collection in one invocation.
- `remove-worktree` deletes the worktree directory and unregisters it from git.
- `remove-refs` releases store references (refcount drops to zero for exclusive content).
- `remove-mirror` deletes the store-local worktree mirror.

## How to get to it (user POV)

- Run `wt clean <name>` (modern verb) to tear down a worktree and run garbage collection.
- Run `wt clean --all` to prune all stale or merged worktrees non-interactively.
- Run `wt remove <name>` (classic verb) to remove a worktree without triggering store sweep.
- Pass `--dir <path>` when the worktree lives outside the default sibling path.

## Driving it with the shell fixture

Preconditions:

- Fixture loaded, cwd `$WT_ORIGIN`.
- A worktree exists: `wt --json create demo --dir "$WT_FIXTURE/demo"` returned
  `ok` with `files_hydrated` > 0.

- **Remove via clean.** `wt --json clean demo --dir "$WT_FIXTURE/demo" --age 0s`.
  Envelope `status` is `ok`; `command` is `clean`; `data.mirrors_removed` is `1`;
  `data.references_released` is `40`; `data.sweep_examined` is present;
  `data.sweep_reclaimed` is present.
- **Remove via classic primitive.** `wt --json remove demo --dir "$WT_FIXTURE/demo"`.
  Envelope `status` is `ok`; `command` is `remove`; `data.mirror_removed` is `true`;
  `data.references_released` equals the file count from create.
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
  same content. A second create from the same fixture releases its half only.
- `wt remove` leaves unreferenced blobs in the store until a later sweep.
  `wt clean` runs sweep immediately during removal.
- Removing a `scratch`/`isolate` worktree also requires its lease to expire or
  sweep to reclaim it; see scratch-isolate.md.
