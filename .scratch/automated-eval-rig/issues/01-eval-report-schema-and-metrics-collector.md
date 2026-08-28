# 01: JSON metrics schema and stage metrics collector

**What to build:** A unified metrics collection and aggregation engine that captures system telemetry, parses `WT_TIMING=1` stderr stage lines, and accumulates sample distributions into a structured JSON report format.

**Blocked by:** None (can start immediately)

**Status:** ready-for-agent

- [ ] Define JSON schema capturing platform metadata (OS, kernel, CPU, architecture), commit SHA, scenario parameters, and stage timings.
- [ ] Implement stream parser for `wt-stage <name>=<ms>` output lines, mapping them to stage models (git-worktree, ingest, references, materialize, snapshot lookup/clonefile/link-train).
- [ ] Implement statistical distribution accumulator computing sample count, mean, median, p95, standard deviation, and interquartile range (IQR).
- [ ] Implement triple-axis fidelity result recorder tracking regular file byte diffs, file mode diffs, and symlink target diffs.
- [ ] Unit tests verifying parsing accuracy on malformed, partial, and complete `wt-stage` logs.
