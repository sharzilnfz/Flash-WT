# 05: Copy Engine, Backend Safety & Materializer Simplification

Status: ready-for-agent

Blocked by: None (can start immediately).

## Problem

`flashwt-copy` contains obsolete backend safety contracts (`Safety::UnsafePending`, `Error::UnsafeBackend`) despite all shipped backends being verified safe. It also contains redundant delegator methods on `CopyEngine`, dead constructor methods on `Materializer`, zero-sized wrapper structs, manual 128 KiB buffer copy loops, and duplicate directory ancestor searching.

## Work

1. Delete `Safety::UnsafePending` and `Error::UnsafeBackend` contract from `lib.rs` and related staged copy tests since all shipped backends are safe.
2. Remove redundant file materialization delegators (`materialize_file`, `materialize_files`) from `CopyEngine`.
3. Remove dead methods on `Materializer` (`custom`, `for_directories`, `backend`, `new`) and `BatchPlacementReceipt::new`.
4. Replace zero-sized wrapper structs `ReflinkOut` and `CopyFileRangeOut` with direct function calls.
5. Replace manual 128 KiB buffer copy loop in `sys::buffered_copy_file` with `std::io::copy`.
6. Replace hand-rolled parent directory walking in `find_existing_ancestor` with `Path::ancestors()`.
7. Deduplicate hardlink creation and read-only mode stripping into a shared `hardlink_readonly` helper.

## Files Owned

- `crates/flashwt-copy/src/lib.rs`
- `crates/flashwt-copy/src/materialize.rs`
- `crates/flashwt-copy/src/sys.rs`
- `crates/flashwt-copy/tests/copy.rs`

## Done When

- [ ] `Safety::UnsafePending` and associated validation boilerplate are removed.
- [ ] Dead delegators in `CopyEngine` and `Materializer` are eliminated.
- [ ] Manual copy loop in `sys.rs` is replaced with `std::io::copy`.
- [ ] `find_existing_ancestor` uses `Path::ancestors()`.
- [ ] Copy engine and materialization tests pass with zero regressions.
