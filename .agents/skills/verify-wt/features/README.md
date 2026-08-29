# wt verification map

This directory is the maintained source for verifying the user-facing behavior
of `wt`. Read the index before driving, then use the matching feature file as
the recipe.

## Baseline preconditions

- Binary at `$WT_BIN` (release build, `wt --version` answers).
- Fixture loaded: `eval "$(helpers/mkfixture.sh "$WT")"` — sets `WT_BIN`,
  `WT_FIXTURE`, `WT_ORIGIN`, `WT_STORE`, defines `wt()`, and `cd`s are yours.
- `WT_STORE` exported and pointing at the fixture store on every invocation.
- Working directory for all `wt` calls is `$WT_ORIGIN` (or the worktree).
- Never run any `wt` command without the fixture `WT_STORE` set.

## Driving conventions

- Every command goes through `wt --json ...`; assert `status`, `data` fields,
  and on-disk state.
- Treat commands as literal. Keep quoted names and flags unchanged.
- One fixture per run. Recreate with `mkfixture.sh` when dirty; never repair.
- Sweep with `--age 0s` in proofs; the default grace is 15m and would make
  assertions time-dependent.
- `WT_SNAPSHOTS` / `WT_SNAPSHOTS_V2` are macOS/APFS-only opt-ins. On Linux,
  do not write proofs that depend on snapshot hits.

## Proof and skip reporting

- Capture the command, stdout envelope, stderr, and exit code for every step
  into `artifacts/verify-wt/<run-id>/`.
- Pair every envelope with the resulting disk state (worktree listing,
  hydrated file contents, store listing).
- An envelope alone is not proof. `create` claims hydration — show the files;
  `remove` claims release — show the directory gone.
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

- [Create a worktree](./create.md) covers branch+worktree creation and heavy-directory hydration.
- [Remove a worktree](./remove.md) covers teardown and store reference release.
- [Sweep the store](./sweep.md) covers garbage collection of unreferenced entries and leases.
- [Scrub the store](./scrub.md) covers corruption detection, dry-run reporting, and repair.
- [Scratch / isolate](./scratch-isolate.md) covers ephemeral leased sandboxes with optional command execution.
