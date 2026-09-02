# 03: Simplify Snapshot Index and Unify Store Codecs

Status: ready-for-agent

Blocked by: `02-store-legacy-refcount-and-gc-mode-purge.md`

## Problem

Snapshot metadata persistence in `SelectionIndex` relies on multi-file WAL journal appending (`journal.tsv`), journal compaction, and `MetadataLock` compaction locking. Additionally, file stat and byte parsing utilities are duplicated across modules or hand-rolled where standard library functions already exist (e.g. byte slice decoding in `bulkwalk.rs`, character shifting hex decoder in `ContentId`, and manual directory ancestor searches).

## Work

1. Simplify `SelectionIndex` and snapshot metadata persistence to write an atomic TSV manifest via temporary file rename, removing multi-file WAL journal appending (`journal.tsv`), journal compaction, and `MetadataLock` compaction locking.
2. Unify TSV timestamp serialization and parsing between `ValidationCache` and `VerifiedLedger` using shared `format_mtime` and `parse_mtime` helpers.
3. Replace hand-rolled little-endian byte slice decoding functions (`read_u32`, `read_i32`, `read_u64`, `read_i64`) in `bulkwalk.rs` with standard library `u32::from_le_bytes` conversions.
4. Replace custom 21-line character shifting hex decoder in `ContentId::from_hex` with idiomatic slice parsing.
5. Replace hand-rolled directory ancestor search in `find_lockfile` with standard `Path::ancestors()`.
6. Consolidate duplicate recursive file traversal functions in `scrub.rs` and `snapshot/tree.rs` into `fsutil::collect_dir_rels`.

## Files Owned

- `crates/wt-store/src/snapindex.rs`
- `crates/wt-store/src/validation.rs`
- `crates/wt-store/src/verified.rs`
- `crates/wt-store/src/bulkwalk.rs`
- `crates/wt-store/src/content_id.rs`
- `crates/wt-store/src/fsutil.rs`
- `crates/wt-store/src/scrub.rs`
- `crates/wt-store/src/snapshot/tree.rs`

## Done When

- [ ] Snapshot index updates write atomically via single-file tempfile-rename without WAL journal complexity.
- [ ] Stat caches and verified ledgers share unified TSV timestamp codec functions.
- [ ] Hand-rolled byte decoders in `bulkwalk.rs` are replaced with standard library methods.
- [ ] `find_lockfile` uses `Path::ancestors()`.
- [ ] Snapshot projection fast path and incremental rebuilds pass all unit and integration tests.
