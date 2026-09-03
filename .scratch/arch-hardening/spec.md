# Spec: architecture hardening and simplification

Branch: `arch/hardening-and-simplify`. Three parallel audits (flashwt-store, flashwt-cli,
flashwt-copy + workspace hygiene) found no ADR conflicts. Every ticket below
strengthens existing decisions (mark-and-sweep GC, snapshots-as-cache,
explicit-first CLI); none reopens them.

## Goals

1. Split god files into deep modules: `snapshot.rs` (1118 lines), `main.rs`
   (621 lines with a ~300-line `create`).
2. Make durability claims real: fsync the files that act as truth (mirrors,
   blobs, snapshot metadata), lock refcount updates.
3. Replace stringly-typed errors with typed ones at both crate edges.
4. Close correctness holes: racy `--age` overflow panic, hardlink backend
   pointed at mutable sources, half-built trees after mid-copy failure,
   quadratic snapdiff.
5. Workspace hygiene: `[workspace.lints]`, shared dependency table, crate
   metadata, macOS clippy coverage in CI, supply-chain audit.

## Constraints

- Do not re-litigate ADR-0001 through ADR-0006.
- E2E tests parse stderr prose. Keep user-visible message text byte-identical
  unless a ticket explicitly says otherwise.
- Each ticket lists the files it owns. Agents must not edit outside their
  ownership set; cross-crate items belong to ticket 05 or a follow-up.
- Public API breaks are acceptable inside this workspace as long as all three
  crates compile and tests pass.

## Waves

- Wave 1 (done): three audit sub-agents produced the findings.
- Wave 2 (parallel, isolated worktrees):
  - Ticket 01 + 02 → branch `arch/store-refactor` (owns `crates/flashwt-store/**`)
  - Ticket 03 → branch `arch/cli-refactor` (owns `crates/flashwt-cli/**`)
  - Ticket 04 → branch `arch/copy-harden` (owns `crates/flashwt-copy/**`)
- Wave 3 (sequential on integration branch): ticket 05, workspace-wide.
- Wave 4: full verification (`cargo fmt --check`, `clippy -D warnings` on
  macOS, full test suite), blast-radius review via codebase-memory, index
  refresh.

## Tickets

- `.scratch/arch-hardening/issues/01-store-snapshot-split.md`
- `.scratch/arch-hardening/issues/02-store-durability.md`
- `.scratch/arch-hardening/issues/03-cli-decompose.md`
- `.scratch/arch-hardening/issues/04-copy-harden.md`
- `.scratch/arch-hardening/issues/05-workspace-hygiene.md`

## Out of scope

- Async I/O (ADR-0003 keeps blocking sync I/O).
- Per-event index write amplification on the hit path (gated off by default;
  revisit when snapshots leave opt-in).
- sha2 0.11 / libc 1.0 upgrades (breaking upstream moves; bundle separately).
