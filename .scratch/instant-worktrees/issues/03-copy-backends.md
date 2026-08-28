# 03: Copy backends

**What to build:** All three copy strategies behind the backend trait from
ticket 01: whole-directory `clonefile` on APFS, reflink where Linux
filesystems support it, hardlink elsewhere. Backend selection detects what
the filesystem supports and picks the fastest safe option. Pure library code
against the trait, tested against real temp directories; no CLI involvement.
Hardlink mode ships disabled by default until ticket 07 makes it safe.

**Blocked by:** 01 (skeleton, contracts, test rig).

**Status:** needs-triage

- [x] Clonefile backend clones a thousand-file directory in well under a second
- [x] Reflink backend passes the same test on a supporting Linux filesystem
- [x] Hardlink backend exists but reports itself as unsafe-pending
- [x] Selection logic picks the best available backend per filesystem
- [x] Unit tests per backend plus one integration test through the trait

## Comments

2026-08-23 (agent): Implemented in `crates/wt-copy`. `StubBackend` removed;
its DeepCopy slot is now a real portable fallback (`DeepCopyBackend`).

- `clonefile.rs` (macOS): whole-directory `clonefile(2)`; `supports()`
  checks the `statfs` fstypename is `apfs`. Thousand-file tree clones in
  milliseconds on this machine (test asserts < 1 s).
- `reflink.rs` (Linux): per-file `FICLONE` ioctl over a shared tree walker;
  `supports()` matches btrfs/XFS magic via `statfs`. Test skips silently on
  tmpfs/ext4 hosts; CI needs a btrfs/XFS runner to exercise it.
- `hardlink.rs`: present, reports `Safety::UnsafePending`, `copy_dir`
  refuses with `Error::UnsafeBackend` until ticket 07.
- `deep_copy.rs`: byte-copy fallback, always supports.
- `selection.rs`: `candidates()` (fastest-first, hardlink excluded) and
  `select_backend(dir)` returning `Box<dyn CopyBackend>` — first safe,
  supported candidate, else deep copy.

Public API added: `ClonefileBackend` (macOS), `ReflinkBackend` (Linux),
`HardlinkBackend`, `DeepCopyBackend`, `candidates()`, `select_backend()`.
The frozen trait in `lib.rs` is untouched. Tests: unit tests per backend
plus integration through the trait (`tests/backends.rs`, fixture in
`tests/common/mod.rs`). Verified: `cargo test -p wt-copy` (14 pass),
`cargo clippy --workspace --all-targets` clean, `cargo fmt --check` clean,
and `cargo check -p wt-copy --target x86_64-unknown-linux-gnu` for the
reflink path.
