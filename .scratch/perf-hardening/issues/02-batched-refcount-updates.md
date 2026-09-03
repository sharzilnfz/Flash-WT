# 02: batch refcount updates behind one lock acquisition

Status: ready-for-agent

Legacy-mode reference claiming calls `DiskStore::add_ref` once per distinct
blob (crates/flashwt-cli/src/hydrate.rs claim_references loop -> crates/flashwt-store/
src/disk.rs). Each call: open refs dir + flock + close, stat ref file, read,
NamedTempFile write, rename. ~8-10 serialized syscalls per blob; 40k blobs ≈
~400k syscalls in the references stage. Removal does the same dance through
release_ref.

## Work

1. Add `DiskStore::add_refs(&[ContentId])` (or BTreeSet) that takes the
   refs-dir flock ONCE, reads each existing count, applies all increments in
   memory, writes counts back (temp+rename each, no fsync), releases the
   lock. Same for `release_refs`. Keep single-id add_ref/release_ref as thin
   wrappers for compatibility with other callers (check gc.rs usage).
2. Switch claim_references and the remove path to the batch APIs.
3. Preserve exact ref-file format and crash semantics: a torn count line is
   already tolerated; increments lost to a crash are covered by grace period
   + mark sweep (ADR-0004). Note in docs that counts are advisory inputs to
   legacy sweep, not truth.

## Done when

- cargo test --workspace green (gc tests exercise refcounts heavily);
  clippy both targets clean; fmt clean.
- One flock acquisition per batch, verifiable by reading the code path.

## Comments

Recon from the halted first attempt (no code was changed): touch points are
`add_ref` / `release_ref` / `lock_refs` / `read_ref_count` / `write_ref_count`
in crates/flashwt-store/src/disk.rs, plus both `claim_*_references` functions in
crates/flashwt-cli/src/hydrate.rs and the removal path.
