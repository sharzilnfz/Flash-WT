# 04: Content-addressed store

**What to build:** The store behind its trait from ticket 01: every unique
file content kept once, addressed by hash, with reference counting for later
garbage collection. Reads verify hashes so silent corruption is detectable.
Pure library code against the trait; no CLI involvement. This is the
source-of-truth layer from ADR-0001.

**Blocked by:** 01 (skeleton, contracts, test rig).

**Status:** ready-for-agent

- [ ] Put/get round-trips byte-identical content
- [ ] Identical content stored twice occupies disk once
- [ ] Reference counts increment and decrement correctly
- [ ] Hash mismatch on read returns an error rather than bad data
- [ ] Store tests run fast without touching real project trees
