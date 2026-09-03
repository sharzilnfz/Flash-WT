# Context

A tool that makes agentic coding fast on any OS by ending small-file churn.
Free, open source, installs in one command, changes nothing about how humans work.

## Glossary

**Store**
The single place where every unique file content is kept exactly once. The
source of truth for code. Inspired by git's object database.

**Tree (or projection)**
The normal folder of files on disk that editors, build tools, and humans see.
Not the source of truth. Rebuildable from the store at any time. Explicit
hydration (`flashwt hydrate` or `flashwt new`) is the sole synchronization mechanism.
There is no background sync daemon, filesystem watcher, or automatic
bidirectional tree-store synchronization.

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
Both humans and agents edit normal files in the tree. Neither humans nor agents
bypass the filesystem to mutate the store directly. Both trigger explicit CLI
commands (`flashwt new`, `flashwt hydrate`) when they need to populate or update heavy
directories.

**Mirror**
The store-local GC root for one hydrated worktree: a small TSV file under
`<store>/worktrees/` listing every blob (or snapshot) the worktree hydrates
from, written atomically once per successful create. Written beside legacy
refcounts until the explicit cutover (ADR-0004).

**Snapshot**
A rebuildable whole-directory image in the store:
`<store>/snapshots/<manifest-hash>/` holds a canonical manifest plus a tree
of files hardlinked to object blobs. On a hit, hydrating one heavy
directory is a single recursive APFS clone of that tree; a miss rebuilds
it from blobs first. Cache only. The blobs remain the truth (ADR-0005).

**Grace period**
How long the store waits before collecting unreferenced blobs, snapshots, or
whole dead-worktree mirrors. Defaults to 15 minutes, overridable with
`FLASHWT_GC_GRACE`. Deletion needs both "no live root" and "older than the grace
period", so a kill at any point can only leak reclaimable cache data, never
collect live data.
