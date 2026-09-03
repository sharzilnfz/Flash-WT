# Remove a worktree

`flashwt clean` (and its lower-level primitive `flashwt remove`) tears down worktrees,
releases store references, and reclaims unreferenced store storage.

## Sub-features

- `clean-unified` removes worktrees and performs store garbage collection in one invocation.
- `clean-safety` verifies that the branch is merged and the working directory is clean before removal.
- `remove-worktree` deletes the worktree directory and unregisters it from git.
- `remove-refs` releases store references (refcounts decrement for each hydrated blob).
- `remove-mirror` deletes the store-local worktree mirror.

## How to get to it (user POV)

- Run `flashwt clean <name>` (modern verb) to tear down a worktree and run garbage collection (use `--force` / `-f` if unmerged or dirty).
- Run `flashwt clean --all` to prune all clean, merged worktrees non-interactively.
- Run `flashwt remove <name>` (classic verb) to remove a worktree without triggering store sweep.
- Pass `--dir <path>` when the worktree lives outside the default sibling path.

## Driving it with the shell fixture

Preconditions:

- Fixture loaded, cwd `$FLASHWT_ORIGIN`.
- A worktree exists: `flashwt --json create demo --dir "$FLASHWT_FIXTURE/demo"` returned
  `ok` with `files_hydrated` > 0.

- **Remove via clean.** `flashwt --json clean demo --dir "$FLASHWT_FIXTURE/demo" --force --age 0s`.
  Envelope `status` is `ok`; `command` is `clean`; `data.mirrors_removed` is `1`;
  `data.references_released` is `40`; `data.sweep_examined` is present;
  `data.sweep_reclaimed` is present.
- **Remove via classic primitive.** `flashwt --json remove demo --dir "$FLASHWT_FIXTURE/demo"`.
  Envelope `status` is `ok`; `command` is `remove`; `data.mirror_removed` is `true`;
  `data.references_released` equals the file count from create.
- **Verify the directory is gone.** `test ! -e "$FLASHWT_FIXTURE/demo" && echo gone`.
- **Verify git forgot it.** `git -C "$FLASHWT_ORIGIN" worktree list` no longer
  lists the demo path.
- **Verify refs released.** In legacy mode, the refcount files under `$FLASHWT_STORE/refs/`
  no longer count the removed worktree.
- **Proof.** Save the envelope, the `worktree list` output, and the store
  listing to `artifacts/verify-flashwt/<run-id>/`.

## Gotchas

- Deleting the worktree directory by hand first makes remove fail with
  `<path> is not a worktree` and strands the store mirror. Always let flashwt do it.
- `references_released` counts the number of references successfully decremented. In shared stores, decrements succeed (e.g. refcount drops from 2 to 1), so `references_released` matches the hydrated blob count. It reports 0 only if no files were hydrated or in `mark-sweep-no-refs` mode.
- `flashwt remove` leaves unreferenced blobs in the store until a later sweep.
  `flashwt clean` runs sweep immediately during removal.
- Removing a `scratch`/`isolate` worktree leaves its lease file behind until `flashwt sweep` reclaims it.
