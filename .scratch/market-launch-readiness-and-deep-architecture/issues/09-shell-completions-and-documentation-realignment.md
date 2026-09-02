# 09: Shell Autocompletions and Documentation Realignment

Status: ready-for-agent

Blocked by: `06-cli-dead-code-clap-aliases-and-dep-pruning.md`

## Problem

Developers evaluating and adopting `wt` expect shell completion support for standard interactive shells (Bash, Zsh, Fish, Elvish, PowerShell). Currently, no autocompletion machinery exists, requiring manual typing of all branch names, subcommands, and flags. Furthermore, `README.md` documents legacy manual snapshot flags (`export WT_SNAPSHOTS=1`) as a necessary step, while failing to highlight that whole-directory snapshots and diff rebuilds are enabled automatically on macOS APFS, and omits the modern primary 3-verb workflow (`wt new`, `wt clean`, `wt list`, `wt demo`).

## Work

1. Add `clap_complete` to `crates/wt-cli/Cargo.toml`.
2. Implement `wt completions <shell>` command supporting `bash`, `elvish`, `fish`, `powershell`, and `zsh`.
3. Update `install.sh` to detect active shell directories and install completion scripts automatically.
4. Update `README.md` to showcase the 3-verb workflow (`wt new`, `wt clean`, `wt list`) and zero-setup test drive (`wt demo`).
5. Update environment variable reference tables in `README.md` to accurately indicate that `WT_SNAPSHOTS` defaults to enabled on macOS APFS.
6. Add integration tests verifying completion generation across all supported shells.

## Files Owned

- `crates/wt-cli/Cargo.toml`
- `crates/wt-cli/src/cli.rs`
- `crates/wt-cli/src/commands/mod.rs`
- `crates/wt-cli/src/commands/completions.rs`
- `install.sh`
- `README.md`
- `crates/wt-cli/tests/completions.rs`

## Done When

- [ ] `wt completions <shell>` generates valid completion scripts for Bash, Zsh, Fish, Elvish, and PowerShell.
- [ ] `install.sh` detects active shell configurations and places completions in appropriate directories.
- [ ] `README.md` highlights the modern 3-verb workflow and APFS snapshot defaults.
- [ ] Integration tests verify completion output and exit status for all shells.
