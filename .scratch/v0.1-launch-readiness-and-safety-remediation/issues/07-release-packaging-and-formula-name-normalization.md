Status: ready-for-agent

# Issue 07: Release Packaging & Formula Name Normalization

## Problem
1. `.github/workflows/release.yml` produces `wt-0.1.0-<target>.tar.gz` (without `v`), while `install.sh`, `gen-formula.sh`, and `Formula/wt.rb` expect `wt-v0.1.0-<target>.tar.gz`. Release packaging fails on tag push.
2. `install.sh` does not normalize bare `WT_VERSION=0.1.0` vs `v0.1.0` when fetching release assets from GitHub.

## Requirements
1. Update `release.yml` to package release tarballs matching the canonical `wt-v<version>-<target>.tar.gz` format.
2. Normalize version tags in `install.sh` to handle both `0.1.0` and `v0.1.0` transparently.
3. Update Homebrew formula generator script `scripts/gen-formula.sh` to validate the generated formula with `brew audit` or `ruby -c`.

## Verification
- Run `scripts/smoke-install.sh` using release archives generated with the exact naming contract from `release.yml`.
