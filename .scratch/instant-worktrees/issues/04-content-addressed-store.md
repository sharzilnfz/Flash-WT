# 04: Content-addressed store

**What to build:** The store behind its trait from ticket 01: every unique
file content kept once, addressed by hash, with reference counting for later
garbage collection. Reads verify hashes so silent corruption is detectable.
Pure library code against the trait; no CLI involvement. This is the
source-of-truth layer from ADR-0001.

**Blocked by:** 01 (skeleton, contracts, test rig).

**Status:** ready-for-agent

- [x] Put/get round-trips byte-identical content
- [x] Identical content stored twice occupies disk once
- [x] Reference counts increment and decrement correctly
- [x] Hash mismatch on read returns an error rather than bad data
- [x] Store tests run fast without touching real project trees

## Comments

### What was built

`DiskStore` in `crates/wt-store/src/disk.rs`, replacing `StubStore`
(the trait itself is untouched). Layout inside the root it owns:
blobs at `objects/<2 hex>/<62 hex>` named by SHA-256 digest; decimal
refcount files at `refs/<64 hex>`. Blobs are written to a temp file in
the same directory and renamed into place, so no half-written blob
ever sits at its final address.

Contract notes for tickets 05/06:

- `put` never touches refcounts; content put but unreferenced counts 0.
- Releasing to zero keeps the blob on disk — deletion belongs to GC
  (ticket 06), which can treat refcount-0 entries as collectible.
- All state is on disk, so separate handles on one root already see
  each other's writes; cross-process locking remains deferred to 06.
- New dependency: `sha2` (hash function stays an implementation detail
  of this crate, as the trait doc requires).

Twelve integration tests in `crates/wt-store/tests/store.rs` cover all
five acceptance items (corruption simulated by clobbering every file
under a temp root, so tests need no knowledge of the internal layout).
Full suite runs in well under a second; workspace tests stay green;
clippy clean.
