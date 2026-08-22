# 09: Distribution

**What to build:** A stranger goes from nothing to working tool without
building anything: release binaries for macOS arm64/x86_64 and Linux x86_64
built by CI, installed via brew formula and a curl installer script.

**Blocked by:** 07 (hardlink safety), so no one downloads a tool whose
fallback can corrupt projects.

**Status:** ready-for-human

- [x] CI builds signed release binaries for all three targets on tag push
- [x] Curl installer installs the right binary and verifies it runs
- [x] Brew formula installs and upgrades cleanly
- [ ] Fresh-machine smoke test passes for both install paths
- [x] README quick start matches what the installers actually do

## Comments

### Agent, 2026-08-23

Implemented on branch `fleet/09-distribution`:

- `.github/workflows/release.yml` — builds on `v*` tag push for
  aarch64-apple-darwin, x86_64-apple-darwin, x86_64-unknown-linux-gnu;
  packages tarballs plus sha256 sidecars; macOS binaries are codesigned
  with Developer ID when `MACOS_CERTIFICATE`/`MACOS_IDENTITY` secrets are
  configured, ad-hoc otherwise; publishes the release with a checksummed,
  ready-to-use brew formula (`wt.rb`).
- `.github/workflows/ci.yml` — cargo test plus the installer smoke script
  on macOS and Linux runners on every push, so the install paths are
  exercised before any tag exists.
- `install.sh` — curl installer: platform detection, checksum
  verification before extraction, install to `~/.local/bin`
  (`WT_BIN_DIR` to override), runs `wt --version` to prove the binary
  works, PATH hint if needed.
- `Formula/wt.rb` + `scripts/gen-formula.sh` — formula template with
  per-target checksum placeholders; generation fills version, download
  base, and checksums from release artifacts.
- `scripts/smoke-install.sh` — exercises both paths without a release:
  curl path against a local dist laid out like GitHub's per-tag layout
  (including a corrupt-checksum rejection case), brew path installs an
  older version then upgrades through a throwaway tap.
- `README.md` — quick start mirrors exactly what the two installers do.

Verified locally: full cargo suite green, both workflows parse as valid
YAML, generated formula passes `ruby -c`, and `./scripts/smoke-install.sh`
passes end to end on macOS arm64 (curl install + verify, bad-checksum
rejection, brew install 0.0.1 -> upgrade -> 0.1.0).

Left for a human (needs things only a real tag/fresh machine provides):

1. The unchecked acceptance item: fresh-machine verification of both
   install paths against a published release.
2. `WT_REPO` defaults to `sharzilnafis/wt` (no git remote exists yet).
   When the repo lands on GitHub, update the default in `install.sh`,
   the `env:` block in `release.yml`, and the README URLs if it differs.
3. Signing falls back to ad-hoc until `MACOS_CERTIFICATE`,
   `MACOS_CERTIFICATE_PASSWORD`, `MACOS_KEYCHAIN_PASSWORD`, and
   `MACOS_IDENTITY` secrets are set; notarization is not wired up.
4. The Linux target is glibc-linked, built on ubuntu-22.04; a musl/static
   variant would widen compatibility if old-distro users show up.
