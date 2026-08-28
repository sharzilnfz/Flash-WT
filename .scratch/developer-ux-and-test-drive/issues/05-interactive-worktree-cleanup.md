# 05: Interactive Multi-Select Worktree Cleanup

**What to build:**
Provide an interactive terminal selection prompt when `wt clean` is invoked without arguments in a TTY. It scans active worktrees, detects merged branches against HEAD, displays an interactive checklist with pre-selected merged candidates, deletes confirmed worktrees, triggers storage reclamation, and prints a receipt of reclaimed disk space.

**Blocked by:**
02: Worktree Discovery and Disk Accounting (`wt list`)
03: Human-Centric Verbs and Actionable Receipts (`wt new`, `wt clean`)

**Status:**
ready-for-human

- [x] Implement git branch merged check (`git branch --merged`) to identify worktrees safe for removal.
- [x] Implement interactive multi-select prompt for terminal (TTY) sessions with pre-selected merged worktrees.
- [x] Support `--all` and `--force` flags for automated non-interactive batch cleanup.
- [x] Execute batch worktree removal and sweep unreferenced objects in a single transaction.
- [x] Render formatted summary of removed worktrees and reclaimed disk space.
- [x] Add integration tests in `crates/wt-cli/tests/` asserting interactive and non-interactive batch cleanup flows.
