# 02: Worktree command and manifest

**What to build:** `flashwt create feature-x` wraps `git worktree add` and then
hydrates heavy directories into the new worktree by copying from the source
checkout. A `.flashwtinclude` manifest (gitignore syntax) lists which directories
count as heavy; sensible defaults apply when it is absent. The command prints
what it linked and from where, because trust requires honesty about disk
changes. This ticket touches only the CLI layer; hydration goes through the
copy-backend trait from ticket 01, using whichever backend is available.

**Blocked by:** 01 (skeleton, contracts, test rig).

**Status:** ready-for-agent

- [x] `flashwt create` produces a working git worktree
- [x] Directories matched by `.flashwtinclude` exist in the worktree, byte-identical to source
- [x] Absent manifest falls back to documented defaults plus a suggested starter file
- [x] Output lists every hydrated directory and its source
- [x] End-to-end tests cover all of the above through the CLI seam

## Comments

- Implemented on branch `fleet/02-cli-and-manifest`. Hydration goes through
  the frozen `CopyBackend` trait; selection tries safe backends in order and
  falls back to a portable deep-copy backend local to flashwt-cli, so ticket 03's
  fast backends slot in without CLI changes. Manifest matching supports
  gitignore anchoring (`/`), trailing `/`, `*`, and `**`; `!` negation lines
  are ignored rather than misapplied. Absent default-location manifest uses
  documented defaults and writes a starter `.flashwtinclude` to the repo root;
  an explicitly passed missing manifest is an error.
