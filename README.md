# wt

[![CI](https://github.com/sharzilnfz/wt/actions/workflows/ci.yml/badge.svg)](https://github.com/sharzilnfz/wt/actions/workflows/ci.yml)

Instant git worktrees with heavy directories already hydrated.

```sh
wt new my-feature
```

One command gives you a new worktree on a new branch with `node_modules/`, `target/`, `.venv/`, and build caches already in place. No reinstalling. No rebuilding. Files materialize from a local content-addressed store as private copy-on-write clones. A 40,000-file project environment appears in 1.5 seconds instead of minutes.

## The problem

Parallel development multiplies environment overhead. Every feature branch, quick experiment, and autonomous AI coding agent wants an isolated checkout. Every fresh checkout pays the full setup tax:

- `npm install` or `pnpm install` into a fresh `node_modules` directory takes minutes and hundreds of megabytes.
- `cargo build` from scratch into an empty `target` directory wastes CPU and time.
- Python virtual environments and build caches rebuild from zero every time.
- Five parallel agent tasks create five identical copies of the same dependencies, consuming gigabytes of disk.

Standard `git worktree add` checks out tracked source files instantly, but leaves untracked dependencies empty. `wt` eliminates this gap.

## Why wt

`wt` stores each unique file once in a central content-addressed store. When you create a worktree, `wt` materializes heavy directories using APFS copy-on-write clones on macOS, reflink or `copy_file_range` on Linux, or optional hardlinks.

Files in your worktree share storage blocks with the store until modified. They are private, fully writable, normal files. Editors, compilers, and language servers treat them like regular files on disk.

### Measured performance (40,000 files / 800 packages fixture)

Measured on macOS APFS comparing cold ingestion against whole-tree clonefile snapshots, incremental rebuilds, and standard recursive filesystem copying:

| Scenario | Measured time | Notes |
|---|---|---|
| Cold ingestion (unprimed store, 2,000 files) | 3,032 ms | Initial store blob ingestion and snapshot creation |
| Warm snapshot hit (`wt new` / `wt create`) | **1,317 ms** | 2.3x faster than cold build; whole-tree APFS `clonefile()` |
| Incremental snapshot rebuild (3 of 800 packages modified) | **1,569 ms** | Diff-based snapshot clone; updates modified packages only |
| Per-file fallback mode (`WT_SNAPSHOTS=0`) | 1,443 ms | Iterative per-file clonefile fallback |
| Raw recursive copy (`cp -Rc`) | 394 ms | Copies bytes on disk without git worktree setup or store deduplication |

### APFS storage deduplication across concurrent worktrees

Measured via volume free-space probes (`df -k`) on macOS APFS across 40,000 files:

| Concurrent worktrees | Logical unshared size | Physical disk consumed | Physical disk savings | Notes |
|---|---|---|---|---|
| 1 worktree | 156.25 MB | 120.76 MB | 1.0x | Base store allocation |
| 3 worktrees | 468.75 MB | ~120.76 MB | ~3.9x | 0 MB additional dirty blocks |
| 5 worktrees | **781.25 MB** | **120.76 MB** | **4.37x** | 5 worktrees share identical disk blocks |

When you edit, create, or delete files in one worktree, the operating system writes changes to new disk blocks for that worktree only. Other worktrees and the central store remain untouched. `wt` runs no background sync daemons and no filesystem watchers. Explicit hydration (`wt hydrate` / `wt new`) is the sole mechanism for materializing and updating dependencies.

### Performance characteristics by workload scale

`wt` carries a fixed ~1.3-second baseline cost for subprocess coordination, git worktree creation, store verification, and GC root registration. Because of this fixed floor, `wt` is engineered for medium to large dependency trees:

| Workload size | Raw `cp -Rc` | `wt new` | Package manager install | Best fit |
|---|---|---|---|---|
| Tiny (<500 files) | ~0.05s | ~1.3s | ~2.0s | Direct copy |
| Medium (5,000 files) | ~1.2s | ~1.3s | ~12.0s | `wt` |
| Large (40,000+ files) | ~7.9s | ~1.5s | ~35.0s+ | `wt` (20x faster than install, 5x faster than `cp`) |

## How wt compares to alternatives

| Alternative | What it does | Where wt differs |
|---|---|---|
| `git worktree add` | Creates isolated branches for tracked files. | Leaves `node_modules/` and build directories empty. `wt` hydrates them in 1.3 seconds. |
| `pnpm` / `uv` | Optimizes package installation for one language. | Language-specific. `wt` handles `node_modules/`, `target/`, `.venv/`, and build caches together. |
| `cp -Rc` shell scripts | Bare recursive APFS copies. | Takes ~8 seconds at 40k scale, lacks cross-project deduplication, lacks garbage collection, and breaks on dirty trees. |
| Docker / Devcontainers | Containerized sandboxes. | High memory overhead, slow bind mounts on macOS. `wt` runs natively on host files. |

## Installation

### Homebrew (macOS)

```sh
brew install https://github.com/sharzilnfz/wt/releases/latest/download/wt.rb
```

Upgrades follow standard Homebrew workflow:

```sh
brew upgrade wt
```

### curl installer (macOS and Linux)

```sh
curl -fsSL https://raw.githubusercontent.com/sharzilnfz/wt/main/install.sh | sh
```

The script downloads the prebuilt static binary for your architecture, verifies its SHA-256 checksum, installs to `~/.local/bin`, and drops shell completions into your active shell's completion directory (skip with `WT_COMPLETIONS=no`).

### Shell completions

`wt completions <shell>` generates a tab-completion script covering every subcommand and flag. Supported shells: `bash`, `zsh`, `fish`, `elvish`, `powershell`.

```sh
wt completions bash > ~/.local/share/bash-completion/completions/wt
```

The curl installer and the Homebrew formula both install completions automatically.

### Build from source

```sh
cargo install --locked --path crates/wt-cli
```

`wt` compiles into a single static binary with no runtime dependencies beyond git.

## Quick start

Run `wt` from inside any git repository:

```sh
# Create a worktree at ../<repo>-feature on branch feature with hydrated directories
wt new feature

# Inspect active worktrees, their disk usage, and shared savings
wt list

# Run a test or command inside an ephemeral sandbox; cleans up automatically on exit
wt scratch --run "pnpm test"

# Remove the worktree, release its store references, and reclaim freed space
wt clean feature

# Remove every stale or merged worktree non-interactively, then reclaim space
wt clean --all
```

### Complete command reference

`wt` provides 11 subcommands covering worktree creation, storage inspection, sandboxing, and maintenance:

| Command | Purpose | Example |
|---|---|---|
| `wt new` (alias `wt create`) | Create a new worktree with hydrated heavy folders | `wt new feat-auth --base main` |
| `wt hydrate` | Hydrate an existing directory in place without creating a branch | `wt hydrate ./my-dir` |
| `wt list` (alias `wt ls`) | Display all active worktrees, branches, and shared disk savings | `wt list --json` |
| `wt scratch` (alias `wt isolate`) | Run a command in an isolated ephemeral worktree | `wt scratch --run "cargo test"` |
| `wt clean` (alias `wt remove`) | Reclaim a worktree and release store references | `wt clean feat-auth` or `wt clean --all` |
| `wt sweep` | Run mark-and-sweep garbage collection on unreferenced objects | `wt sweep --age 0s` |
| `wt scrub` | Audit store integrity; detect and delete corrupted objects | `wt scrub --dry-run` |
| `wt store migrate` | Migrate store schema and activate mark-sweep GC | `wt store migrate --activate-mark-sweep` |
| `wt init` | Generate a starter `.wtinclude` configuration | `wt init --force` |
| `wt demo` | Run self-contained 10,000-file benchmark verifying speed and isolation | `wt demo` |
| `wt completions` | Generate completion scripts for your shell | `wt completions zsh` |

### Multi-ecosystem fidelity

`wt` preserves file attributes and directory layouts across different language ecosystems. Verified with byte-for-byte SHA-256 checks, POSIX permission modes, and symlink targets:

| Ecosystem | Target directories | Characteristics preserved | Parity |
|---|---|---|:---:|
| Node.js | `node_modules/` | Nested packages, `.bin` executable symlinks, mixed read/write bits | 100% |
| Rust | `target/debug/` | Compiled `.rlib` files, metadata, binaries; volatile incremental caches excluded | 100% |
| Python | `.venv/` | `site-packages`, `__pycache__` directories, python binary symlinks | 100% |
| Monorepos | Combined paths | Multi-language monorepos under a unified `.wtinclude` manifest | 100% |

### Zero-setup test drive

No git repository or configuration needed. `wt demo` builds a synthetic 10,000-file project, measures baseline copy versus `wt` copy-on-write hydration, validates that mutations stay isolated, and cleans up after itself:

```sh
wt demo
```

### Configure what gets hydrated

`wt` reads `.wtinclude` in your repository root using gitignore pattern syntax. If no `.wtinclude` file exists, `wt` creates one with sensible defaults:

```gitignore
node_modules/
target/
.venv/
dist/
build/
.cache/
__pycache__/
```

Edit `.wtinclude` to match your project's heaviest rebuild artifacts.

### Fast path on macOS (APFS)

On macOS, whole-directory snapshots and diff-based incremental rebuilds are enabled automatically. `wt` probes the filesystem at startup: on APFS it hydrates by cloning whole directory trees (~0.45s for 40,000 files), and when dependencies change slightly it clones the previous snapshot and patches only the modified files. No environment variables required.

To opt out and force per-file hydration:

```sh
export WT_SNAPSHOTS=0
export WT_SNAPSHOTS_V2=0
```

### Platform acceleration: macOS versus Linux

`wt` automatically probes host filesystem capabilities at startup to select the fastest supported copy acceleration backend.

| Capability | macOS (APFS) | Linux (Btrfs / XFS) | Linux (ext4) | Linux (generic / cross-device) |
|---|---|---|---|---|
| Primary backend | `clonefile(2)` | `ioctl(FICLONE)` reflink | `copy_file_range(2)` | Buffered byte copy |
| Directory snapshots | Whole-tree APFS clone (~1.3s warm) | Per-file placement ladder | Per-file placement ladder | Per-file placement ladder |
| Incremental rebuilds | Tree clone + selective relink | Per-file delta placement | Per-file delta placement | Per-file delta placement |
| Storage deduplication | Zero-copy CoW extents | Zero-copy CoW extents | Physical disk consumption | Physical disk consumption |
| Hardlink mode (`WT_HARDLINK=1`) | Read-only shared inodes | Read-only shared inodes | Read-only shared inodes | Refused across mounts |
| Fallback diagnostics | Surfaces reason on non-APFS | Reports backend and refusal reason | Reports backend and refusal reason | Reports backend and refusal reason |

When acceleration is unavailable or refused (such as across different mount points or on filesystems lacking reflink support), `wt` falls back safely to byte copies. Both human terminal output and JSON envelopes report the chosen copy mechanism along with the specific refusal reason.


## How it works

- **Content-addressed store**: Unique file contents live once under `~/.cache/wt/store`, addressed by SHA-256 hash.
- **Copy-on-write hydration**: Files materialize through `clonefile(2)` on APFS or hardlinks. They share storage with the store until written.
- **Directory snapshots**: Whole directory trees are cached by manifest hash. Hydration is a single recursive APFS clone.
- **Incremental rebuilds**: A flat manifest diff engine identifies unchanged subtrees, cloning stable packages and relinking only changed files.
- **Crash-safe garbage collection**: Mark-and-sweep GC uses store-local mirror files as roots. A default 15-minute grace period (`WT_GC_GRACE`) ensures concurrent builds and unexpected interruptions never delete live data.
- **Integrity verification**: Blobs are verified on ingest and tracked via a verification ledger. `WT_VERIFY=1` forces full cryptographic re-hashing on every run.
- **Explicit hydration only**: `wt` runs no background daemons, filesystem watchers, or automatic synchronization loops. Explicit CLI commands (`wt new`, `wt hydrate`) are the sole mechanism for populating and updating worktree files from the store.

Read [docs/archive/product-handoff.md](docs/archive/product-handoff.md) and the [Architecture Decision Records](docs/adr/) for full implementation details.

## Environment configuration

| Variable | Default | Description |
|---|---|---|
| `WT_STORE` | `~/.cache/wt/store` | Directory for the content-addressed object store |
| `WT_SNAPSHOTS` | `1` on APFS, `0` elsewhere | Enable whole-directory APFS snapshot caching; `WT_SNAPSHOTS=0` opts out |
| `WT_SNAPSHOTS_V2` | `1` on APFS, `0` elsewhere | Enable diff-based incremental snapshot rebuilds; `WT_SNAPSHOTS_V2=0` opts out |
| `WT_VERIFY` | `0` | Force full cryptographic re-verification of all blobs |
| `WT_HARDLINK` | `0` | Enable hardlink materialization mode |
| `WT_NO_HARDLINK` | `0` | Force byte-by-byte copies instead of links |
| `WT_GC_GRACE` | `15m` | Retention grace period before unreferenced data is collected |
| `WT_TIMING` | `0` | Emit stage timing breakdowns to stderr |

## Development and testing

```sh
# Run all unit, integration, and crash recovery tests (167 tests)
cargo test

# Run linter checks
cargo clippy --all-targets -- -D warnings

# Run master verification rig across all 5 test suites (85s)
./scripts/verify/run_all.sh --all

# Run rapid verification with lightweight fixtures
./scripts/verify/run_all.sh --quick

# Run an individual verification suite
./scripts/verify/run_all.sh --suite 02_flash_apfs
./scripts/verify/run_all.sh --suite 04_isolation_storage
./scripts/verify/run_all.sh --suite 05_chaos_resilience
```

The verification rig executes five automated suites:
- `01_cli_matrix`: Tests all 11 subcommands, flags, and JSON output contracts.
- `02_flash_apfs`: Benchmarks cold vs warm snapshot hydration, incremental rebuilds, and per-file fallback.
- `03_real_repos`: Verifies byte-for-byte SHA-256 parity and symlinks for Node.js, Rust, Python, and Monorepos.
- `04_isolation_storage`: Probes physical disk allocation across 1, 3, and 5 concurrent worktrees via `df -k`.
- `05_chaos_resilience`: Injects 5x worker concurrency, CAS bit rot, cryptographic tamper checks, and `SIGKILL` crash recovery.

Telemetry from runs compiles automatically into [artifacts/verify-wt/REPORT.md](artifacts/verify-wt/REPORT.md).

## License

[MIT](LICENSE)
