# Issue 01: Shell Completions and CLI Polish

Status: ready-for-agent

## Context
Developers evaluating and adopting `wt` expect shell completion support for standard interactive shells (Bash, Zsh, Fish). Currently, no autocompletion machinery exists, requiring manual typing of all branch names, subcommands, and flags.

## Requirements
- Add `clap_complete` to `wt-cli` dependencies.
- Introduce `wt completions <shell>` command supporting `bash`, `elvish`, `fish`, `powershell`, and `zsh`.
- Update `install.sh` to detect active shell directories and optionally install completion scripts automatically.
- Add integration tests verifying completion generation across all supported shells.

## Files Owned
- `crates/wt-cli/Cargo.toml`
- `crates/wt-cli/src/cli.rs`
- `crates/wt-cli/src/commands/mod.rs`
- `crates/wt-cli/src/commands/completions.rs`
- `install.sh`
- `crates/wt-cli/tests/completions.rs`
