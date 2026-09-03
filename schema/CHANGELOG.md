# Schema Changelog

## [1.0.0] - 2026-09-03

### Frozen Contract
- Authoritative Envelope v1 schema published at `schema/v1.json`.
- Root envelope format frozen:
  - `flashwt_version`: string
  - `schema_version`: integer (1)
  - `command`: string
  - `status`: string ("ok" | "error")
  - `data`: object (command-specific payload) | null
  - `diagnostics`: array of diagnostic objects (`code`, `message`, optional `level`)

### Command Payloads Frozen
- `create` (`CreateData`): worktree path, branch, cache hit, duration, hydration method, CoW bytes, copied bytes, files hydrated.
- `hydrate` (`HydrateData`): destination path, source path, cache hit, duration, method, CoW bytes, copied bytes, files hydrated, dirs hydrated.
- `init` (`InitData`): manifest path, created boolean.
- `clean` (`CleanData`): removed worktrees, branches removed, references released, mirrors removed, reclaimed bytes, sweep examined/reclaimed.
- `remove` (`RemoveData`): worktree path, branch, references released, mirror removed.
- `sweep` (`SweepData`): mode, examined, reclaimed, and optional lease / snapshot metrics.
- `scrub` (`ScrubData`): dry run, scanned, corrupt blobs, deleted, and optional snapshot metrics.
- `store` (`MigrateData`): gc mode, purged legacy refs.
- `scratch` (`ScratchData`): worktree path, branch, lease id, lease file, expires at, files hydrated, method, bytes, duration, run command exit.
- `list` (`ListData`): worktrees list with disk usage accounting, total disk saved, total files hydrated.
- `demo` (`DemoData`): 10,000-file benchmark statistics, speedup ratio, bytes shared, isolation verification.
- `lease` (`LeaseData`): active scratch leases list with pid, liveness, ttl remaining, expiration, worktree path, and git dir.

### Additive Extension Rules
- Any future field added to v1 payloads must be optional with `skip_serializing_if = "Option::is_none"`.
- Existing non-optional fields may not be removed or renamed.
- Error diagnostics use stable uppercase codes.
- Mutating commands persist atomic `flashwt-receipt.json` files enabling crash recovery.
