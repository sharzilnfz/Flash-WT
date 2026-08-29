# Issue 02: Deep Presentation and Formatting Module

Status: ready-for-agent

## Context
Multiple command modules (`create.rs`, `clean.rs`, `list.rs`, `demo.rs`) independently implement redundant formatting helpers (`format_bytes`, `format_duration`, `format_number`, `format_count`). They have slight arithmetic and decimal precision discrepancies and produce duplicated code.

## Requirements
- Introduce a deep `output` (or `ui`) module in `wt-cli` that encapsulates:
  - Byte unit scaling and formatting (`HumanBytes`).
  - Duration representation (`HumanDuration`).
  - Digit grouping for file and object counts (`HumanCount`).
  - Aligned terminal table rendering.
- Replace duplicate formatting helper functions in `create.rs`, `clean.rs`, `list.rs`, and `demo.rs` with calls to the presentation module.
- Add unit tests for the presentation module covering zero values, unit boundaries, and decimal precision.

## Files Owned
- `crates/wt-cli/src/output.rs`
- `crates/wt-cli/src/main.rs`
- `crates/wt-cli/src/commands/create.rs`
- `crates/wt-cli/src/commands/clean.rs`
- `crates/wt-cli/src/commands/list.rs`
- `crates/wt-cli/src/commands/demo.rs`
- `crates/wt-cli/tests/output.rs`
