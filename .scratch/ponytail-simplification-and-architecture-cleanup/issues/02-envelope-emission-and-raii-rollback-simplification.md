# Ticket 02: Envelope Emission & RAII Rollback Simplification

Status: ready-for-human

## Description

Command handlers across `crates/flashwt-cli/src/commands/mod.rs` duplicate 7-line JSON envelope serialization, error mapping, and stdout printing blocks across 10 match branches. In `commands/clean.rs`, empty receipts are constructed via 10-line manual struct initializations across 4 early exit branches. In `commands/create.rs`, error handling manually calls `guard.rollback(); return Err(e)` in 3 match branches, defeating the purpose of RAII drop guards.

## Requirements

1. **Centralized Envelope Emission**:
   - Extract an `emit_json` helper function in `crates/flashwt-cli/src/commands/mod.rs` that accepts command name, data payload, diagnostics, and returns `Result<(), Error>`.
   - Replace repetitive `Envelope::ok`, `serde_json::to_string`, and `println!` blocks across all 10 subcommand branches.

2. **Derive `Default` on Data Envelopes**:
   - Add `#[derive(Default)]` to `CleanData` and `ScratchData` in `crates/flashwt-cli/src/envelope.rs`.
   - Replace manual zeroed field initializations in `commands/clean.rs` and `commands/scratch.rs` with `CleanData::default()` and `ScratchData::default()`.

3. **Idiomatic RAII Rollback in `create.rs`**:
   - Remove manual `guard.rollback()` calls and replace boilerplate `match` statements with standard `?` propagation on `load_patterns`, `open_store`, and `h_engine.hydrate`.
   - Ensure `CreateGuard::drop` handles rollback automatically on error exit.
   - Delete dead `Diagnostic::info` constructor marked with `#[allow(dead_code)]`.

## Verification

- Run `cargo test -p flashwt-cli --test json_output`.
- Verify JSON envelope format, diagnostics, and exit codes remain completely unchanged.
