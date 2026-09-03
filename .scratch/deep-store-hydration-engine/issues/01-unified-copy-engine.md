# 01: Unified Copy Engine in flashwt-copy

**What to build:** Consolidate directory copying and per-file placement into a deep `CopyEngine` in `flashwt-copy`. The engine automatically detects volume boundaries and filesystem capabilities, selects the optimal placement strategy (APFS clonefile on macOS, reflink or copy_file_range on Linux, or buffered byte copying), normalizes file permissions, and returns an execution report. Callers no longer probe device identifiers or pass raw boolean capability flags.

**Blocked by:** None (can start immediately)

**Status:** ready-for-agent

- [x] `flashwt-copy` exposes a unified `CopyEngine` providing `copy_dir` and `materialize_files` operations
- [x] Filesystem capability detection and device boundary comparisons are encapsulated inside the copy crate
- [x] Callers supply source paths, destination paths, and optional strategy policies without passing boolean capability flags
- [x] File permissions and executable bit normalization are handled transparently during placement
- [x] Unit and integration tests in `flashwt-copy` verify whole-directory cloning, file materialization, fallback byte copying, and cross-device handling
