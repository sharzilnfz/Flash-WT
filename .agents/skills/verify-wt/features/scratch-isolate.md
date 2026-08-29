# Scratch / isolate a sandbox

`wt scratch` (and its alias `wt isolate`) creates an ephemeral leased worktree,
executes an optional command inside it, and tears down on exit.
This serves as the primary entry point for isolated agent execution.

## Sub-features

- `scratch-run` runs a command inside a hydrated sandbox and cleans up after.
- `scratch-lease` persists a TTL lease for sandboxes left behind.
- `isolate-alias` behaves identically to scratch for agent execution with command name `isolate`.
- `scratch-auto-name` generates a `scratch-<id>` branch when no name is given.
- `scratch-manifest` honors custom `--manifest` overrides.
## How to get to it (user POV)

- Run `wt scratch --run '<command>'` inside a repo with a `.wtinclude`.
- Run `wt isolate --ttl 1h` to leave a leased sandbox behind for later work.
- Run `wt scratch <name> --dir <path>` for a named, long-lived scratch tree.
- Optionally pass `--manifest <path>` to override included directories.
## Driving it with the shell fixture

Preconditions:

- Fixture loaded, cwd `$WT_ORIGIN`.

- **Run-and-clean.** `wt --json scratch --dir "$WT_FIXTURE/scratch1" --run 'echo inside && ls heavy/pkg00'`.
  When `--json` is enabled, child command output routes to stderr, while stdout
  contains strictly single-line NDJSON. Envelope `data.exit_code` is `0`,
  `data.cleaned_up` is `true`, `data.lease_id` and `data.expires_at` are present.
- **Leave one behind.** `wt --json isolate --dir "$WT_FIXTURE/iso1" --ttl 1h`.
  Envelope `data.cleaned_up` is `false`; `$WT_FIXTURE/iso1/heavy` exists.
- **Verify lease.** `test -f "$WT_STORE/worktrees/scratch-<id>.lease"` using
  `data.lease_file` from the envelope.
- **Remove the leftover.** `wt --json remove <data.branch> --dir "$WT_FIXTURE/iso1"`
  (branch is the generated `scratch-<id>`), then
  `wt --json sweep --age 0s` reports `data.leases_reclaimed` of `1`.
- **Proof.** Save both envelopes, the command stdout, and the sweep envelope
  to `artifacts/verify-wt/<run-id>/`.

## Gotchas

- In `--json` mode, child command output never interleaves with the JSON envelope.
  Child stdout redirects to stderr, keeping stdout clean NDJSON.
- A nonzero exit from `--run` surfaces in `data.exit_code` while envelope `status`
  remains `ok`. The `wt` binary exits with the child status code.
- `isolate` and `scratch` without `--run` leave a real worktree and a lease
  file behind. Proofs must remove the worktree by the generated branch name.
- `wt clean` or `wt remove` on a leased sandbox before TTL expiry removes the
  worktree, but the lease file lingers until a sweep reclaims it.
