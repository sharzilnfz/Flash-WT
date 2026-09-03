---
name: verify-flashwt
description: >
  Launch, drive, and prove the behavior of `flashwt` / `flashwt`, the instant-git-worktrees CLI
  in this repo (Rust, binary surface only, no server). Use whenever a change to
  flashwt needs end-to-end proof: creating/removing worktrees, hydration from the
  content-addressed store, sweep/scrub GC, scratch/isolate sandboxes, or store migration.
  Drives the real binary against throwaway fixture repos with an isolated
  store, asserts on --json envelopes plus on-disk state.
---

# Verify flashwt

`flashwt` (packaged as `flashwt`) is a CLI, not a service. There is no server to keep alive:
**launch** means build the binary once, then drive each proof inside a throwaway fixture
repo with its own private store.

## Launch

Build (or reuse) the release binary:

```sh
cargo build --release -p flashwt-cli
# Cargo produces target/release/flashwt; a symlink or alias target/release/flashwt is also available:
Flash-WT=target/release/flashwt   # or Flash-WT=target/release/flashwt
$Flash-WT --version               # expect: flashwt 0.1.0 or flashwt 0.1.0 (matches Cargo.toml)
```

Both `flashwt` and `flashwt` point to the identical CLI implementation.
Ready signal: `--version` prints and exits 0. No ports, no auth, no seed data.

For each proof, build a fixture (see Helpers) and load its exports:

```sh
eval "$(helpers/mkfixture.sh "$Flash-WT")"   # sets FLASHFLASHWT_BIN, FLASHFLASHWT_FIXTURE, FLASHFLASHWT_ORIGIN, FLASHFLASHWT_STORE; defines flashwt()
cd "$FLASHFLASHWT_ORIGIN"
```

The fixture is a real git repo with 40 untracked files under `heavy/` and a
`.flashwtinclude` manifest naming `heavy/`, the exact shape hydration moves. The
`flashwt()` shell function pins every invocation to the fixture's `FLASHFLASHWT_STORE`.

**Isolation is mandatory.** `FLASHFLASHWT_STORE` defaults to `~/.cache/flashwt/store`, the
developer's machine-wide store. Never run a single `flashwt` command without
`FLASHFLASHWT_STORE` pointing at the fixture store: even a `create` that hydrates
nothing writes a worktree mirror there and registers a git worktree on the
hosting repo. Two runs against one store also contend on its lockfiles.

## Doctor

Run whenever anything looks off, before driving:

```sh
"$FLASHWT_BIN" --version            # prints flashwt x.y.z; empty or error = wrong binary
git --version                  # flashwt shells out to git for every worktree op
[ -w "$(dirname "$FLASHFLASHWT_STORE")" ] && echo "store parent writable"
git -C "$FLASHFLASHWT_ORIGIN" rev-parse --is-inside-work-tree   # expect: true

# APFS clonefile readiness:
if [ "$(uname -s)" = "Darwin" ]; then
  stat -f "%f %T" "$FLASHFLASHWT_STORE" 2>/dev/null | grep -qi "apfs" \
    && echo "APFS clonefile ready" \
    || echo "Non-APFS filesystem (falls back to byte copies)"
fi

# Store GC migration state:
if [ -f "$FLASHFLASHWT_STORE/gc-mode" ]; then
  echo "store gc-mode: $(cat "$FLASHFLASHWT_STORE/gc-mode")"
else
  echo "store gc-mode: legacy (refcount)"
fi

# CLI surface check across subcommands:
"$FLASHWT_BIN" --help | grep -qE 'init' && \
"$FLASHWT_BIN" --help | grep -qE 'hydrate' && \
"$FLASHWT_BIN" --help | grep -qE 'new|create' && \
"$FLASHWT_BIN" --help | grep -qE 'clean|remove' && \
"$FLASHWT_BIN" --help | grep -qE 'list|ls' && \
"$FLASHWT_BIN" --help | grep -qE 'sweep' && \
"$FLASHWT_BIN" --help | grep -qE 'scrub' && \
"$FLASHWT_BIN" --help | grep -qE 'scratch|isolate' && \
"$FLASHWT_BIN" --help | grep -qE 'store' && \
"$FLASHWT_BIN" --help | grep -qE 'demo|test-drive' && \
"$FLASHWT_BIN" --help | grep -qE 'completions' && echo "CLI surface intact (all 11 subcommands verified)"
```

If `--version` prints but a command fails with `git ... fatal`, the fixture
repo is broken. Rebuild it with `mkfixture.sh` rather than debugging it.

## Subcommands

Flash Flash-WT exposes 11 distinct command surfaces (with modern primary verbs and legacy aliases):

1. **`init`**: Initialize a starter `.flashwtinclude` manifest in the repository root or target directory.
   ```sh
   flashwt init [--dir <path>] [--force]
   ```
2. **`new` / `create`**: Provision a new git worktree on a new branch with heavy directories hydrated.
   ```sh
   flashwt new <name> [--dir <path>] [--base <ref>] [--manifest <path>]
   flashwt create <name> [--dir <path>] [--base <ref>] [--manifest <path>]
   ```
3. **`hydrate`**: In-place hydration of heavy directories into an existing directory or worktree.
   ```sh
   flashwt hydrate <path> [--source <repo>] [--manifest <path>]
   ```
4. **`list` / `ls`**: Discover active git worktrees, disk usage, and shared deduplication savings.
   ```sh
   flashwt list
   flashwt ls
   ```
5. **`clean` / `remove`**: Tear down worktrees, release store references, and optionally perform GC sweep.
   ```sh
   flashwt clean [name] [--dir <path>] [--all] [--force] [--age <dur>]
   flashwt remove <name> [--dir <path>]
   ```
6. **`sweep`**: Delete unreferenced store entries and expired scratch leases older than `--age`.
   ```sh
   flashwt sweep [--age 0s|90s|10m|1h|7d]
   ```
7. **`scrub`**: Audit store blobs against content addresses and repair or purge corruption.
   ```sh
   flashwt scrub [--dry-run]
   ```
8. **`scratch` / `isolate`**: Create ephemeral leased sandboxes with optional execution and auto-cleanup.
   ```sh
   flashwt scratch [name] [--dir <path>] [--run '<command>'] [--ttl <dur>]
   flashwt isolate [name] [--dir <path>] [--run '<command>'] [--ttl <dur>]
   ```
9. **`store migrate`**: Inspect and migrate the store GC scheme between refcounts and mark-sweep (ADR-0004).
   ```sh
   flashwt store migrate --activate-mark-sweep
   flashwt store migrate --drop-legacy-refs
   ```
10. **`demo` / `test-drive`**: Run zero-setup 10,000-file benchmark, CoW verification, and isolation tests.
    ```sh
    flashwt demo
    flashwt test-drive
    ```
11. **`completions`**: Generate native shell tab-completion scripts.
    ```sh
    flashwt completions <bash|zsh|fish|elvish|powershell>
    ```

## Environment Variables

Flash Flash-WT honors eight environment variables controlling storage paths, acceleration, and verification:

- **`FLASHFLASHWT_STORE`**: Path to the content-addressed store root (default: `~/.cache/flashwt/store`). Pinned to fixture store in verification.
- **`FLASHWT_SNAPSHOTS`**: Whole-directory snapshot caching fast path. Enabled by default on macOS APFS; set `FLASHWT_SNAPSHOTS=0` to disable and force per-file hydration.
- **`FLASHWT_SNAPSHOTS_V2`**: Incremental diff-based snapshot rebuilds. Enabled by default on macOS APFS; set `FLASHWT_SNAPSHOTS_V2=0` to disable.
- **`FLASHWT_VERIFY`**: Force full SHA-256 re-hash of every blob on every run, bypassing snapshot hits and the verified-blob ledger (`FLASHWT_VERIFY=1`).
- **`FLASHWT_HARDLINK`**: Opt into experimental hardlinked materialization with read-only inodes (`FLASHWT_HARDLINK=1`).
- **`FLASHWT_NO_HARDLINK`**: Force plain byte copies instead of clones or hardlinks (`FLASHWT_NO_HARDLINK=1`). Takes precedence over `FLASHWT_HARDLINK`.
- **`FLASHWT_GC_GRACE`**: Retention grace period protecting young store objects and mirrors in mark-sweep mode (default: `15m`, e.g. `FLASHWT_GC_GRACE=1h`).
- **`FLASHWT_TIMING`**: Print per-stage duration metrics (`flashwt-stage ingest=...`) to stderr (`FLASHWT_TIMING=1`).

Boolean variables follow tri-state semantics: `0`, `false`, `no`, `off` disable; unset uses the default; any other value enables.

## Drive

Every command emits a single-line JSON envelope on stdout when `--json` is supplied:

```json
{"flashwt_version":"0.1.0","schema_version":1,"command":"create","status":"ok","data":{...},"diagnostics":[]}
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

Capture to `artifacts/verify-flashwt/<run-id>/` at the repo root (create it; it
survives fixture teardown). Per feature proven, save:

- the literal command, its stdout envelope, stderr, and exit code (one
  `.txt` per step, e.g. `create.stdout.txt`);
- on-disk state alongside the envelope: `ls -R` of the hydrated worktree,
  `cat` of a hydrated file, `git -C "$FLASHFLASHWT_FIXTURE/demo" branch --show-current`,
  and a `find "$FLASHFLASHWT_STORE" -maxdepth 2` listing;
- side effects, not just final screens: for `remove`, show the worktree dir
  is gone and the store mirror was removed; for `sweep`, show `reclaimed`
  in `data`; for `scrub`, show `corrupt`/`deleted` counts.

Proof standards: exercise the real user path (the plain CLI), not test
endpoints or the Rust fixture harness in `crates/flashwt-cli/tests`. Verify
side effects in the store and worktree alongside what stdout claims. Never
point a proof at `~/.cache/flashwt/store`. An envelope from the machine store
proves nothing about isolation and pollutes developer state.

## Cleanup

No daemons exist, so there is nothing to kill. Tear down state in this
order, from the fixture origin:

```sh
cd "$FLASHFLASHWT_ORIGIN"
flashwt remove <name> --dir "$FLASHFLASHWT_FIXTURE/<name>"   # for each live worktree; scratch --run needs none
flashwt sweep --age 0s                             # collects entries and expired scratch leases
rm -rf "$FLASHFLASHWT_FIXTURE"                          # last: the fixture dir holds the store too
```

Never `rm -rf` the fixture before `remove`. Deleting a worktree directory out
from under flashwt makes `flashwt remove` fail with `is not a working tree` and strands
the store mirror. Kill nothing by process name; flashwt leaves no processes
behind. Evidence in `artifacts/verify-flashwt/<run-id>/` is never removed by
cleanup. After teardown, confirm it still exists.

## Helpers

`helpers/mkfixture.sh` (executable) builds the isolated fixture and prints
shell exports to `eval`. It is shown working in Launch above. Re-invoke it
whenever a fixture is broken. Fixtures are disposable, never repaired.

Feature map: [features/README.md](./features/README.md).

