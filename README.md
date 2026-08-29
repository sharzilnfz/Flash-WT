# wt

[![CI](https://github.com/sharzilnfz/wt/actions/workflows/ci.yml/badge.svg)](https://github.com/sharzilnfz/wt/actions/workflows/ci.yml)

Instant git worktrees with heavy directories already hydrated.

```sh
wt create my-feature
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

`wt` stores each unique file once in a central content-addressed store. When you create a worktree, `wt` materializes heavy directories using APFS copy-on-write clones on macOS or hardlinks on Linux.

Files in your worktree share storage blocks with the store until modified. They are private, fully writable, normal files. Editors, compilers, and language servers treat them like regular files on disk.

### Measured performance (40,000 files / 800 packages fixture)

Measured on macOS APFS against a clean dependency install baseline:

| Scenario | Without wt | With wt | Speedup |
|---|---|---|---|
| Warm worktree creation | 11.4s (fresh install) | **1.5s** | **7.6x** |
| Directory clone vs raw `cp -Rc` | 7.9s | **1.5s** | **5.3x** |
| Rebuild after dependency bump (3 of 800 packages changed) | 17.8s (full rebuild) | **5.3s** | **3.4x** |
| Rebuild after cache poisoning (.DS_Store added) | 18.0s (full rebuild) | **5.7s** | **3.2x** |

### Performance characteristics by workload scale

`wt` carries a fixed ~1.5-second baseline cost for subprocess coordination, git worktree creation, store verification, and GC root registration. Because of this fixed floor, `wt` is engineered specifically for heavy directory structures rather than trivial single-file trees:

| Workload Size | Raw `cp -Rc` | `wt create` | Real Package Manager Install | Best Fit |
|---|---|---|---|---|
| **Tiny (<500 files)** | ~0.05s | ~1.8s | ~2.0s | Direct copy |
| **Medium (5,000 files)** | ~1.2s | ~1.5s | ~12.0s | `wt` |
| **Large (40,000+ files)** | ~7.9s | ~1.6s | ~35.0s+ | `wt` (20x faster than install, 5x faster than `cp`) |

On large trees, `wt` provides instant hydration, eliminates redundant package re-installation, guarantees isolated copy-on-write safety, and deduplicates physical disk usage across multiple checkouts.

## How wt compares to alternatives

| Alternative | What it does | Where wt differs |
|---|---|---|
| `git worktree add` | Creates isolated branches for tracked files. | Leaves `node_modules/` and build directories empty. `wt` hydrates them instantly. |
| `pnpm` / `uv` | Optimizes package installation for one language. | Language-specific. `wt` is language-agnostic and handles `target/`, `.venv/`, caches, and build outputs together. |
| `cp -Rc` shell scripts | Bare recursive APFS copies. | Takes ~8 seconds at 40k scale, lacks cross-project deduplication, lacks garbage collection, and breaks on dirty trees. |
| Docker / Devcontainers | Full containerized sandboxes. | Heavyweight, high memory overhead, and requires complex file synchronization on macOS. `wt` runs natively on host files. |

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

The script downloads the prebuilt static binary for your architecture, verifies its SHA-256 checksum, and installs to `~/.local/bin`.

### Build from source

```sh
cargo install --locked --path crates/wt-cli
```

`wt` compiles into a single static binary with no runtime dependencies beyond git.

## Quick start

Run `wt` from inside any git repository:

```sh
# Create a worktree at ../<repo>-feature on branch feature with hydrated directories
wt create feature

# Remove the worktree and release its store references
wt remove feature

# Reclaim space from deleted worktrees and stale cache data
wt sweep
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

Enable whole-directory snapshots and incremental rebuilds in your shell:

```sh
export WT_SNAPSHOTS=1
export WT_SNAPSHOTS_V2=1
```

- `WT_SNAPSHOTS=1` activates directory-level APFS cloning (~0.45s to hydrate 40,000 files).
- `WT_SNAPSHOTS_V2=1` activates diff-based rebuilds. When dependencies change slightly, `wt` clones the previous snapshot and patches only modified files.

## How it works

- **Content-addressed store**: Unique file contents live once under `~/.cache/wt/store`, addressed by SHA-256 hash.
- **Copy-on-write hydration**: Files materialize through `clonefile(2)` on APFS or hardlinks. They share storage with the store until written.
- **Directory snapshots**: Whole directory trees are cached by manifest hash. Hydration is a single recursive APFS clone.
- **Incremental rebuilds**: A flat manifest diff engine identifies unchanged subtrees, cloning stable packages and relinking only changed files.
- **Crash-safe garbage collection**: Mark-and-sweep GC uses store-local mirror files as roots. A default 15-minute grace period (`WT_GC_GRACE`) ensures concurrent builds and unexpected interruptions never delete live data.
- **Integrity verification**: Blobs are verified on ingest and tracked via a verification ledger. `WT_VERIFY=1` forces full cryptographic re-hashing on every run.

Read [docs/archive/product-handoff.md](docs/archive/product-handoff.md) and the [Architecture Decision Records](docs/adr/) for full implementation details.

## Environment configuration

| Variable | Default | Description |
|---|---|---|
| `WT_STORE` | `~/.cache/wt/store` | Directory for the content-addressed object store |
| `WT_SNAPSHOTS` | `0` | Enable whole-directory APFS snapshot caching (macOS only) |
| `WT_SNAPSHOTS_V2` | `0` | Enable diff-based incremental snapshot rebuilds |
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

# Run benchmark suite with full byte verification
./benchmarks/run.sh --verify

# Run v1 versus v2 incremental rebuild benchmarks
./benchmarks/v2-bench.sh

# Run automated multi-ecosystem evaluation, chaos testing, and regression gating
./benchmarks/eval.sh --verify --chaos --markdown report.md
```

## License

[MIT](LICENSE)
