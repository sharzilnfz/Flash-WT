# flashwt verification map

This directory is the maintained source for verifying the user-facing behavior
of `flashwt`. Read the index before driving, then use the matching feature file as
the recipe.

## Baseline preconditions

- Binary at `$FLASHWT_BIN` (release build, `flashwt --version` answers).
- Fixture loaded with `eval "$(helpers/mkfixture.sh "$Flash-WT")"`.
  This sets `FLASHWT_BIN`, `FLASHWT_FIXTURE`, `FLASHWT_ORIGIN`, and `FLASHWT_STORE`, defines `flashwt()`, and yields cd control.
- `FLASHWT_STORE` exported and pointing at the fixture store on every invocation.
- Working directory for all `flashwt` calls is `$FLASHWT_ORIGIN` (or the worktree).
- Never run any `flashwt` command without the fixture `FLASHWT_STORE` set.

## Driving conventions

- Every command goes through `flashwt --json ...`; assert `status`, `data` fields,
  and on-disk state.
- Treat commands as literal. Keep quoted names and flags unchanged.
- One fixture per run. Recreate with `mkfixture.sh` when dirty; never repair.
- Sweep with `--age 0s` in proofs; the default grace is 15m and would make
  assertions time-dependent.
- `FLASHWT_SNAPSHOTS` / `FLASHWT_SNAPSHOTS_V2` are macOS/APFS-only opt-ins. On Linux,
  do not write proofs that depend on snapshot hits.

## Proof and skip reporting

- Capture the command, stdout envelope, stderr, and exit code for every step
  into `artifacts/verify-flashwt/<run-id>/`.
- Pair every envelope with the resulting disk state (worktree listing,
  hydrated file contents, store listing).
- An envelope alone is not proof. When `create` claims hydration, show the files.
  When `remove` claims release, show the directory gone.
- Report an unreachable path with the attempted command and the unmet
  precondition. Do not report a skipped entry point as verified through a
  different path.

## Feature entry contract

Each feature file starts with an H1 title and one paragraph describing the
user-visible behavior. It then uses exactly four H2 sections in this order.

1. `Sub-features` lists short IDs with one line for each behavior.
2. `How to get to it (user POV)` lists every user entry point.
3. `Driving it with the shell fixture` starts with `Preconditions:` and uses
   labeled bullets that pair each user action with an exact command and
   observable result.
4. `Gotchas` lists traps that can waste or invalidate a verification run.

Keep implementation details out of the map. Name only user paths, stable
handles, required state, commands, and observable proof.

## Features

- [Create a worktree](./create.md) covers worktree creation, `flashwt new`, and heavy-directory hydration.
- [Remove a worktree](./remove.md) covers teardown, `flashwt clean`, and store reference release.
- [Sweep the store](./sweep.md) covers garbage collection of unreferenced entries and leases.
- [Scrub the store](./scrub.md) covers corruption detection, dry-run reporting, and repair.
- [Scratch / isolate](./scratch-isolate.md) covers ephemeral leased sandboxes with optional command execution.
- [In-place hydration](./hydrate.md) covers in-place hydration into existing worktrees and directories.
- [List active worktrees](./list.md) covers worktree discovery, JSON data, disk usage, and shared deduplication savings.
- [Initialize starter manifest](./init.md) covers starter `.flashwtinclude` generation and overwrite protection.
- [Batch clean worktrees](./clean-batch.md) covers batch removal of stale or merged worktrees and immediate GC sweep.
- [Zero-setup demo & benchmark](./demo.md) covers the synthetic 10,000-file benchmark, CoW vs copy comparison, and mutation isolation.
- [Migrate store GC](./store-migrate.md) covers GC cutover to mark-sweep and irreversible dropping of legacy refs.
- [Shell completions](./completions.md) covers tab-completion generation for bash, zsh, fish, elvish, and powershell.
- [Flash snapshots & incremental rebuilds](./flash-snapshots.md) covers APFS whole-directory snapshot caching (v1) and diff-based incremental rebuilds (v2).
- [Storage deduplication & CoW isolation](./storage-dedup.md) covers APFS block sharing, volume storage accounting, and break-on-write mutation isolation.
