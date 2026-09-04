# Migrate store garbage collection

`flashwt store` manages store maintenance and GC scheme transitions, including categorized disk usage inspection (`du`) and one-way cutovers from legacy refcounts to mark-sweep GC (ADR-0004).

## Sub-features

- `store-du` inspects and reports categorized storage usage across objects, snapshots, mirrors, refs, and caches.
- `migrate-activate-mark-sweep` switches store GC mode to `mark-sweep`, determining blob liveness from active worktree mirrors while maintaining backward-compatible `refs/`.
- `migrate-drop-legacy-refs` irreversibly removes all `refs/` files and activates `mark-sweep-no-refs` mode.
- `migrate-mode-persistence` records the active GC scheme in `$FLASHWT_STORE/gc-mode`.
- `migrate-mutual-exclusion` enforces specifying exactly one migration action flag.

## How to get to it (user POV)

- Run `flashwt store du` (or `flashwt store disk-usage`) to view categorized store disk consumption.
- Run `flashwt store migrate --activate-mark-sweep` to transition the store to mirror-based mark-sweep GC.
- Run `flashwt store migrate --drop-legacy-refs` to purge all legacy ref files.

## Driving it with the shell fixture

Preconditions:

- Fixture loaded, cwd `$FLASHWT_ORIGIN`.
- Store initialized with legacy ref files (default state after `flashwt create demo`).

- **Inspect store disk usage.** `flashwt --json store du`. Envelope `status` is `ok`; `command` is `store du`; `data.total_bytes` is numeric.
- **Activate mark-sweep.** `flashwt --json store migrate --activate-mark-sweep`. Envelope `status` is `ok`; `command` is `store`; `data.gc_mode` is `mark-sweep`. Field `purged_legacy_refs` is omitted from serialized JSON when not dropping refs.
- **Verify mode file.** `cat "$FLASHWT_STORE/gc-mode"` prints `mark-sweep`.
- **Verify sweep in mark-sweep mode.** `flashwt --json sweep --age 0s` reports `data.mode` is `mark-sweep`, `data.mirrors_removed` is `0`, and `data.deferred_by_grace` is `false`.
- **Drop legacy refs.** `flashwt --json store migrate --drop-legacy-refs`. Envelope `status` is `ok`; `data.gc_mode` is `mark-sweep-no-refs`; `data.purged_legacy_refs` is >= 40.
- **Verify refs directory is purged.** Files inside `$FLASHWT_STORE/refs` are removed: `[ $(find "$FLASHWT_STORE/refs" -type f | wc -l) -eq 0 ]`.
- **Proof.** Save migration envelopes and `$FLASHWT_STORE/gc-mode` contents to `artifacts/verify-flashwt/<run-id>/`.

## Gotchas

- `--drop-legacy-refs` is irreversible and prevents older binaries relying on refcount files from safely reading the store.
- `--activate-mark-sweep` and `--drop-legacy-refs` are mutually exclusive; passing neither or both returns a CLI error.
- In mark-sweep mode, objects created within the grace period (`FLASHWT_GC_GRACE`, default 15 minutes) are deferred and protected from sweep even if unreferenced. Use `--age 0s` to test immediate reclamation.
- `purged_legacy_refs` is omitted from JSON output during `--activate-mark-sweep` rather than serialized as null.
