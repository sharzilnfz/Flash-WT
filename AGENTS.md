# AGENTS.md

## Agent skills

### Issue tracker

Issues live as local markdown under `.scratch/<feature>/`. See `docs/agents/issue-tracker.md`.

### Triage labels

Default five-role vocabulary (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`), recorded as a `Status:` line. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context layout: `CONTEXT.md` glossary plus `docs/adr/`. See `docs/agents/domain.md`.

### Codebase memory

The repo is indexed in the codebase-memory MCP as project `instant-worktrees`.
Refresh the index after pulling or landing code. See `docs/agents/codebase-memory.md`.
