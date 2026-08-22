# Runbook: automated fleet build

One script drives the whole MVP. It launches agents in Herdr panes, waits for
each to finish, gates on tests and your review between phases, and keeps every
agent pointed at codebase-memory.

## One-time setup (already done)

- Repo indexed in codebase-memory as `instant-worktrees`
- `AGENTS.md` tells every agent how to use the index
- Nine tickets under `.scratch/instant-worktrees/issues/`

## How to run

1. Open a terminal, start Herdr, open this repo.
2. In any Herdr pane, at the repo root:

   ```
   ./orchestrate.sh
   ```

3. The script pauses between phases. Each pause asks you to review the work
   and confirm tests are green before it spends more agent time. Nothing runs
   away unattended.

## Phases

| Phase | Tickets | Agents in parallel |
|-------|---------|--------------------|
| 1     | 01 skeleton + contracts + test rig | 1 |
| 2     | 02 CLI, 03 backends, 04 store      | 3, isolated git worktrees under `.fleet/` |
| 3     | 05 merge + wire end to end         | 1 |
| 4     | 06 GC, 07 hardlink safety, 08 benchmarks, 09 distribution | 4 |

## Knobs

- `AGENT_KIND=codex ./orchestrate.sh` to use a different agent binary.
  Run bare `herdr agent` inside Herdr to list installed kinds.
- `PHASE=2 ./orchestrate.sh` to rerun one phase after fixing something.

## Model pinning

Project `opencode.json` sets every OpenCode session in this repo to
Ox Alpha Free on OpenCode Zen with high reasoning effort, applied as a
model-level request setting so no per-session choice can drop it. If the
catalog slug differs on your install, open the TUI, press `/models`, find the
exact entry, and update the two occurrences of `ox-alpha-free` in
`opencode.json`. Verify after the fleet starts: ask any worker which model it
is running before letting phase 1 finish.

## After phase 4

1. Merge fleet branches into main in numeric order.
2. Re-index: any agent session, or ask for `index_repository` on project
   `instant-worktrees`.
3. Clean up: `git worktree remove .fleet/<n>` for each, delete fleet branches.

## If an agent stalls

The script prints which agent needs attention. Inspect with:

```
herdr agent read <name> --source recent-unwrapped --lines 120
herdr agent get <name>
```

A `blocked` state means the agent hit an approval prompt; answer it via
`herdr agent prompt <name>` or the pane directly.

## Accuracy rules baked in

- Every agent gets the ticket path plus instructions to follow AGENTS.md,
  the ADRs, and the glossary; no free-form goals.
- Tests gate each phase; a red suite stops the pipeline before the next
  phase spends tokens on a broken base.
- Contracts are frozen in ticket 01 precisely so parallel agents cannot
  drift into merge conflicts.
