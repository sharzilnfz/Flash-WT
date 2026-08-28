# 03: Human-Centric Verbs and Actionable Receipts (`wt new`, `wt clean`)

**What to build:**
Introduce `wt new` as the primary creation verb (aliasing `wt create`) and `wt clean` for worktree removal and reclamation (aliasing `wt remove` + `wt sweep`). Replace raw text log lines with structured terminal receipts that display clear checkmarks, duration, hydrated file counts, storage metrics, and explicit next-action hints (e.g. `cd ../<worktree-dir>`).

**Blocked by:**
02: Worktree Discovery and Disk Accounting (`wt list`)

**Status:**
ready-for-human

- [x] Add `wt new` and `wt clean` to CLI command hierarchy with full argument parity to `create` and `remove`.
- [x] Implement receipt formatter emitting structured glyphs (`✓`), humanized file counts, elapsed time, and `cd` action hints on `wt new`.
- [x] Make `wt clean <name>` remove the specified worktree and automatically invoke storage reclamation for unreferenced objects.
- [x] Consolidate `wt isolate` as a transparent alias to `wt scratch`.
- [x] Add integration tests in `crates/wt-cli/tests/` verifying `wt new` and `wt clean` execution, receipts, and exit status.
