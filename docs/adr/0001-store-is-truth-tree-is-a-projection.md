# Store is truth, the on-disk tree is a projection

We considered replacing the filesystem with a database-backed virtual
filesystem, but every interception mechanism on macOS (FUSE, kernel
extensions) adds per-operation cost and install friction, likely making
things slower. Instead, a userspace store holds each unique file content once
(the source of truth), and the visible project tree is a disposable copy kept
in sync with it. Expensive operations are avoided at the source rather than
made faster underneath.

## Considered options

- Virtual filesystem (FUSE/NFS backed by a database): rejected, adds overhead
  and is hard to install on macOS.
- Full bypass where nothing touches disk: rejected, third-party build tools
  require real files.

## Consequences

- Editors and build tools keep working on real files; only agents may talk to
  the store directly.
- A watcher/sync layer must keep the tree coherent with the store when either
  side changes.
