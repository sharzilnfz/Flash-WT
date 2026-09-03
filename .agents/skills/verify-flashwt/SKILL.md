---
name: verify-flashwt
description: >
  Launch, drive, and prove the behavior of `flashwt`, the instant-git-worktrees CLI
  in this repo (Rust, binary surface only, no server). Use whenever a change to
  flashwt needs end-to-end proof: creating/removing worktrees, hydration from the
  content-addressed store, sweep/scrub GC, scratch/isolate sandboxes, or store migration.
  Drives the real binary against throwaway fixture repos with an isolated
  store, asserts on --json envelopes plus on-disk state.
---

# Verify flashwt

`flashwt` is a CLI, not a service. There is no server to keep alive.
Launch means build the binary once, then drive each proof inside a throwaway fixture
repo with its own private store.

## Launch

Build or reuse the release binary:

```sh
cargo build --release -p flashwt-cli
FLASHWT_BIN=target/release/flashwt
"$FLASHWT_BIN" --version
```

Ready signal: `--version` prints and exits 0. No ports, no auth, no seed data.

For each proof, build a fixture and load its exports:

```sh
eval "$(helpers/mkfixture.sh "$FLASHWT_BIN")"
cd "$FLASHWT_ORIGIN"
```

The fixture is a real git repo with 40 untracked files under `heavy/` and a
`.flashwtinclude` manifest naming `heavy/`, the exact shape hydration moves. The
`flashwt()` shell function pins every invocation to the fixture's `FLASHWT_STORE`.

Isolation is mandatory. `FLASHWT_STORE` defaults to `~/.cache/flashwt/store`, the
developer machine-wide store. Never run a single `flashwt` command without
`FLASHWT_STORE` pointing at the fixture store. Even a `create` that hydrates
nothing writes a worktree mirror there and registers a git worktree on the
hosting repo. Two runs against one store also contend on its lockfiles.

## Doctor

Run whenever anything looks off, before driving:

```sh
"$FLASHWT_BIN" --version
git --version
flashwt doctor
flashwt --json doctor
```

The built-in doctor command verifies store path writability, environment variable overrides,
APFS clonefile capabilities, and store byte usage across objects, snapshots, mirrors, refs, and caches.

If `--version` prints but a command fails with `git fatal`, the fixture
repo is broken. Rebuild it with `mkfixture.sh` rather than debugging it.

## Subcommands

flashwt exposes 13 distinct command surfaces (with primary verbs and aliases):

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
5. **`clean`**: Remove worktrees with merge and dirty safety checks, followed by immediate store sweep.
   ```sh
   flashwt clean [name] [--dir <path>] [--all] [--force] [--age <dur>]
   ```
6. **`remove`**: Low-level primitive to unregister a worktree and mirror without triggering store sweep.
   ```sh
   flashwt remove <name> [--dir <path>]
   ```
7. **`sweep`**: Delete unreferenced store entries and expired scratch leases older than `--age`.
   ```sh
   flashwt sweep [--age <dur>] [--dry-run]
   ```
8. **`scrub`**: Audit store blobs and snapshot directories against content addresses and purge corruption.
   ```sh
   flashwt scrub [--dry-run]
   ```
9. **`scratch` / `isolate`**: Create ephemeral leased sandboxes with optional execution and auto-cleanup.
   ```sh
   flashwt scratch [name] [--dir <path>] [--run '<command>'] [--ttl <dur>]
   flashwt isolate [name] [--dir <path>] [--run '<command>'] [--ttl <dur>]
   ```
10. **`lease`**: Inspect active and expired ephemeral scratch worktree leases.
    ```sh
    flashwt lease show [id] [--all]
    flashwt lease list
    ```
11. **`store`**: Manage store GC mode and inspect categorized store disk usage.
    ```sh
    flashwt store du
    flashwt store migrate --activate-mark-sweep
    flashwt store migrate --drop-legacy-refs
    ```
12. **`doctor`**: Inspect environment, filesystem capabilities, and store diagnostics.
    ```sh
    flashwt doctor
    ```
13. **`demo` / `test-drive`**: Run zero-setup 10,000-file benchmark, CoW verification, and isolation tests.
    ```sh
    flashwt demo
    flashwt test-drive
    ```
14. **`completions`**: Generate native shell tab-completion scripts.
    ```sh
    flashwt completions <bash|zsh|fish|elvish|powershell>
    ```

## Environment Variables

flashwt honors environment variables controlling storage paths, acceleration, and verification:

- **`FLASHWT_STORE`**: Path to the content-addressed store root (default: `~/.cache/flashwt/store`).
- **`FLASHWT_SNAPSHOTS`**: Whole-directory snapshot caching fast path. Enabled by default on macOS APFS.
- **`FLASHWT_SNAPSHOTS_V2`**: Incremental diff-based snapshot rebuilds. Enabled by default on macOS APFS.
- **`FLASHWT_VERIFY`**: Force full SHA-256 re-hash of every blob on every run, bypassing snapshot hits.
- **`FLASHWT_HARDLINK`**: Opt into experimental hardlinked materialization with read-only inodes (`FLASHWT_HARDLINK=1`). Requires `FLASHWT_SNAPSHOTS=0` on macOS APFS.
- **`FLASHWT_NO_HARDLINK`**: Force plain byte copies instead of clones or hardlinks (`FLASHWT_NO_HARDLINK=1`). Takes precedence over `FLASHWT_HARDLINK`.
- **`FLASHWT_NO_TINY_BYPASS`**: Disable tiny repository bypass (`FLASHWT_NO_TINY_BYPASS=1`). Repos with under 500 files bypass store ingestion unless this flag is set.
- **`FLASHWT_GC_GRACE`**: Retention grace period protecting young store objects and mirrors in mark-sweep mode (default: `15m`).
- **`FLASHWT_SNAPSHOT_CAP`**: Maximum number of unreferenced snapshots retained before LRU eviction (default: `64`).
- **`FLASHWT_MAX_SNAPSHOT_BYTES`**: Total byte budget ceiling for snapshot cache retention.
- **`FLASHWT_TIMING`**: Print per-stage duration metrics to stderr.

Boolean variables follow tri-state semantics: `0`, `false`, `no`, `off` disable; unset uses the default; any other value enables.

## Drive

Every command emits a single-line JSON envelope on stdout when `--json` is supplied:

```json
{"flashwt_version":"0.1.0","schema_version":1,"command":"create","status":"ok","data":{},"diagnostics":[]}
```

Assert on `status`, on `data` fields, and on disk state.
`diagnostics` carries warning-level codes such as `CROSS_DEVICE_COPY_DEGRADATION` or `ZERO_SAVINGS`.
The `hydration_method` is filesystem-dependent (`byte_copy` on tmpfs, clonefile-based CoW on APFS).

The user-facing features each have a full recipe in [features/](./features/README.md).
Read the map first before driving.
For extensive automated test runs across all subcommands and edge cases, execute the comprehensive test runner:

```sh
scripts/verify/run_all.sh
```

## Evidence

Capture to `artifacts/verify-flashwt/<run-id>/` at the repo root.
Per feature proven, save:

- the literal command, its stdout envelope, stderr, and exit code.
- on-disk state alongside the envelope: `ls -R` of the hydrated worktree, `cat` of a hydrated file, `git branch --show-current`, and `find "$FLASHWT_STORE"` listings.
- side effects, not just final screens: for `remove`, show the worktree directory is gone; for `sweep`, show `reclaimed` in `data`; for `scrub`, show `corrupt` and `deleted` counts.

Proof standards: exercise the real user path (the plain CLI), not test endpoints or the internal harness in `crates/flashwt-cli/tests`.
Verify side effects in the store and worktree alongside what stdout claims.
Never point a proof at `~/.cache/flashwt/store`.

## Cleanup

Tear down state in this order, from the fixture origin:

```sh
cd "$FLASHWT_ORIGIN"
flashwt remove <name> --dir "$FLASHWT_FIXTURE/<name>"
flashwt sweep --age 0s
rm -rf "$FLASHWT_FIXTURE"
```

Never `rm -rf` the fixture before `remove`.
Deleting a worktree directory out from under flashwt makes `flashwt remove` fail with `is not a working tree` and strands the store mirror.
Kill nothing by process name. flashwt leaves no background daemons behind.
Evidence in `artifacts/verify-flashwt/<run-id>/` is never removed by cleanup.
After teardown, confirm it still exists.

## Helpers

`helpers/mkfixture.sh` (executable) builds the isolated fixture and prints shell exports to `eval`.
It sets `FLASHWT_BIN`, `FLASHWT_FIXTURE`, `FLASHWT_ORIGIN`, `FLASHWT_STORE`, and `FLASHWT_NO_TINY_BYPASS=1`.
Re-invoke it whenever a fixture is broken. Fixtures are disposable, never repaired.

Feature map: [features/README.md](./features/README.md).
