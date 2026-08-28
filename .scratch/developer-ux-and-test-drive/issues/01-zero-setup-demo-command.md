# 01: Zero-Setup Self-Test Drive (`wt demo`)

**What to build:**
A self-contained `wt demo` (and `wt test-drive`) command that executes an automated end-to-end demonstration without requiring pre-existing repositories. In under five seconds, it builds a synthetic 10,000-file project fixture in a temporary directory, measures baseline copy speed, hydrates a worktree with copy-on-write sharing, verifies mutation isolation (ensuring store files are unmodified when worktree files change), performs cleanup, and prints a visual comparison scorecard in the terminal.

**Blocked by:**
None (can start immediately).

**Status:**
ready-for-human

- [x] Add `wt demo` and `wt test-drive` subcommands to CLI parser with optional `--json` support.
- [x] Implement synthetic fixture generator creating 10,000 files across realistic package hierarchies in a temp directory.
- [x] Implement baseline copy measurement and copy-on-write hydration execution within the sandbox.
- [x] Implement copy-on-write mutation isolation check validating that modifying worktree files does not alter store blobs.
- [x] Render formatted terminal scorecard with progress steps, timing bars, byte metrics, and cleanup summary.
- [x] Add integration tests in `crates/wt-cli/tests/` asserting `wt demo` executes successfully and produces valid JSON/terminal outputs.
