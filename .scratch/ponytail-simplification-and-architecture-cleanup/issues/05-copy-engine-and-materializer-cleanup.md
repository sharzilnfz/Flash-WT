# Ticket 05: Copy Engine & Materializer Cleanup

Status: ready-for-human

## Description

`crates/wt-copy` contains dead single-file and batch wrapper forwarders on `CopyEngine`, speculative safety gating enums (`Safety::UnsafePending`) that are never triggered by any shipped backend, verbose struct construction branches in `Materializer::select`, and manual buffer allocation loops in byte copying.

## Requirements

1. **Delete Dead `CopyEngine` Forwarders & Batch Receipt**:
   - Delete `CopyEngine::materialize_file`, `CopyEngine::materialize_files`, and `BatchPlacementReceipt` in `crates/wt-copy/src/engine.rs`. Callers construct and drive `Materializer` directly.
   - Delete dead `Materializer::for_directories` alias, `Materializer::custom`, and `Materializer::new`. Keep `select` and `for_paths`.

2. **Remove Speculative Safety Gating**:
   - Remove `Safety::UnsafePending` enum variant, `Error::UnsafeBackend`, `CopyBackend::safety()`, `ensure_backend_runnable()`, and `PendingBackend` test fixture in `crates/wt-copy/src/lib.rs` and `copy_tree.rs`.

3. **Streamline `Materializer::select` & Sys Helpers**:
   - Refactor `Materializer::select` in `materialize.rs` to compute `(backend, strategy)` tuples in a single match/if expression before constructing `Self`.
   - In `crates/wt-copy/src/sys.rs`, replace manual 128 KiB heap buffer loop in `buffered_copy_file` with standard `std::io::copy`.
   - In `crates/wt-copy/src/clonefile.rs`, delete duplicate `c_path` helper and delegate `ClonefileBackend::supports` to `sys::probe_fs_capabilities`.

## Verification

- Run `cargo test -p wt-copy`.
- Verify APFS clonefile, Linux reflink, `copy_file_range`, hardlink, and deep copy backends all pass.
