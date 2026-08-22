# 07: Hardlink safety

**What to build:** On filesystems without clone support, hydration falls back
to hardlinks. Package managers sometimes rewrite files in place, which would
corrupt every project sharing those links. This ticket applies the pnpm
lessons: detect in-place rewrites and copy-on-shared-write so shared content
stays safe. Until this lands, hardlink mode stays disabled and unsupported
filesystems get a clear message.

**Blocked by:** 05 (wire hydration through store).

**Status:** ready-for-agent

- [ ] In-place rewrite of a hardlinked file affects only the writing project
- [ ] Fallback activates automatically on filesystems without clone support
- [ ] Unsupported filesystems show a clear message while mode is disabled
- [ ] Torture test simulates package-manager rewrite patterns across two worktrees
