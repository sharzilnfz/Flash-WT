# 01: Release Packaging, Homebrew Formula & Linux ARM64 Support

Status: ready-for-agent

Blocked by: None (can start immediately).

## Problem

Inconsistencies in release archive naming, the absence of Linux ARM64 release binaries, and strict version tag expectations in installation scripts will cause automated release pipelines and package managers to fail:
1. Release archive naming in `.github/workflows/release.yml` does not match the archive naming expected by `Formula/flashwt.rb`, `install.sh`, and `scripts/gen-formula.sh`.
2. Linux ARM64 (`aarch64-unknown-linux-gnu`) is not built or distributed in release workflows, preventing installation on ARM64 Linux systems (e.g. AWS Graviton).
3. `install.sh` does not normalize version tags, causing 404 download errors if `FLASHWT_VERSION` is specified without or with a leading `v`.
4. Homebrew formula generation and release scripts do not package shell completions.

## Work

1. Standardize release archive naming in `.github/workflows/release.yml` to `flashwt-v${VERSION}-${{ matrix.target }}.tar.gz`.
2. Add `aarch64-unknown-linux-gnu` target matrix across GitHub Actions release workflow, `Formula/flashwt.rb`, and `install.sh` target resolution.
3. Normalize `FLASHWT_VERSION` in `install.sh` so that version strings with or without a leading `v` resolve to valid GitHub release asset URLs.
4. Update `Formula/flashwt.rb` and `scripts/gen-formula.sh` to package and install shell completions for Homebrew users.
5. Verify release formula generation with `scripts/gen-formula.sh` and installer verification with `scripts/smoke-install.sh`.

## Files Owned

- `.github/workflows/release.yml`
- `Formula/flashwt.rb`
- `install.sh`
- `scripts/gen-formula.sh`
- `scripts/smoke-install.sh`

## Done When

- [ ] Release archive name in `release.yml` matches `flashwt-v<VERSION>-<TARGET>.tar.gz`.
- [ ] `aarch64-unknown-linux-gnu` added to release workflow, Homebrew formula template, and installer.
- [ ] `install.sh` cleanly normalizes `FLASHWT_VERSION=0.1.0` and `FLASHWT_VERSION=v0.1.0`.
- [ ] `Formula/flashwt.rb` and `scripts/gen-formula.sh` configure shell completion installation.
- [ ] `scripts/gen-formula.sh` runs successfully against simulated release artifacts.
- [ ] `scripts/smoke-install.sh` passes without 404 or tarball resolution errors.
