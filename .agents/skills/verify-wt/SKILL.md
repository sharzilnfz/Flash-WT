---
name: verify-wt
description: >
  Launch, drive, and prove the behavior of `wt`, the instant-git-worktrees CLI
  in this repo (Rust, binary surface only, no server). Use whenever a change to
  wt needs end-to-end proof: creating/removing worktrees, hydration from the
  content-addressed store, sweep/scrub GC, or scratch/isolate sandboxes.
  Drives the real binary against throwaway fixture repos with an isolated
  store, asserts on --json envelopes plus on-disk state.
---

# Verify wt

`wt` is a CLI, not a service. There is no server to keep alive: **launch**
means build the binary once, then drive each proof inside a throwaway fixture
repo with its own private store.

## Launch

Build (or reuse) the release binary:

```sh
cargo build --release -p wt-cli   # skip if target/release/wt is fresh
WT=target/release/wt
$WT --version                     # expect: wt 0.1.0 (or match Cargo.toml)
```

Ready signal: `--version` prints and exits 0. No ports, no auth, no seed data.

For each proof, build a fixture (see Helpers) and load its exports:

```sh
eval "$(helpers/mkfixture.sh "$WT")"   # sets WT_BIN, WT_FIXTURE, WT_ORIGIN, WT_STORE; defines wt()
cd "$WT_ORIGIN"
```

The fixture is a real git repo with 40 untracked files under `heavy/` and a
`.wtinclude` manifest naming `heavy/`, the exact shape hydration moves. The
`wt()` shell function pins every invocation to the fixture's `WT_STORE`.

**Isolation is mandatory.** `WT_STORE` defaults to `~/.cache/wt/store`, the
developer's machine-wide store. Never run a single `wt` command without
`WT_STORE` pointing at the fixture store: even a `create` that hydrates
nothing writes a worktree mirror there and registers a git worktree on the
hosting repo. Two runs against one store also contend on its lockfiles.

## Doctor

Run whenever anything looks off, before driving:

```sh
"$WT_BIN" --version            # prints wt x.y.z; empty or error = wrong binary
git --version                  # wt shells out to git for every worktree op
[ -w "$(dirname "$WT_STORE")" ] && echo "store parent writable"
git -C "$WT_ORIGIN" rev-parse --is-inside-work-tree   # expect: true
"$WT_BIN" --help | grep -qE '^  (new|create)' && echo "CLI surface intact"
```

If `--version` prints but a command fails with `git ... fatal`, the fixture
repo is broken. Rebuild it with `mkfixture.sh` rather than debugging it.

## Drive

Every command emits a single-line JSON envelope on stdout:

```json
{"wt_version":"0.1.0","schema_version":1,"command":"create","status":"ok","data":{...},"diagnostics":[]}
```

Assert on `status` (never trust exit code alone), on `data` fields, and on
disk state. `diagnostics` carries warning-level codes such as
`CROSS_DEVICE_COPY_DEGRADATION`. Tmpfs forces byte copies. The `hydration_method`
is filesystem-dependent (`byte_copy` on tmpfs, clonefile-based CoW on APFS).

The five user-facing features each have a full recipe in
[features/](./features/README.md). Read the map first; a proof that drives
only `create` is incomplete when the map lists `remove`, `sweep`, `scrub`,
and `scratch`/`isolate`. Quick forms:

```sh
wt --json new demo --dir "$WT_FIXTURE/demo"      # modern create; data.files_hydrated > 0
wt --json clean demo --dir "$WT_FIXTURE/demo"    # modern remove with GC sweep; data.references_released > 0
wt --json create demo --dir "$WT_FIXTURE/demo"   # classic create; data.files_hydrated > 0
wt --json remove demo --dir "$WT_FIXTURE/demo"   # classic remove; data.references_released > 0
wt --json sweep --age 0s                         # GC sweep; --age 0s defeats the default grace period
wt --json scrub --dry-run                        # audit store for corrupted blobs
wt --json scratch --run 'ls heavy/pkg00'         # run command in sandbox and clean up
```

## Evidence

Capture to `artifacts/verify-wt/<run-id>/` at the repo root (create it; it
survives fixture teardown). Per feature proven, save:

- the literal command, its stdout envelope, stderr, and exit code (one
  `.txt` per step, e.g. `create.stdout.txt`);
- on-disk state alongside the envelope: `ls -R` of the hydrated worktree,
  `cat` of a hydrated file, `git -C "$WT_FIXTURE/demo" branch --show-current`,
  and a `find "$WT_STORE" -maxdepth 2` listing;
- side effects, not just final screens: for `remove`, show the worktree dir
  is gone and the store mirror was removed; for `sweep`, show `reclaimed`
  in `data`; for `scrub`, show `corrupt`/`deleted` counts.

Proof standards: exercise the real user path (the plain CLI), not test
endpoints or the Rust fixture harness in `crates/wt-cli/tests`. Verify
side effects in the store and worktree alongside what stdout claims. Never
point a proof at `~/.cache/wt/store`. An envelope from the machine store
proves nothing about isolation and pollutes developer state.

## Cleanup

No daemons exist, so there is nothing to kill. Tear down state in this
order, from the fixture origin:

```sh
cd "$WT_ORIGIN"
wt remove <name> --dir "$WT_FIXTURE/<name>"   # for each live worktree; scratch --run needs none
wt sweep --age 0s                             # collects entries and expired scratch leases
rm -rf "$WT_FIXTURE"                          # last: the fixture dir holds the store too
```

Never `rm -rf` the fixture before `remove`. Deleting a worktree directory out
from under wt makes `wt remove` fail with `is not a working tree` and strands
the store mirror. Kill nothing by process name; wt leaves no processes
behind. Evidence in `artifacts/verify-wt/<run-id>/` is never removed by
cleanup. After teardown, confirm it still exists.

## Helpers

`helpers/mkfixture.sh` (executable) builds the isolated fixture and prints
shell exports to `eval`. It is shown working in Launch above. Re-invoke it
whenever a fixture is broken. Fixtures are disposable, never repaired.

Feature map: [features/README.md](./features/README.md).
