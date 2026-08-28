# 02: Ref-aware symbolic branch tracking and base movement diagnostics

**What to build:** Record symbolic parent branch references in store mirror metadata when creating worktrees with `--base <ref>`. During status inspection and command execution, verify whether the base branch reference has moved relative to the initial worktree state. Surface diagnostic warnings in `--json` payloads and human command output so developers stacking branches detect parent movements immediately.

**Blocked by:** 01: Versioned JSON output envelope across CLI commands.

**Status:** ready-for-agent

- [x] `wt create <name> --base <ref>` records the symbolic name of the parent base branch in worktree mirror metadata.
- [x] Base branch head commit resolution tracks whether the parent branch ref has changed since worktree initialization.
- [x] Diagnostic warnings are emitted in both human output and `--json` envelope diagnostics when parent branch movement is detected.
- [x] Integration tests verify symbolic ref preservation, detection of upstream rebase shifts, and diagnostic envelope formatting.
