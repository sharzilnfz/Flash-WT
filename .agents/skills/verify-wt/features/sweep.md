# Sweep the store

`wt sweep` deletes store entries no live worktree references and older than
`--age`, reclaiming disk. It also reclaims expired scratch leases.

## Sub-features

- `sweep-unreferenced` collects unreferenced blobs past the age floor.
- `sweep-protected` keeps entries any live worktree references.
- `sweep-leases` reclaims expired scratch leases.

## How to get to it (user POV)

- Run `wt sweep` from any repo pointed at the store (the store is global).
- Tune retention with `wt sweep --age 0s|90s|10m|1h|7d`; the effective floor
  in mark-sweep mode is `WT_GC_GRACE` (default 15m).

## Driving it with the shell fixture

Preconditions:

- Fixture loaded, cwd `$WT_ORIGIN`.
- Known store contents: create one worktree (`demo`), remove it, and keep a
  second (`demo2`) alive — the removed one's exclusive blobs are now
  unreferenced.

```sh
wt --json create demo  --dir "$WT_FIXTURE/demo"
wt --json create demo2 --dir "$WT_FIXTURE/demo2"
wt --json remove demo  --dir "$WT_FIXTURE/demo"
```

- **Sweep with age floor.** `wt --json sweep --age 0s`. Envelope `status` is
  `ok`; `data.mode` is `legacy`; `data.examined` counts store entries;
  `data.reclaimed` reports entries deleted; `data.leases_reclaimed` reports
  expired scratch leases.
- **Verify protection.** `demo2` still hydrates fine after sweep:
  `cat "$WT_FIXTURE/demo2/heavy/pkg00/nested/file-0.txt"` matches fixture
  content. Shared blobs referenced by the live worktree are never touched.
- **Verify reclamation.** Blobs unreferenced after the remove are gone from
  `$WT_STORE/objects` (compare before/after `find` listings).
- **Proof.** Save the envelope plus before/after store listings to
  `artifacts/verify-wt/<run-id>/`.

## Gotchas

- Without `--age 0s` the default floor (7d legacy, 15m mark-sweep) means a
  just-removed worktree's blobs will NOT be reclaimed — an assertion of
  `reclaimed > 0` without the flag is a flaky test, not a proof.
- `reclaimed: 0` is correct when the removed worktree shared all content with
  a live one. Seed unique content per worktree to force real reclamation.
- Sweep acts on the store named by `WT_STORE` — running it without the
  fixture env sweeps the developer's machine store. Never do this.
