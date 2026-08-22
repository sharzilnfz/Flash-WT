# 09: Distribution

**What to build:** A stranger goes from nothing to working tool without
building anything: release binaries for macOS arm64/x86_64 and Linux x86_64
built by CI, installed via brew formula and a curl installer script.

**Blocked by:** 07 (hardlink safety), so no one downloads a tool whose
fallback can corrupt projects.

**Status:** ready-for-agent

- [ ] CI builds signed release binaries for all three targets on tag push
- [ ] Curl installer installs the right binary and verifies it runs
- [ ] Brew formula installs and upgrades cleanly
- [ ] Fresh-machine smoke test passes for both install paths
- [ ] README quick start matches what the installers actually do
