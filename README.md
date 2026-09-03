# Flash-WT

[![CI](https://github.com/sharzilnfz/Flash-WT/actions/workflows/ci.yml/badge.svg)](https://github.com/sharzilnfz/Flash-WT/actions/workflows/ci.yml)

Instant git worktrees with heavy directories already hydrated.

```sh
flashwt new my-feature
```

One command gives you a new worktree on a new branch with `node_modules/`, `target/`, `.venv/`, and build caches already in place. No reinstalling. No rebuilding. Files materialize from a local content-addressed store as private copy-on-write clones. A 40,000-file project environment appears in 1.5 seconds instead of minutes.

## The problem

Parallel development multiplies environment overhead. Every feature branch, quick experiment, and autonomous AI coding agent wants an isolated checkout. Every fresh checkout pays the full setup tax:

- `npm install` or `pnpm install` into a fresh `node_modules` directory takes minutes and hundreds of megabytes.
- `cargo build` from scratch into an empty `target` directory wastes CPU and time.
- Python virtual environments and build caches rebuild from zero every time.
- Five parallel agent tasks create five identical copies of the same dependencies, consuming gigabytes of disk.

Standard `git worktree add` checks out tracked source files instantly, but leaves untracked dependencies empty. `flashwt` eliminates this gap.

## Why flashwt `flashwt` stores each unique file once in a central content-addressed store. When you create a worktree, `flashwt` materializes heavy directories using APFS copy-on-write clones on macOS, reflink or `copy_file_range` on Linux, or optional hardlinks.

Files in your worktree share storage blocks with the store until modified. They are private, fully writable, normal files. Editors, compilers, and language servers treat them like regular files on disk.

### Measured performance (40,000 files / 800 packages fixture)

Measured on macOS APFS comparing cold ingestion against whole-tree clonefile snapshots, incremental rebuilds, and standard recursive filesystem copying:

| Scenario | Measured time | Notes |
|---|---|---|
| Cold ingestion (unprimed store, 2,000 files) | 3,032 ms | Initial store blob ingestion and snapshot creation |
| Warm snapshot hit (`flashwt new` / `flashwt create`) | **1,317 ms** | 2.3x faster than cold build; whole-tree APFS `clonefile()` |
| Incremental snapshot rebuild (3 of 800 packages modified) | **1,569 ms** | Diff-based snapshot clone; updates modified packages only |
| Per-file fallback mode (`FLASHWT_SNAPSHOTS=0`) | 1,443 ms | Iterative per-file clonefile fallback |
| Raw recursive copy (`cp -Rc`) | 394 ms | Copies bytes on disk without git worktree setup or store deduplication |

### APFS storage deduplication across concurrent worktrees

Measured via volume free-space probes (`df -k`) on macOS APFS across 40,000 files:

| Concurrent worktrees | Logical unshared size | Physical disk consumed | Physical disk savings | Notes |
|---|---|---|---|---|
| 1 worktree | 156.25 MB | 120.76 MB | 1.0x | Base store allocation |
| 3 worktrees | 468.75 MB | ~120.76 MB | ~3.9x | 0 MB additional dirty blocks |
| 5 worktrees | **781.25 MB** | **120.76 MB** | **4.37x** | 5 worktrees share identical disk blocks |

When you edit, create, or delete files in one worktree, the operating system writes changes to new disk blocks for that worktree only. Other worktrees and the central store remain untouched. `flashwt` runs no background sync daemons and no filesystem watchers. Explicit hydration (`flashwt hydrate` / `flashwt new`) is the sole mechanism for materializing and updating dependencies.

### Performance characteristics by workload scale

`flashwt` automatically applies a **tiny repository bypass** policy for projects under 500 files and 8 MB: it skips store ingestion and snapshot indexing, executing `git worktree add` with direct recursive copy-on-write materialization. This brings tiny worktree creation down from ~1.3s to **~0.05s**, matching bare `cp -Rc`.

For medium and large projects, `flashwt`'s content-addressed store, snapshot caching, parallel streaming ingestion, and batch durability provide up to 20x speedups over package installations and 5x over recursive copies:

| Workload size | Raw `cp -Rc` | `flashwt new` | Package manager install | Best fit |
|---|---|---|---|---|
| Tiny (<500 files, <8 MB) | ~0.05s | **~0.05s** (tiny bypass) | ~2.0s | `flashwt new` or direct copy |
| Medium (5,000 files) | ~1.2s | ~1.3s | ~12.0s | `flashwt` |
| Large (40,000+ files) | ~7.9s | ~1.5s | ~35.0s+ | `flashwt` (20x faster than install, 5x faster than `cp`) |

## How flashwt compares to alternatives

| Alternative | What it does | Where flashwt differs |
|---|---|---|
| `git worktree add` | Creates isolated branches for tracked files. | Leaves `node_modules/` and build directories empty. `flashwt` hydrates them in 1.3 seconds. |
| `pnpm` / `uv` | Optimizes package installation for one language. | Language-specific. `flashwt` handles `node_modules/`, `target/`, `.venv/`, and build caches together. |
| `cp -Rc` shell scripts | Bare recursive APFS copies. | Takes ~8 seconds at 40k scale, lacks cross-project deduplication, lacks garbage collection, and breaks on dirty trees. |
| Docker / Devcontainers | Containerized sandboxes. | High memory overhead, slow bind mounts on macOS. `flashwt` runs natively on host files. |

## Installation

### Homebrew (macOS)

```sh
brew install https://github.com/sharzilnfz/Flash-WT/releases/latest/download/flashwt.rb
```

Upgrades follow standard Homebrew workflow:

```sh
brew upgrade flashwt
```

### curl installer (macOS and Linux)

```sh
curl -fsSL https://raw.githubusercontent.com/sharzilnfz/Flash-WT/main/install.sh | sh
```

The script downloads the prebuilt static binary for your architecture, verifies its SHA-256 checksum, installs to `~/.local/bin`, and drops shell completions into your active shell's completion directory (skip with `FLASHWT_COMPLETIONS=no`).

### Shell completions

`flashwt completions <shell>` generates a tab-completion script covering every subcommand and flag. Supported shells: `bash`, `zsh`, `fish`, `elvish`, `powershell`.

```sh
flashwt completions bash > ~/.local/share/bash-completion/completions/flashwt
```

The curl installer and the Homebrew formula both install completions automatically.

### Build from source

```sh
cargo install --locked --path crates/flashwt-cli
```

`flashwt` compiles into a single static binary with no runtime dependencies beyond git.

## Quick start

Run `flashwt` from inside any git repository:

```sh
# Create a worktree at ../<repo>-feature on branch feature with hydrated directories
flashwt new feature

# Inspect active worktrees, their disk usage, and shared savings
flashwt list

# Diagnose environment variables, store health, and filesystem copy acceleration
flashwt doctor

# Inspect storage breakdown across CAS blobs, snapshots, mirrors, and caches
flashwt store du

# Run a test or command inside an ephemeral sandbox; cleans up automatically on exit
flashwt scratch --run "pnpm test"

# Inspect active ephemeral scratch leases and remaining TTL
flashwt lease show

# Preview unreferenced objects and disk space that sweep would reclaim without deleting
flashwt sweep --dry-run

# Remove the worktree, release its store references, and reclaim freed space
flashwt clean feature

# Remove every stale or merged worktree non-interactively, then reclaim space
flashwt clean --all
```

### Complete command reference

`flashwt` provides 14 subcommands and administrative actions covering worktree creation, storage inspection, sandboxing, and maintenance:

| Command | Purpose | Example |
|---|---|---|
| `flashwt new` (alias `flashwt create`) | Create a new worktree with hydrated heavy folders | `flashwt new feat-auth --base main` |
| `flashwt hydrate` | Hydrate an existing directory in place without creating a branch | `flashwt hydrate ./my-dir` |
| `flashwt list` (alias `flashwt ls`) | Display all active worktrees, branches, and shared disk savings | `flashwt list --json` |
| `flashwt scratch` (alias `flashwt isolate`) | Run a command in an isolated ephemeral worktree with lease persistence | `flashwt scratch --run "cargo test"` |
| `flashwt clean` (alias `flashwt remove`) | Reclaim a worktree and release store references | `flashwt clean feat-auth` or `flashwt clean --all` |
| `flashwt doctor` | Inspect environment variables, store configuration, filesystem capabilities, and disk usage | `flashwt doctor` |
| `flashwt store du` (alias `flashwt store disk-usage`) | Print breakdown of disk usage in the store | `flashwt store du` |
| `flashwt store migrate` | Migrate store schema and activate mark-sweep GC | `flashwt store migrate --activate-mark-sweep` |
| `flashwt sweep` | Run mark-and-sweep garbage collection on unreferenced objects | `flashwt sweep --age 0s` or `flashwt sweep --dry-run` |
| `flashwt scrub` | Audit store integrity; detect and delete corrupted objects | `flashwt scrub --dry-run` |
| `flashwt lease show` (alias `flashwt lease ls`) | Inspect active ephemeral scratch leases, PID liveness, and remaining TTL | `flashwt lease show` |
| `flashwt init` | Generate a starter `.flashwtinclude` configuration | `flashwt init --force` |
| `flashwt demo` (alias `flashwt test-drive`) | Run self-contained 10,000-file benchmark verifying speed and isolation | `flashwt demo` |
| `flashwt completions` | Generate completion scripts for your shell | `flashwt completions zsh` |

### Multi-ecosystem fidelity

`flashwt` preserves file attributes and directory layouts across different language ecosystems. Verified with byte-for-byte SHA-256 checks, POSIX permission modes, and symlink targets:

| Ecosystem | Target directories | Characteristics preserved | Parity |
|---|---|---|:---:|
| Node.js | `node_modules/` | Nested packages, `.bin` executable symlinks, mixed read/write bits | 100% |
| Rust | `target/debug/` | Compiled `.rlib` files, metadata, binaries; volatile incremental caches excluded | 100% |
| Python | `.venv/` | `site-packages`, `__pycache__` directories, python binary symlinks | 100% |
| Monorepos | Combined paths | Multi-language monorepos under a unified `.flashwtinclude` manifest | 100% |

### Zero-setup test drive

No git repository or configuration needed. `flashwt demo` builds a synthetic 10,000-file project, measures baseline copy versus `flashwt` copy-on-write hydration, validates that mutations stay isolated, and cleans up after itself:

```sh
flashwt demo
```

### Configure what gets hydrated

`flashwt` works immediately in any repository without manual configuration. If no `.flashwtinclude` manifest is present, `flashwt` automatically applies standard defaults:

```gitignore
node_modules/
target/
.venv/
dist/
build/
.cache/
__pycache__/
```

If no matching heavy directories exist or matched directories contain zero files to hydrate, `flashwt` provides explicit zero-savings feedback (`0 bytes saved (no matching heavy directories found and the worktree relies strictly on git tracking)`) rather than failing.

To customize what gets hydrated or add exclusions, create a `.flashwtinclude` file in your repository root using gitignore pattern syntax (or run `flashwt init` to generate a starter template):

```gitignore
# Include heavy directories
node_modules/
target/
.venv/

# Exclude volatile subdirectories with standard negation syntax
!node_modules/.cache/
```

### Fast path on macOS (APFS)

On macOS, whole-directory snapshots and diff-based incremental rebuilds are enabled automatically. `flashwt` probes the filesystem at startup: on APFS it hydrates by cloning whole directory trees (~0.45s for 40,000 files), and when dependencies change slightly it clones the previous snapshot and patches only the modified files. Diff-based incremental snapshot rebuilds (`FLASHWT_SNAPSHOTS_V2`) include an automatic guard: if modifications exceed 10% of entries or on lockfile miss, `flashwt` automatically falls back to a clean full snapshot clone, ensuring wide diffs never execute slower than fresh clones. No environment variables required.

To opt out and force per-file hydration:

```sh
export FLASHWT_SNAPSHOTS=0
export FLASHWT_SNAPSHOTS_V2=0
```

### Platform acceleration: macOS versus Linux

`flashwt` automatically probes host filesystem capabilities at startup to select the fastest supported copy acceleration backend.

| Capability | macOS (APFS) | Linux (Btrfs / XFS) | Linux (ext4) | Linux (generic / cross-device) |
|---|---|---|---|---|
| Primary backend | `clonefile(2)` | `ioctl(FICLONE)` reflink | `copy_file_range(2)` | Buffered byte copy |
| Directory snapshots | Whole-tree APFS clone (~1.3s warm) | Per-file placement ladder | Per-file placement ladder | Per-file placement ladder |
| Incremental rebuilds | Tree clone + selective relink | Per-file delta placement | Per-file delta placement | Per-file delta placement |
| Storage deduplication | Zero-copy CoW extents | Zero-copy CoW extents | Physical disk consumption | Physical disk consumption |
| Hardlink mode (`FLASHWT_HARDLINK=1`) | Read-only shared inodes | Read-only shared inodes | Read-only shared inodes | Refused across mounts |
| Fallback diagnostics | Surfaces reason on non-APFS | Reports backend and refusal reason | Reports backend and refusal reason | Reports backend and refusal reason |

When acceleration is unavailable or refused (such as across different mount points or on filesystems lacking reflink support), `flashwt` falls back safely to byte copies. Both human terminal output and JSON envelopes report the chosen copy mechanism along with the specific refusal reason.

### Machine-readable JSON contract and agent automation

`flashwt` is engineered for automated orchestration and AI coding agents. Every subcommand supports a global `--json` flag producing a frozen single-line NDJSON envelope adhering to `schema/v1.json` (see `schema/CHANGELOG.md`):

```json
{
  "flashwt_version": "0.1.0",
  "schema_version": 1,
  "command": "create",
  "status": "ok",
  "data": {
    "branch": "feat-auth",
    "worktree_path": "/path/to/repo-feat-auth",
    "cache_hit": true,
    "hydration_method": "clone",
    "bytes_shared_cow": 126615552,
    "bytes_copied": 0,
    "total_files": 40000,
    "duration_ms": 1317
  },
  "diagnostics": []
}
```

Key capabilities for agent fleets:
- **Stable diagnostic codes**: Errors and warnings return structured uppercase codes (e.g., `ZERO_SAVINGS`, `CROSS_DEVICE_COPY_DEGRADATION`, `CORRUPT_BLOB`, `UNREFERENCED_OBJECTS`) instead of unstructured text.
- **Execution receipts & atomic crash recovery**: Mutating commands persist atomic receipt files (`flashwt-receipt.json`) in the worktree's git directory tracking lifecycle state (`in_progress`, `completed`, `failed`). If a command is interrupted by `SIGKILL` or an abrupt termination, subsequent `flashwt new` or `flashwt clean` invocations detect the receipt and resume hydration or clean up without manual intervention.
- **Scratch lease management**: Ephemeral sandbox worktrees persist leases with PID tracking and TTL expiration, inspectable via `flashwt lease show --json`.

## How it works

- **Content-addressed store**: Unique file contents live once under `~/.cache/flashwt/store`, addressed by SHA-256 hash.
- **Parallel streaming ingest & batch durability**: Ingestion streams files in 64 KB chunks across worker threads with batched parent-directory syncs, eliminating per-file fsync bottlenecks on cold ingest.
- **Copy-on-write hydration**: Files materialize through `clonefile(2)` on APFS, reflink or `copy_file_range` on Linux, or optional hardlinks. They share physical storage blocks until written.
- **Directory snapshots & incremental rebuild guard**: Whole directory trees are cached by manifest hash. Incremental rebuilds diff and relink modified subtrees, with an automatic guard falling back to full snapshot clones when diffs exceed 10% or lockfiles mismatch.
- **Tiny repository bypass**: Repositories under 500 files and 8 MB automatically bypass the CAS and clone directly via recursive CoW, completing in ~50ms.
- **Validation cache alias protection**: The validation cache mixes inode and ctime alongside file size and mtime, rehashing near-now mtimes to prevent stale cache hits on rapid same-size file edits.
- **Post-hydration sanitization**: Automatically purges volatile compiler caches (`incremental/`, `CACHEDIR.TAG`, `.fingerprint`) during hydration so toolchains rebuild reliably without state corruption.
- **Execution receipts & crash safety**: Atomic receipts (`flashwt-receipt.json`) record operation progress, allowing interrupted worktree creations to cleanly resume.
- **Crash-safe garbage collection**: Mark-and-sweep GC uses store-local mirror files as roots with a default 15-minute grace period (`FLASHWT_GC_GRACE`), dual-writing snapshot member references during migration to safeguard live objects.
- **Integrity verification & scrub**: Blobs are verified on ingest and tracked via a verification ledger. `flashwt scrub` re-verifies all CAS blobs against bit rot.
- **Coalesced git operations**: Process-level rev-parse query caching eliminates redundant subprocess spawning.
- **Explicit hydration only**: `flashwt` runs no background daemons, filesystem watchers, or automatic synchronization loops. Explicit CLI commands (`flashwt new`, `flashwt hydrate`) are the sole mechanism for populating and updating worktree files from the store.

Read [docs/archive/product-handoff.md](docs/archive/product-handoff.md) and the [Architecture Decision Records](docs/adr/) for full implementation details.

## Environment configuration

| Variable | Default | Description |
|---|---|---|
| `FLASHWT_STORE` | `~/.cache/flashwt/store` | Directory for the content-addressed object store |
| `FLASHWT_SNAPSHOTS` | `1` on APFS, `0` elsewhere | Enable whole-directory APFS snapshot caching; `FLASHWT_SNAPSHOTS=0` opts out |
| `FLASHWT_SNAPSHOTS_V2` | `1` on APFS, `0` elsewhere | Enable diff-based incremental snapshot rebuilds; `FLASHWT_SNAPSHOTS_V2=0` opts out |
| `FLASHWT_TINY_BYPASS` | `1` | Bypass store for repos under 500 files and 8 MB; `FLASHWT_TINY_BYPASS=0` opts out |
| `FLASHWT_NO_TINY_BYPASS` | `0` | Force all repos through the store, disabling tiny repo bypass |
| `FLASHWT_VERIFY` | `0` | Force full cryptographic re-verification of all blobs |
| `FLASHWT_HARDLINK` | `0` | Enable hardlink materialization mode |
| `FLASHWT_NO_HARDLINK` | `0` | Force byte-by-byte copies instead of links |
| `FLASHWT_GC_GRACE` | `15m` | Retention grace period before unreferenced data is collected |
| `FLASHWT_SNAPSHOT_CAP` | `50` | Maximum number of snapshot rings retained per heavy directory |
| `FLASHWT_MAX_SNAPSHOT_BYTES` | *unset* | Byte cap on snapshot creation before falling back to per-file placement |
| `FLASHWT_TIMING` | `0` | Emit stage timing breakdowns to stderr |

## Development and testing

```sh
cargo test
cargo clippy --all-targets -- -D warnings
./scripts/verify/run_all.sh --all
./scripts/verify/run_all.sh --quick
./scripts/verify/run_all.sh --suite 02_flash_apfs
./scripts/verify/run_all.sh --suite 04_isolation_storage
./scripts/verify/run_all.sh --suite 05_chaos_resilience
```

The verification rig executes five automated suites:
- `01_cli_matrix`: Tests all 14 subcommands, flags, and frozen v1 JSON envelope contracts.
- `02_flash_apfs`: Benchmarks cold vs warm snapshot hydration, incremental rebuilds, and per-file fallback.
- `03_real_repos`: Verifies byte-for-byte SHA-256 parity and symlinks for Node.js, Rust, Python, and Monorepos.
- `04_isolation_storage`: Probes physical disk allocation across 1, 3, and 5 concurrent worktrees via `df -k`.
- `05_chaos_resilience`: Injects 5x worker concurrency, CAS bit rot, cryptographic tamper checks, and `SIGKILL` crash recovery.

Telemetry from runs compiles automatically into [artifacts/verify-flashwt/REPORT.md](artifacts/verify-flashwt/REPORT.md).

## License

[MIT](LICENSE)
