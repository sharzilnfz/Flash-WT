# Flash-WT

[![CI](https://github.com/sharzilnfz/Flash-WT/actions/workflows/ci.yml/badge.svg)](https://github.com/sharzilnfz/Flash-WT/actions/workflows/ci.yml)

Instant git worktrees with heavy dependency directories already hydrated.

```sh
flashwt new feat-auth
```

`flashwt` creates a git worktree on a new branch with `node_modules/`, `target/`, `.venv/`, and build caches already in place. Files materialize from a local content-addressed store or your donor checkout using copy-on-write clones (`clonefile` on macOS, `reflink`/`copy_file_range` on Linux). Instead of copying gigabytes across physical disk sectors, heavy directories share physical storage blocks until modified.

## Why Flash-WT

Standard `git worktree add` checks out source code quickly, but leaves untracked dependencies empty. Every fresh worktree requires manually copying dependency folders or re-running package managers and compilers.

`flashwt` streamlines this workflow:

- **Instant hydration.** Hydrates project dependencies in sub-seconds via APFS `clonefile` on macOS or reflink and `copy_file_range` on Linux.
- **Copy-on-write storage.** Worktree files share physical disk blocks with the donor checkout and store until modified, avoiding redundant disk usage.
- **Private and isolated.** Worktree files are normal, fully writable files. Edits in one worktree never bleed into sibling worktrees or the central store.
- **Cross-branch lockfile guard.** If target branch lockfiles differ from the donor checkout, hydration is refused cleanly to prevent corrupted dependencies.
- **No background daemons.** No background sync processes, daemons, or filesystem watchers. All operations run synchronously on demand.
- **Agent automation.** Every command supports `--json` output conforming to a frozen schema for AI coding workflows.

### How it compares to physical duplication

| Operation | Standard byte copy | Flash-WT (Copy-on-Write) | Benefit |
|---|---|---|---|
| Heavy dir hydration (10,000 files) | ~1,000–3,000 ms (full byte I/O) | **~10–50 ms** (clonefile/reflink) | Near-instant checkout |
| Storage footprint per worktree | 100% duplicated bytes | **0 B shared** (CoW metadata only) | Multi-worktree density |
| Isolation | Isolated | **Isolated** (break-on-write) | Zero bleed across branches |

## Installation

### Homebrew (macOS)

```sh
brew install https://github.com/sharzilnfz/Flash-WT/releases/latest/download/flashwt.rb
```

### curl installer (macOS and Linux)

```sh
curl -fsSL https://raw.githubusercontent.com/sharzilnfz/Flash-WT/main/install.sh | sh
```

The script installs prebuilt binaries to `~/.local/bin` and drops shell completions into your shell configuration.

### Build from source

```sh
cargo install --locked --path crates/flashwt-cli
```

## Quick start

Run these commands inside any git repository:

```sh
# Create a worktree with hydrated dependencies on branch feat-auth
flashwt new feat-auth

# List active worktrees and physical disk savings
flashwt list

# Run a command in an ephemeral sandbox that cleans up on exit
flashwt scratch --run "pnpm test"

# Inspect store health and filesystem copy capabilities
flashwt doctor

# Remove a worktree and release store references
flashwt clean feat-auth

# Prune unreferenced objects from the store
flashwt sweep
```

## Command reference

| Command | Description | Example |
|---|---|---|
| `new` | Create a worktree with hydrated heavy folders | `flashwt new feat-auth --base main` |
| `hydrate` | Hydrate an existing directory in place | `flashwt hydrate ./target-dir` |
| `list` | Display active worktrees and storage savings | `flashwt list --json` |
| `scratch` | Run a task in a temporary isolated worktree | `flashwt scratch --run "cargo test"` |
| `clean` | Delete a worktree and release store references | `flashwt clean feat-auth` |
| `sweep` | Run garbage collection on unreferenced store blobs | `flashwt sweep --dry-run` |
| `scrub` | Verify store integrity and remove corrupt blobs | `flashwt scrub` |
| `doctor` | Inspect environment, store health, and copy backend | `flashwt doctor` |
| `store du` | Show disk usage breakdown in the store | `flashwt store du` |
| `lease show` | Show active ephemeral scratch leases | `flashwt lease show` |
| `init` | Generate a `.flashwtinclude` manifest | `flashwt init` |
| `demo` | Run a self-contained 10,000-file benchmark | `flashwt demo` |
| `completions` | Generate shell completions (bash, zsh, fish) | `flashwt completions zsh` |

## Configuration

By default, `flashwt` automatically hydrates standard dependency directories: `node_modules/`, `target/`, `.venv/`, `dist/`, `build/`, `.cache/`, and `__pycache__/`.

To customize included or excluded paths, create a `.flashwtinclude` file in your repository root:

```gitignore
node_modules/
target/
.venv/
!node_modules/.cache/
```

### Environment variables

| Variable | Default | Purpose |
|---|---|---|
| `FLASHWT_STORE` | `~/.cache/flashwt/store` | Path for the content-addressed object store. |
| `FLASHWT_SNAPSHOTS` | `1` on APFS, `0` elsewhere | Enable whole-tree APFS snapshots. Set to `0` for per-file hydration. |
| `FLASHWT_SNAPSHOTS_V2` | `1` on APFS, `0` elsewhere | Enable diff-based incremental snapshot updates. |
| `FLASHWT_TINY_BYPASS` | `1` | Bypass store ingestion for repositories under 500 files and 8 MB. |
| `FLASHWT_HARDLINK` | `0` | Enable hardlinks instead of copy-on-write clones. |
| `FLASHWT_GC_GRACE` | `15m` | Grace period before unreferenced objects are purged during sweep. |

## How it works

- **Content-addressed store.** Files live once under `~/.cache/flashwt/store`, indexed by SHA-256 hash.
- **Copy-on-write placement.** Files materialize through `clonefile(2)` on APFS, or `ioctl(FICLONE)` and `copy_file_range(2)` on Linux. Files share storage until modified.
- **Directory snapshots.** Cached directory trees avoid per-file traversal on warm checkouts.
- **Tiny repository bypass.** Small repositories under 500 files and 8 MB clone directly via recursive copy-on-write in ~50 ms.
- **Crash safety.** Atomic receipt files track operation progress. Interrupted commands resume safely on the next invocation.
- **Sanitization.** Volatile compiler caches such as `target/debug/incremental` and `CACHEDIR.TAG` are removed on checkout so builds stay reliable.

## Development

```sh
cargo test
cargo clippy --all-targets --locked -- -D warnings
cargo fmt --check
./scripts/verify/run_all.sh --quick
```

## License

[MIT](LICENSE)
