# 02: Worktree Discovery and Disk Accounting (`flashwt list`)

**What to build:**
A `flashwt list` (and `flashwt ls`) command that discovers all active git worktrees for the current repository and displays their branches, paths, hydrated directories, disk savings from copy-on-write deduplication, ephemeral lease TTL status, and creation age.

**Blocked by:**
None (can start immediately).

**Status:**
ready-for-human

- [x] Add `flashwt list` and `flashwt ls` subcommands to CLI parser with optional `--json` support.
- [x] Implement worktree discovery engine parsing git worktree metadata alongside `flashwt-hydrated.tsv` sidecars and store mirror files.
- [x] Compute shared disk space savings by cross-referencing hydrated files with store blob sizes.
- [x] Parse ephemeral scratch lease metadata to display remaining TTL and active PID liveness.
- [x] Format human-readable aligned table output with active worktree markers and total disk savings summary.
- [x] Add integration tests in `crates/flashwt-cli/tests/` asserting `flashwt list` accurately reports created worktrees and outputs valid JSON envelopes.
