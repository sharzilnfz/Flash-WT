# Context

A tool that makes agentic coding fast on any OS by ending small-file churn.
Free, open source, installs in one command, changes nothing about how humans work.

## Glossary

**Store**
The single place where every unique file content is kept exactly once. The
source of truth for code. Inspired by git's object database.

**Tree (or projection)**
The normal folder of files on disk that editors, build tools, and humans see.
Not the source of truth. Rebuildable from the store at any time. Kept coherent
with the store by a watcher/sync layer.

**Materialize**
Producing the tree from the store. Uses links and native copies rather than
writing every file fresh, so it costs near zero.

**Hot paths**
The operations where filesystem slowness actually hurts agentic coding:
dependency installs (`node_modules` churn), git worktrees, and caches. First
targets of the tool.

**Hydrate**
Filling a fresh worktree or checkout with heavy untracked content such as
`node_modules`, build outputs, and caches by copying links from existing
checkouts or the store, instead of downloading or rebuilding. Fast because no
file contents are rewritten, only linked or cloned.

**Human path / agent path**
Humans keep editing real files in the tree as always. Agents may work through
the store directly. Only the agent side changes.
