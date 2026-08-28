# 01: Versioned JSON output envelope across CLI commands

**What to build:** Add a global `--json` CLI flag to `wt`. When passed, suppress human-readable stdout output and emit a single line of NDJSON containing command results, schema version, execution status, typed data payload, and diagnostics. Route all diagnostic logs and stderr messages away from stdout. Support structured envelopes across `create`, `remove`, `sweep`, and `scrub` commands. Include hydration methods, byte counters, and storage boundary diagnostics.

**Blocked by:** None (can start immediately).

**Status:** ready-for-agent

- [x] Global `--json` flag is parsed across `create`, `remove`, `sweep`, and `scrub` subcommands.
- [x] Command execution on stdout emits a single line NDJSON envelope with `wt_version`, integer `schema_version` (value `1`), `command`, `status` (`"ok"` or `"error"`), `data`, and `diagnostics` array.
- [x] Non-JSON human progress lines and warnings are suppressed or redirected to stderr when `--json` is active.
- [x] `wt create --json` data payload includes worktree path, branch, cache hit status, duration, `hydration_method`, `bytes_shared_cow`, and `bytes_copied`.
- [x] `wt remove --json`, `wt sweep --json`, and `wt scrub --json` emit typed machine-readable payload summaries.
- [x] Automated CLI integration tests verify schema conformance and stderr isolation for all commands.
