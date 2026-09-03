# Ephemeral lease management

`flashwt lease` inspects active and expired ephemeral scratch worktree leases, reporting owning process identifiers, remaining time-to-live durations, and backing worktree paths.

## Sub-features

- `lease-show` displays active ephemeral leases from the store.
- `lease-show-all` includes expired leases when `--all` is passed.
- `lease-show-id` targets a specific lease identifier.
- `lease-aliases` supports `flashwt lease list` and `flashwt lease ls` as direct aliases.

## How to get to it (user POV)

- Run `flashwt lease show` to list active leases.
- Run `flashwt lease show --all` to list both active and expired leases.
- Run `flashwt lease show <id>` to inspect a single lease.
- Run `flashwt --json lease show` to obtain structured JSON data.

## Driving it with the shell fixture

Preconditions:

- Fixture loaded (`FLASHWT_BIN`, `FLASHWT_ORIGIN`, `FLASHWT_STORE` set), cwd `$FLASHWT_ORIGIN`.
- One leased sandbox created: `flashwt --json isolate --dir "$FLASHWT_FIXTURE/iso1" --ttl 1h`.

- **List active leases.** `flashwt lease show`. Output displays the lease identifier, process ID, remaining TTL, and worktree destination.
- **Inspect JSON envelope.** `flashwt --json lease show --all`. Envelope `status` is `ok`; `command` is `lease`; `data.leases` contains an entry matching `iso1`.
- **Proof.** Save the text table and JSON envelope to `artifacts/verify-flashwt/<run-id>/`.

## Gotchas

- Leases whose owning process has exited appear with dead status indicators in human output.
- `flashwt clean` or `flashwt remove` deletes the worktree, but the lease record remains in the store until `flashwt sweep` collects it.
