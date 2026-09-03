# List active worktrees

`flashwt list` (and its shorthand alias `flashwt ls`) discovers all active git worktrees, inspects their branch names, heads, and hydration state, and computes exact disk usage alongside shared deduplication savings.

## Sub-features

- `list-worktrees` enumerates all registered git worktrees for the current repository.
- `list-hydration-stats` calculates hydrated file count and bytes hydrated from store mirrors and `flashwt-hydrated.tsv` sidecars.
- `list-disk-savings` computes deduplicated disk space saved through logical store reuse.
- `list-leases` displays active and expired ephemeral scratch sandbox leases and process liveness.
- `list-alias` supports `flashwt ls` as a direct alias for `flashwt list`.

## How to get to it (user POV)

- Run `flashwt list` or `flashwt ls` in the terminal to view an aligned table of worktrees, hydrated sizes, disk saved, and age/status.
- Run `flashwt --json list` or `flashwt --json ls` to output machine-readable JSON data with full per-worktree metadata and totals.

## Driving it with the shell fixture

Preconditions:

- Fixture loaded, cwd `$FLASHWT_ORIGIN`.
- At least one hydrated worktree created: `flashwt --json new demo --dir "$FLASHWT_FIXTURE/demo"` returned `ok` with `files_hydrated` > 0.

- **List worktrees.** `flashwt --json list`. Envelope `status` is `ok`; `command` is `list`; `data.worktrees` is a non-empty array; `data.total_disk_saved` > 0; `data.total_files_hydrated` >= 40.
- **Inspect entry fields.** Each item in `data.worktrees` provides `branch`, `path`, `is_active`, `is_main`, `is_ephemeral`, `files_hydrated`, `bytes_hydrated`, and `bytes_saved`. Optional fields like `head`, `base_branch`, `age_secs`, `hydrated_dirs`, and `lease` are populated when present.
- **Shorthand alias.** `flashwt --json ls` produces identical envelope output with `command` `list`.
- **Ephemeral lease tracking.** Create a scratch tree with lease: `flashwt --json isolate --dir "$FLASHWT_FIXTURE/iso1" --ttl 1h`. Next `flashwt --json list` shows the worktree entry with `is_ephemeral: true`, `lease.lease_id` matching the lease, `lease.pid_alive: true`, and positive `ttl_remaining_secs`.
- **Human table format.** Run `flashwt list` without `--json` to verify formatted terminal output containing 6 columns: active indicator (`*`), `BRANCH`, `PATH`, `HYDRATED`, `DISK SAVED`, `AGE / STATUS`, and summary line `Total disk saved: ... across ... worktree(s) (... files deduplicated, estimated logical reuse)`.
- **Proof.** Save `flashwt --json list` envelope and human table output to `artifacts/verify-flashwt/<run-id>/`.

## Gotchas

- `bytes_saved` represents estimated logical reuse by sharing blobs from the store; it reflects logical file size reuse over full separate duplication.
- The leading column in the human table displays `*` for the current active worktree (matching cwd) and a blank space for others.
- `hydrated_dirs` is omitted from serialized JSON for non-hydrated worktrees (such as the main repo tree).
- Scratch worktrees show `ttl: <dur> (pid: <pid>)` or `expired (pid: <pid>)` in human output depending on lease state and PID liveness.
