# 01: Skeleton, contracts, and the end-to-end test rig

**What to build:** The repo scaffold with crate boundaries agreed upfront, so
parallel agents can build against frozen interfaces without touching each
other's files. Defines the two central traits: the copy-backend trait
(clonefile, reflink, hardlink behind one interface) and the store trait
(hash-addressed put/get, reference counting). Also fixes the CLI argument
shape for `wt create` and builds the founding end-to-end test rig: tests run
the binary against real temporary git repositories containing generated fake-
heavy directories, asserting only through the CLI boundary.

**Blocked by:** None (can start immediately).

**Status:** ready-for-agent

- [x] Crate layout compiles with both traits defined and stubbed
- [x] `wt create --help` works and argument shape is documented in the ticket thread
- [x] Test rig creates a temp repo with thousands of small fake-heavy files
- [x] One smoke test passes through the CLI boundary end to end
- [x] Traits have doc comments precise enough that tickets 02-04 can code against them without asking questions

## Comments

### Frozen CLI argument shape (ticket 02 codes against this)

```
wt create <NAME> [--manifest <PATH>] [--dir <PATH>]
```

- `<NAME>` — required positional. Used as the git branch name.
- `--manifest <PATH>` — optional. Manifest listing heavy directories,
  gitignore syntax. Defaults to `.wtinclude` at the repository root.
- `--dir <PATH>` — optional destination for the new worktree. Defaults
  to a sibling of the current repository named `<repo>-<NAME>` (for
  example, repo `origin` + `wt create demo` produces `../origin-demo`).
- Exit code 0 on success, 1 with a message on `wt:` stderr on failure
  (including "destination already exists").

`--help` output must keep mentioning NAME, --manifest, and --dir; the
test rig asserts on those strings.

### Crate layout

Cargo workspace, three crates:

- `crates/wt-copy` — the copy-backend trait (`CopyBackend`,
  `BackendKind`, `Safety`, `Error`). Ticket 03 implements backends
  here. `BackendKind::Hardlink` reports `Safety::UnsafePending` until
  ticket 07.
- `crates/wt-store` — the store trait (`Store`, `ContentId`, `Error`).
  Ticket 04 implements the real store here.
- `crates/wt-cli` — the `wt` binary. Depends on both crates. Owns all
  end-to-end tests under `tests/`.

Both traits ship with placeholder `Stub*` implementations so every
crate compiles before tickets 03/04 land.

### Trait contracts worth restating

- `put` does not take a reference; callers pair explicit `add_ref` /
  `release_ref`. Underflow is an error.
- `get` verifies the hash and returns `Corrupted` instead of bad bytes.
- `copy_dir(src, dest)` requires `dest` not to exist and copies the
  whole tree; symlinks are never followed out of `src`.
- Backend selection asks `supports(dir)`, which answers for the
  filesystem holding that directory.

### What was built

Test rig lives in `crates/wt-cli/tests/`: `common/mod.rs` builds a
temp git repo (one commit, `.gitignore`d `heavy/` directory with 2,000
generated small files across nested dirs), then runs the real binary
via `CARGO_BIN_EXE_wt`. Four tests pass: help shape, smoke `wt create
demo` producing a working worktree, fixture file count, and failure
when the destination exists. In v0 of `create`, hydration only prints
a notice; tickets 02 and 05 fill it in through the frozen traits.
