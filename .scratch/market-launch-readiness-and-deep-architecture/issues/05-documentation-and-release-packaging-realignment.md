# Issue 05: Documentation and Release Packaging Realignment

Status: ready-for-agent

## Context
`README.md` currently documents legacy manual snapshot configuration (`export WT_SNAPSHOTS=1`) as a necessary step, while failing to highlight that whole-directory snapshots and diff rebuilds are enabled automatically on macOS APFS. It also omits modern primary workflow verbs (`wt new`, `wt clean`, `wt list`, `wt demo`). Furthermore, Homebrew formula templates and release scripts should package generated shell completions.

## Requirements
- Update `README.md` to showcase the 3-verb workflow (`wt new`, `wt clean`, `wt list`) and test drive (`wt demo`).
- Update environment variable reference tables in `README.md` to accurately indicate that `WT_SNAPSHOTS` defaults to enabled on macOS APFS.
- Update `Formula/wt.rb` and `scripts/gen-formula.sh` to install shell completions for Homebrew users.
- Verify that `install.sh` and release documentation provide clear, consistent instructions for all platforms.

## Files Owned
- `README.md`
- `Formula/wt.rb`
- `scripts/gen-formula.sh`
- `install.sh`
