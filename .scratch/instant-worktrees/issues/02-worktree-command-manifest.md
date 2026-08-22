# 02: Worktree command and manifest

**What to build:** `wt create feature-x` wraps `git worktree add` and then
hydrates heavy directories into the new worktree by copying from the source
checkout. A `.wtinclude` manifest (gitignore syntax) lists which directories
count as heavy; sensible defaults apply when it is absent. The command prints
what it linked and from where, because trust requires honesty about disk
changes. This ticket touches only the CLI layer; hydration goes through the
copy-backend trait from ticket 01, using whichever backend is available.

**Blocked by:** 01 (skeleton, contracts, test rig).

**Status:** ready-for-agent

- [ ] `wt create` produces a working git worktree
- [ ] Directories matched by `.wtinclude` exist in the worktree, byte-identical to source
- [ ] Absent manifest falls back to documented defaults plus a suggested starter file
- [ ] Output lists every hydrated directory and its source
- [ ] End-to-end tests cover all of the above through the CLI seam
