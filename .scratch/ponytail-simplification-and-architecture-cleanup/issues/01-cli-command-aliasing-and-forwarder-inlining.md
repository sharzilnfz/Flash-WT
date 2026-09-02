# Ticket 01: CLI Command Aliasing & Forwarder Inlining

Status: ready-for-agent

## Description

The CLI subcommand definitions in `crates/wt-cli/src/cli.rs` and `commands/mod.rs` currently define separate enum variants for alias commands (`New`/`Create`, `Isolate`/`Scratch`, `TestDrive`/`Demo`), resulting in duplicated dispatch arms and error mappings. In addition, `hydration_filter.rs` contains a dead `HydrationFilter` struct wrapper whose methods merely forward to free functions, `manifest.rs` is a legacy forwarding module, and `scratch.rs` pulls in the external `sha2` crate just to hash a timestamp and PID for scratch identifiers.

## Requirements

1. **Clap Subcommand Aliases**:
   - Add `#[command(alias = "new")]` to `WtCommand::Create`.
   - Add `#[command(alias = "isolate")]` to `WtCommand::Scratch`.
   - Add `#[command(alias = "test-drive")]` to `WtCommand::Demo`.
   - Remove duplicate enum variants `New`, `Isolate`, `TestDrive` and their corresponding duplicate match arms in `commands/mod.rs`.
   - Preserve exact JSON envelope command name behavior (`"create"`, `"scratch"`, `"demo"`).

2. **Delete Dead `HydrationFilter` Struct & Forwarders**:
   - Delete the `HydrationFilter` struct wrapper in `crates/wt-cli/src/hydration_filter.rs` and its 8 forwarding methods.
   - Delete trivial function aliases `parse`, `matches`, and `collect_matched_directories`. Callers use `load_patterns` and `collect_matches` directly.
   - Delete deprecated forwarding file `crates/wt-cli/src/manifest.rs`. Direct all imports to `hydration_filter`.

3. **Eliminate External `sha2` Crate Dependency in `wt-cli`**:
   - In `crates/wt-cli/src/commands/scratch.rs`, replace SHA-256 ID generation with standard library hex formatting (e.g. `format!("{:08x}", ...)` or stdlib SipHasher).
   - Remove `sha2` from `crates/wt-cli/Cargo.toml`.

4. **Invert Forwarder Functions**:
   - Rename private inner functions `create`, `hydrate`, and `init` directly to `pub fn run` in their respective command modules, removing 1-line wrapper forwarders.

## Verification

- Run `cargo check -p wt-cli` and `cargo test -p wt-cli`.
- Verify `wt create`, `wt new`, `wt scratch`, `wt isolate`, `wt demo`, `wt test-drive` all execute identically.
- Confirm `sha2` is absent from `crates/wt-cli/Cargo.toml`.
