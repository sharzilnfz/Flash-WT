# Issue 04: Encapsulated Materializer & Capability Matrix

Status: ready-for-human

## Context
Filesystem probing, strategy selection (APFS clonefile, Linux reflink, copy_file_range, byte fallback), and inode permission repairs (`finalize_mode` replacing shared hardlinks with private copies) are currently scattered between `wt-cli` and `wt-copy`.

## Requirements
- Deepen `wt-copy::Materializer` to handle capability probing, backend strategy resolution, and permission mode normalization internally.
- Encapsulate hardlink inode exec-bit repairs within `wt-copy` so callers simply supply target permission bits.
- Provide uniform error classification and diagnostics for cross-device fallback degradation.

## Files Owned
- `crates/wt-copy/src/materialize.rs`
- `crates/wt-copy/src/selection.rs`
- `crates/wt-copy/src/lib.rs`
