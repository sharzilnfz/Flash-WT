---
name: verify-wt
description: >
  Launch, drive, and prove the behavior of `wt` / `flashwt`, the instant-git-worktrees CLI
  in this repo (Rust, binary surface only, no server). Use whenever a change to
  wt needs end-to-end proof: creating/removing worktrees, hydration from the
  content-addressed store, sweep/scrub GC, scratch/isolate sandboxes, or store migration.
  Drives the real binary against throwaway fixture repos with an isolated
  store, asserts on --json envelopes plus on-disk state.
---

# Verify wt

`wt` (packaged as `flashwt`) is a CLI, not a service. There is no server to keep alive:
**launch** means build the binary once, then drive each proof inside a throwaway fixture
repo with its own private store.

## Launch

Build (or reuse) the release binary:

```sh
cargo build --release -p wt-cli
# Cargo produces target/release/flashwt; a symlink or alias target/release/wt is also available:
WT=target/release/flashwt   # or WT=target/release/wt
$WT --version               # expect: flashwt 0.1.0 or wt 0.1.0 (matches Cargo.toml)
```

Both `flashwt` and `wt` point to the identical CLI implementation.
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
"$WT_BIN" --version            # prints flashwt/wt x.y.z; empty or error = wrong binary
git --version                  # wt shells out to git for every worktree op
[ -w "$(dirname "$WT_STORE")" ] && echo "store parent writable"
git -C "$WT_ORIGIN" rev-parse --is-inside-work-tree   # expect: true

# APFS clonefile readiness:
if [ "$(uname -s)" = "Darwin" ]; then
  stat -f "%f %T" "$WT_STORE" 2>/dev/null | grep -qi "apfs" \
    && echo "APFS clonefile ready" \
    || echo "Non-APFS filesystem (falls back to byte copies)"
fi

# Store GC migration state:
if [ -f "$WT_STORE/gc-mode" ]; then
  echo "store gc-mode: $(cat "$WT_STORE/gc-mode")"
else
  echo "store gc-mode: legacy (refcount)"
fi

# CLI surface check across subcommands:
"$WT_BIN" --help | grep -qE 'init' && \
"$WT_BIN" --help | grep -qE 'hydrate' && \
"$WT_BIN" --help | grep -qE 'new|create' && \
"$WT_BIN" --help | grep -qE 'clean|remove' && \
"$WT_BIN" --help | grep -qE 'list|ls' && \
"$WT_BIN" --help | grep -qE 'sweep' && \
"$WT_BIN" --help | grep -qE 'scrub' && \
"$WT_BIN" --help | grep -qE 'scratch|isolate' && \
"$WT_BIN" --help | grep -qE 'store' && \
"$WT_BIN" --help | grep -qE 'demo|test-drive' && \
"$WT_BIN" --help | grep -qE 'completions' && echo "CLI surface intact (all 11 subcommands verified)"
```

If `--version` prints but a command fails with `git ... fatal`, the fixture
repo is broken. Rebuild it with `mkfixture.sh` rather than debugging it.

## Subcommands

Flash WT exposes 11 distinct command surfaces (with modern primary verbs and legacy aliases):

1. **`init`**: Initialize a starter `.wtinclude` manifest in the repository root or target directory.
   ```sh
   wt init [--dir <path>] [--force]
   ```
2. **`new` / `create`**: Provision a new git worktree on a new branch with heavy directories hydrated.
   ```sh
   wt new <name> [--dir <path>] [--base <ref>] [--manifest <path>]
   wt create <name> [--dir <path>] [--base <ref>] [--manifest <path>]
   ```
3. **`hydrate`**: In-place hydration of heavy directories into an existing directory or worktree.
   ```sh
   wt hydrate <path> [--source <repo>] [--manifest <path>]
   ```
4. **`list` / `ls`**: Discover active git worktrees, disk usage, and shared deduplication savings.
   ```sh
   wt list
   wt ls
   ```
5. **`clean` / `remove`**: Tear down worktrees, release store references, and optionally perform GC sweep.
   ```sh
   wt clean [name] [--dir <path>] [--all] [--force] [--age <dur>]
   wt remove <name> [--dir <path>]
   ```
6. **`sweep`**: Delete unreferenced store entries and expired scratch leases older than `--age`.
   ```sh
   wt sweep [--age 0s|90s|10m|1h|7d]
   ```
7. **`scrub`**: Audit store blobs against content addresses and repair or purge corruption.
   ```sh
   wt scrub [--dry-run]
   ```
8. **`scratch` / `isolate`**: Create ephemeral leased sandboxes with optional execution and auto-cleanup.
   ```sh
   wt scratch [name] [--dir <path>] [--run '<command>'] [--ttl <dur>]
   wt isolate [name] [--dir <path>] [--run '<command>'] [--ttl <dur>]
   ```
9. **`store migrate`**: Inspect and migrate the store GC scheme between refcounts and mark-sweep (ADR-0004).
   ```sh
   wt store migrate --activate-mark-sweep
   wt store migrate --drop-legacy-refs
   ```
10. **`demo` / `test-drive`**: Run zero-setup 10,000-file benchmark, CoW verification, and isolation tests.
    ```sh
    wt demo
    wt test-drive
    ```
11. **`completions`**: Generate native shell tab-completion scripts.
    ```sh
    wt completions <bash|zsh|fish|elvish|powershell>
    ```

## Environment Variables

Flash WT honors eight environment variables controlling storage paths, acceleration, and verification:

- **`WT_STORE`**: Path to the content-addressed store root (default: `~/.cache/wt/store`). Pinned to fixture store in verification.
- **`WT_SNAPSHOTS`**: Whole-directory snapshot caching fast path. Enabled by default on macOS APFS; set `WT_SNAPSHOTS=0` to disable and force per-file hydration.
- **`WT_SNAPSHOTS_V2`**: Incremental diff-based snapshot rebuilds. Enabled by default on macOS APFS; set `WT_SNAPSHOTS_V2=0` to disable.
- **`WT_VERIFY`**: Force full SHA-256 re-hash of every blob on every run, bypassing snapshot hits and the verified-blob ledger (`WT_VERIFY=1`).
- **`WT_HARDLINK`**: Opt into experimental hardlinked materialization with read-only inodes (`WT_HARDLINK=1`).
- **`WT_NO_HARDLINK`**: Force plain byte copies instead of clones or hardlinks (`WT_NO_HARDLINK=1`). Takes precedence over `WT_HARDLINK`.
- **`WT_GC_GRACE`**: Retention grace period protecting young store objects and mirrors in mark-sweep mode (default: `15m`, e.g. `WT_GC_GRACE=1h`).
- **`WT_TIMING`**: Print per-stage duration metrics (`wt-stage ingest=...`) to stderr (`WT_TIMING=1`).

Boolean variables follow tri-state semantics: `0`, `false`, `no`, `off` disable; unset uses the default; any other value enables.

## Drive

Every command emits a single-line JSON envelope on stdout when `--json` is supplied:

```json
{"wt_version":"0.1.0","schema_version":1,"command":"create","status":"ok","data":{...},"diagnostics":[]}
```

Assert on `status` (never trust exit code alone), on `data` fields, and on
disk state. `diagnostics` carries warning-level codes such as
`CROSS_DEVICE_COPY_DEGRADATION`. Tmpfs forces byte copies. The `hydration_method`
is filesystem-dependent (`byte_copy` on tmpfs, clonefile-based CoW on APFS).

The user-facing features each have a full recipe in
[features/](./features/README.md). Read the map first before driving.
For extensive automated test runs across all subcommands and edge cases, execute
the comprehensive test runner:

```sh
scripts/verify/run_all.sh
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

