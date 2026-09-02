# 07: Deep Presentation and Formatting Module

Status: ready-for-agent

Blocked by: `06-cli-dead-code-clap-aliases-and-dep-pruning.md`

## Problem

Multiple command modules (`create.rs`, `clean.rs`, `list.rs`, `demo.rs`) independently implement redundant formatting helpers (`format_bytes`, `format_duration`, `format_number`, `format_count`). They have slight arithmetic and decimal precision discrepancies and produce duplicated formatting code across the CLI crate.

## Work

1. Introduce a deep `output` module in `crates/wt-cli/src/output.rs` that encapsulates:
   - Byte unit scaling and formatting (`HumanBytes`).
   - Duration representation (`HumanDuration`).
   - Digit grouping for file and object counts (`HumanCount`).
   - Aligned terminal table and scorecard rendering helpers.
2. Replace duplicate formatting helper functions across `create.rs`, `clean.rs`, `list.rs`, and `demo.rs` with calls to the presentation module.
3. Add unit tests for the presentation module covering zero values, unit boundaries, large numbers, and decimal precision.

## Files Owned

- `crates/wt-cli/src/output.rs`
- `crates/wt-cli/src/main.rs`
- `crates/wt-cli/src/commands/create.rs`
- `crates/wt-cli/src/commands/clean.rs`
- `crates/wt-cli/src/commands/list.rs`
- `crates/wt-cli/src/commands/demo.rs`
- `crates/wt-cli/tests/output.rs`

## Done When

- [ ] Presentation helpers reside in `crates/wt-cli/src/output.rs`.
- [ ] `create.rs`, `clean.rs`, `list.rs`, and `demo.rs` use the unified presentation module.
- [ ] No duplicate `format_bytes` or `format_duration` functions remain in command handlers.
- [ ] Unit tests for `output.rs` pass with comprehensive edge-case coverage.
