# 01: Skeleton, contracts, and the end-to-end test rig

**What to build:** The repo scaffold with crate boundaries agreed upfront, so
parallel agents can build against frozen interfaces without touching each
other's files. Defines the two central traits: the copy-backend trait
(clonefile, reflink, hardlink behind one interface) and the store trait
(hash-addressed put/get, reference counting). Also fixes the CLI argument
shape for `wt create` and builds the founding end-to-end test rig: tests run
the binary against real temporary git repositories containing generated fake-
heavy directories, asserting only through the CLI boundary.

**Blocked by:** None (can start immediately).

**Status:** ready-for-agent

- [ ] Crate layout compiles with both traits defined and stubbed
- [ ] `wt create --help` works and argument shape is documented in the ticket thread
- [ ] Test rig creates a temp repo with thousands of small fake-heavy files
- [ ] One smoke test passes through the CLI boundary end to end
- [ ] Traits have doc comments precise enough that tickets 02-04 can code against them without asking questions
