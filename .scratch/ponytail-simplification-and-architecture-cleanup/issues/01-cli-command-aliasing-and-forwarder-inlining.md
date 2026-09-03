# Ticket 01: CLI Command Aliasing & Forwarder Inlining

Status: ready-for-human

## Description

The CLI subcommand definitions in `crates/flashwt-cli/src/cli.rs` and `commands/mod.rs` currently define separate enum variants for alias commands (`New`/`Create`, `Isolate`/`Scratch`, `TestDrive`/`Demo`), resulting in duplicated dispatch arms and error mappings. In addition, `hydration_filter.rs` contains a dead `HydrationFilter` struct wrapper whose methods merely forward to free functions, `manifest.rs` is a legacy forwarding module, and `scratch.rs` pulls in the external `sha2` crate just to hash a timestamp and PID for scratch identifiers.

## Requirements

1. **Clap Subcommand Aliases**:
   - Add `#[command(alias = "new")]` to `FlashwtCommand::Create`.
   - Add `#[command(alias = "isolate")]` to `FlashwtCommand::Scratch`.
   - Add `#[command(alias = "test-drive")]` to `FlashwtCommand::Demo`.
   - Remove duplicate enum variants `New`, `Isolate`, `TestDrive` and their corresponding duplicate match arms in `commands/mod.rs`.
   - Preserve exact JSON envelope command name behavior (`"create"`, `"scratch"`, `"demo"`).

2. **Delete Dead `HydrationFilter` Struct & Forwarders**:
   - Delete the `HydrationFilter` struct wrapper in `crates/flashwt-cli/src/hydration_filter.rs` and its 8 forwarding methods.
   - Delete trivial function aliases `parse`, `matches`, and `collect_matched_directories`. Callers use `load_patterns` and `collect_matches` directly.
   - Delete deprecated forwarding file `crates/flashwt-cli/src/manifest.rs`. Direct all imports to `hydration_filter`.

3. **Eliminate External `sha2` Crate Dependency in `flashwt-cli`**:
   - In `crates/flashwt-cli/src/commands/scratch.rs`, replace SHA-256 ID generation with standard library hex formatting (e.g. `format!("{:08x}", ...)` or stdlib SipHasher).
   - Remove `sha2` from `crates/flashwt-cli/Cargo.toml`.

4. **Invert Forwarder Functions**:
   - Rename private inner functions `create`, `hydrate`, and `init` directly to `pub fn run` in their respective command modules, removing 1-line wrapper forwarders.

## Verification

- Run `cargo check -p flashwt-cli` and `cargo test -p flashwt-cli`.
- Verify `flashwt create`, `flashwt new`, `flashwt scratch`, `flashwt isolate`, `flashwt demo`, `flashwt test-drive` all execute identically.
- Confirm `sha2` is absent from `crates/flashwt-cli/Cargo.toml`.
