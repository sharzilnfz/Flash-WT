# Flash-WT

[![CI](https://github.com/sharzilnfz/Flash-WT/actions/workflows/ci.yml/badge.svg)](https://github.com/sharzilnfz/Flash-WT/actions/workflows/ci.yml)

Instant git worktrees with heavy dependency directories already hydrated.

```sh
flashwt new feat-auth
```

`flashwt` creates a git worktree on a new branch with `node_modules/`, `target/`, `.venv/`, and build caches in place. Files materialize from a local content-addressed store using copy-on-write clones. A 40,000-file project workspace appears in 1.3 seconds instead of minutes.

## Why Flash-WT

Standard `git worktree add` checks out source code quickly. It leaves untracked dependencies empty. Every fresh checkout pays the full setup tax again. Running `npm install`, `cargo build`, or recreating virtual environments across parallel branches wastes time, CPU, and disk space.

`flashwt` eliminates this friction:

- **Instant hydration.** Hydrates project dependencies in ~1.3 seconds via APFS `clonefile` on macOS or reflink and `copy_file_range` on Linux.
- **Zero-copy storage.** Worktree files share physical disk blocks with the store until modified. Five concurrent worktrees of a 150 MB project take ~120 MB total on disk instead of 780 MB.
- **Private and isolated.** Worktree files are normal, fully writable files. Changes in one worktree never affect sibling branches or the central store.
- **No background daemons.** No background sync processes and no filesystem watchers. All actions run on demand.
- **Agent automation.** Every command supports `--json` output conforming to a frozen schema for AI coding workflows.

### Measured performance (40,000 files / 800 packages)

| Operation | Standard approach | Flash-WT | Delta |
|---|---|---|---|
| Fresh worktree setup | 35.0s (`npm install`) | **1.3s** (`flashwt new`) | 27x faster |
| Disk space (5 worktrees) | 781 MB (unshared copies) | **121 MB** (APFS CoW) | 6.5x storage saved |
| Small repo setup (<500 files) | 2.0s (package install) | **0.05s** (tiny bypass) | 40x faster |

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
