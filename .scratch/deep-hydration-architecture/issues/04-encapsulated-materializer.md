# Issue 04: Encapsulated Materializer & Capability Matrix

Status: ready-for-human

## Context
Filesystem probing, strategy selection (APFS clonefile, Linux reflink, copy_file_range, byte fallback), and inode permission repairs (`finalize_mode` replacing shared hardlinks with private copies) are currently scattered between `flashwt-cli` and `flashwt-copy`.

## Requirements
- Deepen `flashwt-copy::Materializer` to handle capability probing, backend strategy resolution, and permission mode normalization internally.
- Encapsulate hardlink inode exec-bit repairs within `flashwt-copy` so callers simply supply target permission bits.
- Provide uniform error classification and diagnostics for cross-device fallback degradation.

## Files Owned
- `crates/flashwt-copy/src/materialize.rs`
- `crates/flashwt-copy/src/selection.rs`
- `crates/flashwt-copy/src/lib.rs`
