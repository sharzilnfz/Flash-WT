# Store is truth, the on-disk tree is a projection

We considered replacing the filesystem with a database-backed virtual
filesystem, but every interception mechanism on macOS (FUSE, kernel
extensions) adds per-operation cost and install friction, likely making
things slower. Instead, a userspace store holds each unique file content once
(the source of truth), and the visible project tree is a disposable projection
materialized on demand. Expensive operations are avoided at the source rather
than made faster underneath.

## Considered options

- Virtual filesystem (FUSE/NFS backed by a database): rejected, adds overhead
  and is hard to install on macOS.
- Full bypass where nothing touches disk: rejected, third-party build tools
  require real files.
- Background sync daemons or filesystem watchers: rejected, adds background
  resource drain, state desynchronization bugs, and crash complexity.

## Consequences

- Editors and build tools keep working on real files. Both humans and agents
  interact with normal on-disk files in the projected tree.
- Explicit hydration (`flashwt hydrate` or `flashwt new`) is the sole mechanism for
  materializing files from the store into trees.
- There is no background watcher daemon, automatic synchronization layer, or
  bidirectional tree-store sync. Edits in the tree stay private to that tree.
