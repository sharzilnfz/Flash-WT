# Codebase memory

This repo is indexed in the codebase-memory MCP under the project name
`instant-worktrees`. Use it in every session:

## Before implementing a ticket

- Call `index_repository` on this repo (mode `moderate`) if you just pulled
  new commits or branches; the index does not auto-refresh mid-session.
- Use `get_architecture` and `search_graph` to orient instead of re-reading
  files grep-first.
- Check `check_index_coverage` before claiming anything about files you did
  not open yourself.

## After landing work

- Re-run `index_repository` so the next parallel agent sees your symbols.
- If you changed a public trait (copy-backend, store), note it in your final
  report; downstream tickets depend on those signatures.
