# 03: Copy backends

**What to build:** All three copy strategies behind the backend trait from
ticket 01: whole-directory `clonefile` on APFS, reflink where Linux
filesystems support it, hardlink elsewhere. Backend selection detects what
the filesystem supports and picks the fastest safe option. Pure library code
against the trait, tested against real temp directories; no CLI involvement.
Hardlink mode ships disabled by default until ticket 07 makes it safe.

**Blocked by:** 01 (skeleton, contracts, test rig).

**Status:** ready-for-agent

- [ ] Clonefile backend clones a thousand-file directory in well under a second
- [ ] Reflink backend passes the same test on a supporting Linux filesystem
- [ ] Hardlink backend exists but reports itself as unsafe-pending
- [ ] Selection logic picks the best available backend per filesystem
- [ ] Unit tests per backend plus one integration test through the trait
