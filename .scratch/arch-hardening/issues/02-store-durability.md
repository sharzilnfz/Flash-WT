# 02: flashwt-store durability, concurrency, streaming verify

Status: ready-for-agent
Owner branch: `arch/store-refactor` (agent does ticket 01 first, then this)
Owns: `crates/flashwt-store/src/{disk,gc,mirror,verified,validation,snapindex,bulkwalk}.rs`,
new `crates/flashwt-store/src/fsutil.rs`, plus `crates/flashwt-store/src/snapdiff.rs`.
Must run AFTER ticket 01 in the same worktree so snapshot/ metadata-write
fsync hooks land on the post-split code.

## Problem

Every durability claim rests on write-temp-then-rename, but nothing is fsynced
before the rename and no parent dir is fsynced after. On power loss an ext4/
APFS can land the rename with unwritten data: a mirror that is the sole GC
root for a live worktree can appear empty at its final name. Refcount updates
are non-atomic read-modify-write across processes (lost decrement strands a
blob; lost increment can let legacy sweep collect live content). Verification
reads whole blobs into memory just to hash them, so paranoid builds peak at
largest-blob times concurrency and OOMs look like mysterious kills.
snapdiff's unchanged_units is quadratic in entries x directories on v2's hot
path.

## Work

1. New `fsutil.rs`: `durable_write(path, bytes)` (write, sync_all file,
   fsync parent dir) and `durable_write_then_rename(tmp_path, final_path)`.
   Use for: mirror publish (mirror.rs), blob put (disk.rs), gc-mode write
   (gc.rs), snapshot manifest.tsv + .complete metadata (snapshot/publish.rs,
   post-split). Leave selection index, verified ledger, and validation cache
   best-effort but state that explicitly in their doc comments ("rebuildable;
   not crash-durable").
2. Refcount locking: advisory flock via a new `<root>/refs/.lock` around the
   add_ref/release_ref read-modify-write (disk.rs). One private helper;
   uncontended after the mark-sweep cutover.
3. Streaming verification: `DiskStore::verify_digest(id)` streaming through
   BufReader into incremental Sha256, replacing ensure_verified's whole-file
   read; `verify_file(path, expected)` used by paranoid_verify_tree and
   placement. Store::get stays for real byte consumers. Additive API.
4. snapdiff.rs unchanged_units: build HashSet membership for old/new dir rels
   once; compute "all descendants unchanged" bottom-up in one reverse-order
   sweep over merged sorted lists. Existing snapdiff tests pin semantics;
   keep behavior identical, fix complexity.
5. read_published currently swallows IO errors via .ok()? making
   permission-denied indistinguishable from debris in THE shared validity
   check GC also relies on. Return a private enum Miss | Invalid(reason) |
   Io(err) internally; treat Io as invalid-but-log-at-debug, or surface it,
   whichever keeps GC conservative. Document the choice.
6. Kill default_file_mode()'s process-global umask mutation (disk.rs ~359):
   hard-set Permissions::from_mode(0o644) since blobs get explicit permission
   normalization anyway.
7. Hygiene pass on owned files: #[must_use] on escape, worktree_key,
   ContentId::from_hex, Manifest::serialize, SnapshotDiff::{compute,
   changed_count, unchanged_units}; delete mirror.rs's hand-rolled hex() in
   favor of Display; add SAFETY comments to any bare extern decls in
   bulkwalk.rs.

## Constraints

- Grace period / mark-and-sweep semantics unchanged (ADR-0004); these changes
  strengthen its crash-consistency premise.
- Latency cost lands once per create, not per blob; acceptable.
- Do not restructure modules here (ticket 01 owns layout).

## Done when

- `cargo test -p flashwt-store` passes including snapshot/tests.rs tamper tests
  (they force mtime changes past kernel clock ticks; make sure durability
  changes don't break them).
- `cargo clippy -p flashwt-store --all-targets` clean.
- A crash between write and rename can no longer leave mirror or gc-mode
  empty/truncated at their final names (reasoned argument in commit message).

## Comments

Completed on `arch/hardening-and-simplify` via wave 2 of the orchestrated build. See git log for the implementing commits.
