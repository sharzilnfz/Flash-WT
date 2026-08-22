# 08: Public benchmark suite

**What to build:** A script that reproduces the macOS-versus-Linux scenario
numbers: plain `git worktree add` plus fresh dependency install versus our
tool, on identical project fixtures. Produces a table of results suitable for
the README and launch post, so claims are verifiable by anyone.

**Blocked by:** 05 (wire hydration through store).

**Status:** ready-for-agent

- [ ] One command runs the full comparison and prints a results table
- [ ] Fixtures match the published benchmarks' shape (thousands of small files)
- [ ] Results include cold and warm worktree creation times
- [ ] Suite runs unattended on macOS and Linux CI
