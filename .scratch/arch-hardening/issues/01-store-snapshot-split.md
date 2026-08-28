# 01: split snapshot.rs into a module directory and dedupe the publish protocol

Status: ready-for-agent
Owner branch: `arch/store-refactor`
Owns: `crates/wt-store/src/snapshot.rs` and everything it becomes, plus
re-exports in `crates/wt-store/src/lib.rs`.

## Problem

`snapshot.rs` bundles three unrelated concepts: a TSV codec (Manifest,
SnapshotEntry, EntryKind, validate_rel), a distributed publish protocol
(staging dirs, atomic rename, winner-collision handling), and filesystem tree
construction (build_tree, place_entry, clone_dir_recursive,
paranoid_verify_tree). Full and incremental publish copy-paste ~60 lines of
staging/rename/collision logic each; drift between the two routes produces
snapshots valid on one path and debris on the other.

## Work

1. Convert to a directory module, preserving the public surface via re-exports:
   - `snapshot/mod.rs`: paths (`snapshot_path`, `snapshot_tree_path`),
     `read_published`, re-exports.
   - `snapshot/manifest.rs`: Manifest, SnapshotEntry, EntryKind, validate_rel.
   - `snapshot/publish.rs`: PublishOutcome, BuildError, SnapshotBuildTiming,
     staging/rename protocol.
   - `snapshot/tree.rs`: build_tree, place_entry, clone_dir_recursive,
     paranoid_verify_tree, collect_rels.
2. Extract one private helper on DiskStore, e.g.
   `stage_and_publish(manifest, seed, body)` where Seed is FreshTree or
   CloneFrom(ContentId). It owns tmp setup, metadata writes, rename with
   EEXIST/ENOTEMPTY winner validation, loser cleanup. Both public publish
   functions become thin wrappers supplying their closure.
3. Flatten the double `Result<Result<PublishOutcome, BuildError>, Error>`:
   add `BuildError::Io(crate::Error)` so both methods return
   `Result<PublishOutcome, BuildError>`. Update the ~3 wt-cli call sites is
   ticket 03's job; here just keep lib compiling if possible, otherwise note
   the break in the commit message so integration resolves it.
4. Replace the trailing `Option<&mut SnapshotBuildTiming>` params with a
   returned receipt (`PublishReceipt { outcome, timing }`). Remove the four
   `&mut u64` accumulator params from place_entry by gathering into a local
   timing struct passed by `&mut`.
5. Replace the four `expect(...)` panics inside placement/verify paths
   (snapshot.rs lines ~451, 470, 1039, 1056) with typed BuildError returns
   carrying the entry relpath.

## Constraints

- Keep Display strings for errors byte-compatible where tests assert
  substrings ("hash verification", "paranoid check" in snapshot/tests.rs).
- Do not touch disk.rs, mirror.rs, gc.rs, verified.rs, validation.rs,
  snapindex.rs, snapdiff.rs (ticket 02 owns those). If fsync hooks are wanted
  at metadata-write sites, leave them; ticket 02 adds them after merge.

## Done when

- `cargo test -p wt-store` passes; `cargo clippy -p wt-store --all-targets` clean.
- snapshot.rs no longer exists as a single file; all former public items still
  reachable from `wt_store`.

## Comments

Completed on `arch/hardening-and-simplify` via wave 2 of the orchestrated build. See git log for the implementing commits.
