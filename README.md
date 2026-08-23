# wt

[![CI](https://github.com/sharzilnfz/wt/actions/workflows/ci.yml/badge.svg)](https://github.com/sharzilnfz/wt/actions/workflows/ci.yml)

Instant git worktrees with heavy directories already hydrated.

```sh
wt create my-feature
```

One command gives you a new worktree on a new branch with `node_modules/`,
`target/`, and friends already in place — no reinstall, no rebuild. Files
come out of a local content-addressed store as private copy-on-write
clones, so a 40,000-file environment materializes in about a second
instead of minutes.

## Why

Every fresh checkout pays the same tax: `npm install`, `cargo build`,
cache warm-up — repeated for every branch, every agent session, every
parallel task. The contents already exist on your disk; wt stores each
unique file once and links it everywhere it's needed.

Measured against a fresh-install baseline (40k files / 800 packages):

| Scenario | Without wt | With wt |
|---|---|---|
| Warm environment create | 11.4s | **~1.5s** |
| Rebuild after dependency bump | 17.8s | **5.3s** |
| Same after junk file poisons the tree | 18.0s | **5.7s** |

## Install

**curl** (macOS arm64/x86_64, Linux x86_64):

```sh
curl -fsSL https://raw.githubusercontent.com/sharzilnfz/wt/main/install.sh | sh
```

Downloads the release tarball for your platform, verifies its SHA-256
checksum, installs to `~/.local/bin`, and confirms `wt --version` works.

**Homebrew:**

```sh
brew install https://github.com/sharzilnfz/wt/releases/latest/download/wt.rb
```

Upgrades follow the usual `brew upgrade wt`. The formula carries
checksums for all three release targets.

**From source:**

```sh
cargo install --locked --path crates/wt-cli
```

Both install paths end at the same place: one static binary called `wt`
with no runtime dependencies beyond git.

## Usage

From inside any git repository:

```sh
wt create my-feature     # worktree at <repo>-my-feature, branch my-feature
wt remove my-feature     # remove it and release its store references
wt sweep                 # collect store entries nothing references anymore
```

Hydrated directories are controlled by a `.wtinclude` manifest in the
repo root (gitignore syntax). If none exists, sensible defaults apply
(`node_modules/`, `target/`, `dist/`, `build/`, `.cache/`, `.venv/`,
`__pycache__/`) and a starter manifest is written for you to edit:

```gitignore
node_modules/
target/
```

Point it at whatever your project rebuilds most and never wait on an
install again.

### Faster still (macOS/APFS)

```sh
export WT_SNAPSHOTS=1        # whole-directory snapshot hits: one clonefile per heavy dir
export WT_SNAPSHOTS_V2=1     # incremental rebuilds after small changes
```

With both set, a dependency bump that touches 3 of 800 packages
rebuilds in ~5s instead of ~18s: the previous snapshot tree is cloned
wholesale and only the changed paths are relinked inside the private
copy.

## How it works

- **Store**: unique file contents live exactly once under
  `~/.cache/wt/store` (override with `WT_STORE`), addressed by SHA-256,
  like git's object database.
- **Hydration**: files are materialized as APFS copy-on-write clones or
  hardlinks — private, fully writable, byte-identical to the originals.
  Filesystems without clone support get plain copies automatically.
- **Snapshots**: whole directory trees are cached; a matching hydrate is
  one recursive `clonefile(2)` (~0.45s for 40k files). A v2 incremental
  rebuild clones that tree and applies just the diff.
- **GC**: mark-and-sweep with a grace period (`WT_GC_GRACE`, default
  15m). Crash-tested: a kill at any instant can leak reclaimable cache
  data, never destroy live data.
- **Integrity**: blobs are hash-verified once, then trusted while size
  and mtime hold (`WT_VERIFY=1` re-hashes everything for paranoid runs).

More detail: [docs/product-handoff.md](docs/product-handoff.md) and the
ADRs under [docs/adr/](docs/adr/).

## Environment knobs

| Variable | Effect |
|---|---|
| `WT_STORE` | Store location |
| `WT_SNAPSHOTS=1` | Enable directory snapshots (macOS/APFS) |
| `WT_SNAPSHOTS_V2=1` | Enable diff-based incremental snapshot rebuilds |
| `WT_VERIFY=1` | Full re-hash of every blob / staged file |
| `WT_HARDLINK=1` | Experimental hardlinked materialization (max sharing) |
| `WT_NO_HARDLINK=1` | Force plain byte copies |
| `WT_GC_GRACE` | GC grace period (default `15m`) |
| `WT_TIMING=1` | Per-stage timings on stderr |

## Requirements

- git. That's all.

Snapshot fast paths additionally need macOS on APFS; elsewhere wt uses
its fallback backends and stays correct, just slower on cold paths.

## Development

```sh
cargo test                      # unit + integration tests (167)
cargo clippy --all-targets -- -D warnings
./benchmarks/run.sh --verify    # four scenarios, deep-verified
./benchmarks/v2-bench.sh        # v1-vs-v2 rebuild comparison at scale
./scripts/smoke-install.sh      # exercises both install paths locally
```

Releases are cut by pushing a `v*` tag; CI builds, signs (macOS),
checksums, and publishes binaries plus the brew formula.

## License

[MIT](LICENSE)
