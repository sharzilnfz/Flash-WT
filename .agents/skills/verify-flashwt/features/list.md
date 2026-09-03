# List active worktrees

`flashwt list` (and its shorthand alias `flashwt ls`) discovers all active git worktrees, inspects their branch names, heads, and hydration state, and computes exact disk usage alongside shared deduplication savings.

## Sub-features

- `list-worktrees` enumerates all registered git worktrees for the current repository.
- `list-hydration-stats` calculates hydrated file count and bytes hydrated from store mirrors and `flashwt-hydrated.tsv` sidecars.
- `list-disk-savings` computes deduplicated disk space saved through block sharing.
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
- **Inspect entry fields.** Each item in `data.worktrees` provides `branch`, `path`, `head`, `is_active`, `is_main`, `is_ephemeral`, `files_hydrated`, `bytes_hydrated`, `bytes_saved`, and `hydrated_dirs`.
- **Shorthand alias.** `flashwt --json ls` produces identical envelope output with `command` `list`.
- **Ephemeral lease tracking.** Create a scratch tree with lease: `flashwt --json isolate --dir "$FLASHWT_FIXTURE/iso1" --ttl 1h`. Next `flashwt --json list` shows the worktree entry with `is_ephemeral: true`, `lease.lease_id` matching the lease, `lease.pid_alive: true`, and positive `ttl_remaining_secs`.
- **Human table format.** Run `flashwt list` without `--json` to verify formatted terminal output containing columns `BRANCH`, `PATH`, `HYDRATED`, `DISK SAVED`, `AGE / STATUS` and summary line `Total disk saved: ...`.
- **Proof.** Save `flashwt --json list` envelope and human table output to `artifacts/verify-flashwt/<run-id>/`.

## Gotchas

- `bytes_saved` represents the logical disk space saved by sharing blobs from the store; in non-shared stores or when only one worktree exists, it reflects logical reuse over full separate duplication.
- Detached or deleted worktree directories may still appear in `git worktree list` until git prunes them.
- Scratch worktrees show `expired` in human output once their TTL elapses, but remain on disk until swept or cleaned.
