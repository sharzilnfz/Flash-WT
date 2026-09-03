# 04: flashwt-copy hardening — source policy, atomic copies, sys consolidation

Status: ready-for-agent
Owner branch: `arch/copy-harden`
Owns: `crates/flashwt-copy/**`.

## Problem

Three holes. First, selection hands the mutating hardlink backend arbitrary
sources: stripping write bits from the shared inode also strips them from the
source path, so hydrating FROM a live checkout silently makes its files
unwritable. Second, mid-copy_tree failure leaves a half-built dest (and with
hardlink, demoted source files); the trait contract punts cleanup to callers.
Third, four near-duplicate statfs wrappers repeat unsafe CString/zeroed-struct
boilerplate across backends.

## Work

1. Source-side safety seam: widen select_backend to take a source-immutability
   hint (e.g. `SourcePolicy { Immutable, Any }`) or src+dest signatures. When
   the caller cannot promise immutability, hardlink falls through to deep
   copy. Update the two call sites + flashwt-cli usage. Document in lib.rs that
   hydration from the Store is Immutable; from live checkouts is Any.
2. Atomic copy contract: each backend materializes into
   `<dest>.<pid>.tmp` created exclusively, then renames onto dest; on error
   remove the tmp tree before returning. Shared wrapper in copy_tree.rs;
   backends change ~5 lines each. Contract becomes "on Err, dest does not
   exist". Kill the ensure_dest_free TOCTOU window this way.
3. Consolidate statfs probing into one private `sys.rs`:
   `statfs_of(path) -> io::Result<libc::statfs>` cfg'd internally, one SAFETY
   comment, unit-testable. clonefile/reflink/hardlink keep predicate
   functions over it.
4. Panic removal: CloneOut::materialize_file's file_name().expect returns
   Error::InvalidInput like everything else in materialize.rs. Same treatment
   for select_backend's expect if lint work flags it.
5. Mode parity: reflink's FICLONE path must produce the same final mode as
   fs::copy; add explicit set_permissions after successful FICLONE.
6. Test gaps in tests/backends.rs (run through the trait so every runnable
   backend gets them): empty directories preserved; symlink-to-directory in
   src; dangling symlink; failure-in-the-middle via a #[cfg(test)] injection
   hook asserting dest state afterwards; Safety::UnsafePending refusal path
   via a test-local backend. Mark the wall-clock perf tests #[ignore] or give
   them a generous multiplier.
7. Add SAFETY comments to bulkwalk-style extern declarations if any live in
   this crate without them.

## Constraints

- ADR-0006 settled library choices; do not introduce external crates here.
- The "pnpm lesson" write-bit stripping stays; only the SOURCE side gets a
  policy gate.

## Done when

- `cargo test -p flashwt-copy` passes including new cases.
- `cargo clippy -p flashwt-copy --all-targets` clean.
- No code path can point hardlink at a source the caller didn't declare
  immutable.

## Comments

Completed on `arch/hardening-and-simplify` via wave 2 of the orchestrated build. See git log for the implementing commits.
