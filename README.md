# wt

Instant git worktrees with heavy directories already hydrated. One
dependency-free binary: `wt create feature` gives you a new worktree with
`node_modules`, `target/`, and friends already in place — copied through a
content-addressed store so nothing is rewritten, just linked or cloned.

## Quick start

**curl** (macOS arm64/x86_64, Linux x86_64):

```sh
curl -fsSL https://raw.githubusercontent.com/sharzilnafis/wt/main/install.sh | sh
```

This downloads the release tarball for your platform, verifies its SHA-256
checksum, installs the binary into `~/.local/bin`, and runs `wt --version`
to confirm it works before reporting success. If `~/.local/bin` is not on
your `PATH`, the installer prints the exact line to add to your shell
profile.

**Homebrew:**

```sh
brew install https://github.com/sharzilnafis/wt/releases/latest/download/wt.rb
```

The formula picks the right tarball for your machine, and upgrades follow
the usual `brew upgrade wt`. The formula carries checksums for all three
release targets; Homebrew refuses anything that does not match.

Both paths end at the same place — one static binary called `wt`.

## Usage

From inside any git repository:

```sh
wt create my-feature
```

That creates a worktree at `<repo>-my-feature` next to your checkout on a
new `my-feature` branch. If a `.wtinclude` manifest exists in the repo
root, the directories it lists are hydrated into the new worktree;
otherwise defaults (`node_modules/`, `target/`, `dist/`, `build/`,
`.cache/`, `.venv/`, `__pycache__/`) are used and a starter `.wtinclude`
is written for you to edit.

Point the manifest at whatever your project rebuilds most and never wait
on an install again.

## Requirements

- git (that is all; the binary has no runtime dependencies)

## Development

```sh
cargo test          # unit + integration tests
./scripts/smoke-install.sh   # exercises both install paths against a local build
```

Releases are cut by pushing a `v*` tag; CI builds, signs, and publishes
binaries plus the brew formula.
