# Sweep the store

`flashwt sweep` deletes store entries no live worktree references and older than
`--age`, reclaiming disk. It also reclaims expired, dead, and orphaned scratch leases.

## Sub-features

- `sweep-unreferenced` collects unreferenced blobs past the age floor.
- `sweep-protected` keeps entries any live worktree references.
- `sweep-leases` reclaims expired, dead-process, or orphaned scratch leases.
- `sweep-dry-run` previews reclaimable blobs and leases without modifying disk.
- `sweep-mark-sweep` collects unreferenced blobs and snapshots using mirror marks.

## How to get to it (user POV)

- Run `flashwt sweep` from any repo pointed at the store.
- Tune retention with `flashwt sweep --age 0s|90s|10m|1h|7d`.
- Preview reclamation without deleting objects via `flashwt sweep --dry-run`.
- An explicit `--age` overrides default floors (7d in legacy mode, 15m in mark-sweep mode).

## Driving it with the shell fixture

Preconditions:

- Fixture loaded, cwd `$FLASHWT_ORIGIN`.
- Known store contents: create one worktree (`demo`) with an exclusive unique file, create a second (`demo2`), and remove `demo`. The exclusive blobs from `demo` are now unreferenced.

```sh
flashwt --json create demo  --dir "$FLASHWT_FIXTURE/demo"
echo "exclusive-payload" > "$FLASHWT_ORIGIN/heavy/pkg00/nested/exclusive.txt"
flashwt --json hydrate "$FLASHWT_FIXTURE/demo"
flashwt --json create demo2 --dir "$FLASHWT_FIXTURE/demo2"
flashwt --json remove demo  --dir "$FLASHWT_FIXTURE/demo"
```

- **Dry-run preview.** `flashwt --json sweep --age 0s --dry-run`. Envelope `status` is
  `ok`; `data.dry_run` is `true`; `data.unreferenced_blobs` is greater than 0;
  `data.reclaimed` is `0`.
- **Live sweep with age floor.** `flashwt --json sweep --age 0s`. Envelope `status` is
  `ok`; `data.mode` is present; `data.examined` counts store entries;
  `data.reclaimed` reports entries deleted; `data.leases_examined` counts evaluated leases;
  `data.leases_reclaimed` reports reclaimed scratch leases.
- **Mark-sweep mode.** In mark-sweep mode (`flashwt --json store migrate --activate-mark-sweep`),
  the envelope also reports `data.mirrors_removed`, `data.snapshot_dirs_removed`,
  `data.snapshot_cap_evicted`, and `data.deferred_by_grace`.
- **Verify protection.** `demo2` still hydrates fine after sweep:
  `cat "$FLASHWT_FIXTURE/demo2/heavy/pkg00/nested/file-0.txt"` matches fixture
  content. Shared blobs referenced by the live worktree are never touched.
- **Verify reclamation.** Exclusive blobs unreferenced after the remove are gone from
  `$FLASHWT_STORE/objects` (compare before/after `find` listings).
- **Proof.** Save dry-run and live envelopes plus before/after store listings to
  `artifacts/verify-flashwt/<run-id>/`.

## Gotchas

- Without `--age 0s` the default floor (7d in legacy, 15m in mark-sweep) means a
  just-removed worktree blobs will not be reclaimed. An assertion of
  `reclaimed > 0` without the flag is a flaky test, not a proof.
- `reclaimed: 0` is correct when the removed worktree shared all content with
  a live one. Seed unique content per worktree to force real reclamation.
- Sweep acts on the store named by `FLASHWT_STORE`. Running it without the
  fixture env sweeps the developer machine store. Never do this.
