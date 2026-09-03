# 05: workspace hygiene — lints, metadata, CI, edition

Status: ready-for-agent
Blocked by: tickets 01-04 merged into `arch/hardening-and-simplify`
Runs sequentially on the integration branch (touches all crates).

## Work

1. Root Cargo.toml `[workspace.lints]`:

```toml
[workspace.lints.rust]
unsafe_op_in_unsafe_fn = "deny"
missing_docs = "warn"

[workspace.lints.clippy]
all = { level = "warn", priority = -1 }
unwrap_used = "warn"
expect_used = "warn"
```

Add `[lints] workspace = true` to each member. Fix resulting hits with real
errors or narrow `#[expect(..., reason)]`. Do NOT enable pedantic yet; do not
blanket-deny restriction lints.

2. `[workspace.package]` gains description/repository/readme/rust-version
   (measure actual MSRV before pinning). Add `[workspace.dependencies]` for
   libc, tempfile, sha2, clap, thiserror; members inherit with
 `<dep>.workspace = true`.
3. Drop libc from flashwt-cli: replace the single
   `e.raw_os_error() == Some(libc::ENOENT)` with
   `e.kind() == io::ErrorKind::NotFound`; delete the dep. Remove the dead
   empty `[dev-dependencies]` section in flashwt-store/Cargo.toml.
4. CI (.github/workflows/ci.yml):
   - macOS job step running `cargo clippy --all-targets --locked -- -D
     warnings` so cfg(macos) code (clonefile, bulkwalk externs) finally gets
     linted; expect existing warnings to surface, fix them.
   - Weekly scheduled cargo-audit job against Cargo.lock.
5. Edition 2024 migration: run `cargo fix --edition`, flip edition to 2024,
   set rust-version accordingly (>= 1.85). Watch for `unsafe extern` block
   requirements in bulkwalk.rs. Run the FULL test suite, not just unit tests.
6. Add one-line `rustfmt.toml` with `style_edition = "2024"` at the bump;
   run `cargo fmt` once. No clippy.toml unless a threshold needs tuning.

## Done when

- `cargo build && cargo test --workspace` green on edition 2024.
- `cargo clippy --workspace --all-targets -- -D warnings` green locally on
  macOS.
- `cargo audit` (if installable) or the scheduled workflow file committed.

## Comments

Completed on `arch/hardening-and-simplify` via wave 2 of the orchestrated build. See git log for the implementing commits.
