# 07: Hardlink safety

**What to build:** On filesystems without clone support, hydration falls back
to hardlinks. Package managers sometimes rewrite files in place, which would
corrupt every project sharing those links. This ticket applies the pnpm
lessons: detect in-place rewrites and copy-on-shared-write so shared content
stays safe. Until this lands, hardlink mode stays disabled and unsupported
filesystems get a clear message.

**Blocked by:** 05 (wire hydration through store).

**Status:** ready-for-agent

- [x] In-place rewrite of a hardlinked file affects only the writing project
- [x] Fallback activates automatically on filesystems without clone support
- [x] Unsupported filesystems show a clear message while mode is disabled
- [x] Torture test simulates package-manager rewrite patterns across two worktrees

## Comments

### What was built (ticket 07)

Copy-on-shared-write via read-only shared inodes. Hydration now links
store objects into worktrees instead of rewriting bytes: `DiskStore::
link_out` verifies the blob hash first, hard-links it to the tree,
and strips write bits from the shared inode (exec bits preserved). An
in-place rewrite therefore fails loudly with EACCES instead of
poisoning every sibling tree and the store; replacement-style writes
(rename-over, unlink plus recreate) break the share and get a private
writable copy. Inherent trade-off: permissions live on the inode, so
the store object becomes read-only too — which is exactly what the
store wants as source of truth.

`HardlinkBackend` earned `Safety::Safe` with the same guard and joined
`candidates()` directly ahead of deep copy, so selection falls back
clone → hardlink → byte copy automatically. `supports()` rejects
FAT/exFAT-family and network filesystems via statfs (and read-only
mounts; macOS `/` is the sealed system volume and correctly reports
unsupported).

CLI messages: when linking is refused by the filesystem it prints
"hardlink unavailable on this filesystem: ..."; `FLASHWT_NO_HARDLINK=1`
disables linking outright with its own line. Byte-copy fallbacks stay
private and writable.

Tests: unit tests on the backend trait seam (shared readonly inodes,
blocked rewrites, rename-over isolation), store tests for `link_out`
(corrupt/unknown content never lands), and `crates/flashwt-cli/tests/
hardlink_safety.rs` through the CLI seam: shared-inode proof across
two worktrees, poison attempt with a third worktree hydrating clean
bytes afterwards, the four-pattern torture suite, and the disabled-
mode message. Ticket 05's corruption test now re-enables write bits
on the blob before flipping bytes, since linked-out blobs are
read-only by design.
