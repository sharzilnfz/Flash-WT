# Codebase memory

This repository is indexed in the codebase-memory MCP under the project name
`instant-worktrees`. All discovery, symbol lookup, and code reading prioritize
codebase-memory MCP tools.

## Retrieval protocol

Follow this sequence for all code and document inspection:

1. **Symbol exploration and reading**.
   Call `search_graph` with your query to obtain the canonical `qualified_name`.
   Pass the exact `qualified_name` to `get_code_snippet` to retrieve the complete source block.
2. **Text, markdown, and configuration search**.
   Call `search_code` with `mode="full"` to locate and read literal strings, markdown sections, or scripts.
3. **Architecture and flow tracing**.
   Call `get_architecture` for high-level structure.
   Call `trace_path` to navigate caller and callee chains across crates.
4. **Coverage verification**.
   Call `check_index_coverage` before making negative or exhaustive claims about code structure.
5. **Non-AST and dirty-state boundary**.
   Use filesystem read tools only for files outside the index (dotfiles, lockfiles, CI configs) or verifying unindexed in-flight edits.

## Impact analysis and performance queries

- Call `detect_changes` with `direction="inbound"` to map the blast radius and transitive callers of uncommitted or branch changes before opening pull requests.
- Call `query_graph` to inspect complexity hotspots and nested loop depth (`transitive_loop_depth`, `linear_scan_in_loop`, `alloc_in_loop`).

## Index synchronization

- Git hooks in `.git/hooks/post-checkout` and `.git/hooks/post-merge` automatically trigger background re-indexing with persistent snapshots upon branch switches or pulls.
- Install or refresh the hooks across checkouts using `scripts/setup/install-cbm-hooks.sh`.
- Persistent graph artifacts live at `.codebase-memory/graph.db.zst` for zero-cold-start initialization.
- Call `index_repository` with `persistence=true` after landing cross-crate symbol changes.
