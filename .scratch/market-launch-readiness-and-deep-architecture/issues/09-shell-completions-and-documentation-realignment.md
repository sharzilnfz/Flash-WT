# 09: Shell Autocompletions and Documentation Realignment

Status: ready-for-agent

Blocked by: `06-cli-dead-code-clap-aliases-and-dep-pruning.md`

## Problem

Developers evaluating and adopting `flashwt` expect shell completion support for standard interactive shells (Bash, Zsh, Fish, Elvish, PowerShell). Currently, no autocompletion machinery exists, requiring manual typing of all branch names, subcommands, and flags. Furthermore, `README.md` documents legacy manual snapshot flags (`export FLASHWT_SNAPSHOTS=1`) as a necessary step, while failing to highlight that whole-directory snapshots and diff rebuilds are enabled automatically on macOS APFS, and omits the modern primary 3-verb workflow (`flashwt new`, `flashwt clean`, `flashwt list`, `flashwt demo`).

## Work

1. Add `clap_complete` to `crates/flashwt-cli/Cargo.toml`.
2. Implement `flashwt completions <shell>` command supporting `bash`, `elvish`, `fish`, `powershell`, and `zsh`.
3. Update `install.sh` to detect active shell directories and install completion scripts automatically.
4. Update `README.md` to showcase the 3-verb workflow (`flashwt new`, `flashwt clean`, `flashwt list`) and zero-setup test drive (`flashwt demo`).
5. Update environment variable reference tables in `README.md` to accurately indicate that `FLASHWT_SNAPSHOTS` defaults to enabled on macOS APFS.
6. Add integration tests verifying completion generation across all supported shells.

## Files Owned

- `crates/flashwt-cli/Cargo.toml`
- `crates/flashwt-cli/src/cli.rs`
- `crates/flashwt-cli/src/commands/mod.rs`
- `crates/flashwt-cli/src/commands/completions.rs`
- `install.sh`
- `README.md`
- `crates/flashwt-cli/tests/completions.rs`

## Done When

- [ ] `flashwt completions <shell>` generates valid completion scripts for Bash, Zsh, Fish, Elvish, and PowerShell.
- [ ] `install.sh` detects active shell configurations and places completions in appropriate directories.
- [ ] `README.md` highlights the modern 3-verb workflow and APFS snapshot defaults.
- [ ] Integration tests verify completion output and exit status for all shells.
