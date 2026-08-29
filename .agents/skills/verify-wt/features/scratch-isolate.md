# Scratch / isolate a sandbox

`wt scratch` (alias `wt isolate`) creates an ephemeral leased worktree, can
execute one command inside it, and tears down on exit — the agent-oriented
entry point for isolated execution.

## Sub-features

- `scratch-run` runs a command inside a hydrated sandbox and cleans up after.
- `scratch-lease` persists a TTL lease for sandboxes left behind.
- `isolate-alias` behaves identically to scratch for agent execution.
- `scratch-auto-name` generates a `scratch-<id>` branch when no name is given.

## How to get to it (user POV)

- Run `wt scratch --run '<command>'` inside a repo with a `.wtinclude`.
- Or `wt isolate --ttl 1h` to leave a leased sandbox behind for later work.
- Or `wt scratch <name> --dir <path>` for a named, long-lived scratch tree.

## Driving it with the shell fixture

Preconditions:

- Fixture loaded, cwd `$WT_ORIGIN`.

- **Run-and-clean.** `wt --json scratch --dir "$WT_FIXTURE/scratch1" --run 'echo inside && ls heavy/pkg00'`.
  Command output (`inside`, `file-0.txt` …) appears on stdout before the
  envelope. Envelope `data.exit_code` is `0`, `data.cleaned_up` is `true`,
  `data.lease_id` and `data.expires_at` are present.
- **Verify cleanup.** `test ! -e "$WT_FIXTURE/scratch1" && echo gone`.
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

- `--run` output is interleaved with the JSON envelope on stdout — parse the
  last line, or split streams.
- `isolate`/`scratch` without `--run` leave a real worktree plus a lease
  behind; proofs must remove it by the generated branch name, not by hand.
- `wt remove` on a leased sandbox before TTL expiry still works, but the
  stale lease file lingers until a sweep reclaims it — that lingering file is
  expected, not a bug.
- A nonzero exit from `--run` surfaces in `data.exit_code` with envelope
  `status` still `ok` (the scratch lifecycle succeeded); assert on both.
