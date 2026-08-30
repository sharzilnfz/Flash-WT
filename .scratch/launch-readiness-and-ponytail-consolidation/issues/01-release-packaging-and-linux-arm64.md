# 01: Release Packaging, Homebrew Formula & Linux ARM64 Support

**What to build:**
Resolve all distribution and installation blockers so that `wt` can be built, packaged, and installed on macOS and Linux (x86_64 and ARM64) without errors:
1. Standardize release archive naming in `.github/workflows/release.yml` to `wt-v${VERSION}-${{ matrix.target }}.tar.gz` to match expectations in `Formula/wt.rb`, `install.sh`, and `scripts/gen-formula.sh`.
2. Add `aarch64-unknown-linux-gnu` target to GitHub Actions release workflow matrix, `Formula/wt.rb`, and `install.sh` target resolution.
3. Normalize `WT_VERSION` in `install.sh` so that version strings with or without a leading `v` resolve to valid GitHub release asset URLs.
4. Verify release formula generation with `scripts/gen-formula.sh` and installer verification with `scripts/smoke-install.sh`.

**Blocked by:** None (can start immediately).

**Status:** ready-for-agent

- [ ] Release archive name in `release.yml` matches `wt-v<VERSION>-<TARGET>.tar.gz`
- [ ] `aarch64-unknown-linux-gnu` added to release workflow, Homebrew formula template, and installer
- [ ] `install.sh` cleanly normalizes `WT_VERSION=0.1.0` and `WT_VERSION=v0.1.0`
- [ ] `scripts/gen-formula.sh` runs successfully against simulated release artifacts
- [ ] `scripts/smoke-install.sh` passes without 404 or tarball resolution errors
