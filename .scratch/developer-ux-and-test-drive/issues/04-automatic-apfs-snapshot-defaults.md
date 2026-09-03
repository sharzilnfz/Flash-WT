# 04: Automatic APFS & Diff-Rebuild Runtime Feature Detection

**What to build:**
Enable directory-level APFS snapshots (`FLASHWT_SNAPSHOTS`) and v2 incremental diff rebuilds (`FLASHWT_SNAPSHOTS_V2`) by default on macOS APFS filesystems without requiring manual environment variables. Retain fallback to per-file copy-on-write or byte copying on unsupported platforms, and treat environment variables `FLASHWT_SNAPSHOTS=0` and `FLASHWT_SNAPSHOTS_V2=0` as explicit opt-out toggles.

**Blocked by:**
01: Zero-Setup Self-Test Drive (`flashwt demo`)

**Status:**
ready-for-human

- [x] Update `RunConfig` resolution to probe host filesystem and default `snapshots` and `snapshots_v2` to enabled on macOS APFS.
- [x] Ensure explicit environment variables (`FLASHWT_SNAPSHOTS=0`, `FLASHWT_SNAPSHOTS_V2=0`) allow opt-out disablement.
- [x] Preserve silent, safe fallback ladder on Linux and non-APFS volumes.
- [x] Update CLI help documentation to reflect smart defaults rather than requiring manual environment setup.
- [x] Add integration tests verifying default behavior across platforms and explicit opt-out flags.
