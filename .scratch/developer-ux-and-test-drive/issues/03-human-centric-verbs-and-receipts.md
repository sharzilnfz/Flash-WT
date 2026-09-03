# 03: Human-Centric Verbs and Actionable Receipts (`flashwt new`, `flashwt clean`)

**What to build:**
Introduce `flashwt new` as the primary creation verb (aliasing `flashwt create`) and `flashwt clean` for worktree removal and reclamation (aliasing `flashwt remove` + `flashwt sweep`). Replace raw text log lines with structured terminal receipts that display clear checkmarks, duration, hydrated file counts, storage metrics, and explicit next-action hints (e.g. `cd ../<worktree-dir>`).

**Blocked by:**
02: Worktree Discovery and Disk Accounting (`flashwt list`)

**Status:**
ready-for-human

- [x] Add `flashwt new` and `flashwt clean` to CLI command hierarchy with full argument parity to `create` and `remove`.
- [x] Implement receipt formatter emitting structured glyphs (`✓`), humanized file counts, elapsed time, and `cd` action hints on `flashwt new`.
- [x] Make `flashwt clean <name>` remove the specified worktree and automatically invoke storage reclamation for unreferenced objects.
- [x] Consolidate `flashwt isolate` as a transparent alias to `flashwt scratch`.
- [x] Add integration tests in `crates/flashwt-cli/tests/` verifying `flashwt new` and `flashwt clean` execution, receipts, and exit status.
