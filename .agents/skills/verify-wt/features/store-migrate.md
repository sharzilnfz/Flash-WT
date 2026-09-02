# Migrate store garbage collection

`wt store migrate` manages the store's garbage-collection architecture, enabling the one-way cutover from legacy refcount files (`refs/`) to modern mark-and-sweep GC driven by store mirrors and a safety grace period (ADR-0004).

## Sub-features

- `migrate-activate-mark-sweep` switches the store GC mode to `mark-sweep`, determining blob liveness from active worktree mirrors while maintaining backward-compatible `refs/`.
- `migrate-drop-legacy-refs` irreversibly removes all `refs/` files and activates `mark-sweep-no-refs` mode.
- `migrate-mode-persistence` records the active GC scheme in `$WT_STORE/gc-mode`.
- `migrate-mutual-exclusion` enforces specifying exactly one migration action flag.

## How to get to it (user POV)

- Run `wt store migrate --activate-mark-sweep` to transition the store to mirror-based mark-sweep GC.
- Run `wt store migrate --drop-legacy-refs` to purge all legacy ref files and prevent pre-cutover binary usage.

## Driving it with the shell fixture

Preconditions:

- Fixture loaded, cwd `$WT_ORIGIN`.
- Store initialized with legacy ref files (default state after `wt create demo`).

- **Create initial worktree to populate refs.** `wt --json new demo --dir "$WT_FIXTURE/demo"`. Check that `$WT_STORE/refs` exists and contains ref files.
- **Activate mark-sweep.** `wt --json store migrate --activate-mark-sweep`. Envelope `status` is `ok`; `command` is `store`; `data.gc_mode` is `mark-sweep`; `data.purged_legacy_refs` is `null`.
- **Verify mode file.** `cat "$WT_STORE/gc-mode"` prints `mark-sweep`.
- **Verify sweep in mark-sweep mode.** `wt --json sweep --age 0s` reports `data.mode` is `mark-sweep`, `data.mirrors_removed` is `0`, and `data.deferred_by_grace` is `false`.
- **Drop legacy refs.** `wt --json store migrate --drop-legacy-refs`. Envelope `status` is `ok`; `data.gc_mode` is `mark-sweep-no-refs`; `data.purged_legacy_refs` is >= 40.
- **Verify refs directory is purged.** `test ! -d "$WT_STORE/refs" || [ $(find "$WT_STORE/refs" -type f | wc -l) -eq 0 ]`.
- **Proof.** Save migration envelopes and `$WT_STORE/gc-mode` contents to `artifacts/verify-wt/<run-id>/`.

## Gotchas

- `--drop-legacy-refs` is irreversible and makes the store unreadable to older binaries that rely on refcount files for GC safety.
- `--activate-mark-sweep` and `--drop-legacy-refs` are mutually exclusive; passing neither or both returns a CLI usage error.
- In mark-sweep mode, objects created within the grace period (`WT_GC_GRACE`, default 15 minutes) are deferred and protected from sweep even if unreferenced. Use `--age 0s` to test immediate reclamation.
