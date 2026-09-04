# In-place hydration

`flashwt hydrate` materializes heavy directories into an existing git worktree or pre-existing directory in-place from the content-addressed store without provisioning a new git branch or worktree.

## Sub-features

- `hydrate-existing-dest` hydrates heavy files into an existing target directory or worktree.
- `hydrate-source-discovery` infers the source repository from cwd or accepts an explicit `--source <path>`.
- `hydrate-manifest` honors `--manifest` overrides of default `.flashwtinclude`.
- `hydrate-cow` uses fast Copy-on-Write clones (`fclonefileat` on APFS) or falls back to byte copies.
- `hydrate-lockfile-hit` skips store ingestion entirely when a pinned lockfile matches a published snapshot, cloning the snapshot tree in one step and reporting `hydration_method` `"clone"` with `bytes_shared_cow` above zero and `bytes_copied` at zero.
- `hydrate-timing` measures stage durations and reports total files and bytes shared.

## How to get to it (user POV)

- Run `flashwt hydrate <path>` to hydrate heavy files into `<path>` using the current repository's `.flashwtinclude`.
- Run `flashwt hydrate <path> --source <repo-root>` to hydrate from an explicit source repository root.
- Run `flashwt hydrate <path> --manifest <manifest-file>` to select an explicit manifest.

## Driving it with the shell fixture

Preconditions:

- Fixture loaded (`FLASHWT_BIN`, `FLASHWT_ORIGIN`, `FLASHWT_STORE` set), cwd `$FLASHWT_ORIGIN`.
- Fixture contains `.flashwtinclude` and heavy files under `$FLASHWT_ORIGIN/heavy`.
- An existing directory exists at `$FLASHWT_FIXTURE/target-dir`.

- **Prepare target.** `mkdir -p "$FLASHWT_FIXTURE/target-dir"` creates the existing target directory.
- **Hydrate in-place.** `flashwt --json hydrate "$FLASHWT_FIXTURE/target-dir" --source "$FLASHWT_ORIGIN"`. Envelope `status` is `ok`; `command` is `hydrate`; `data.destination_path` matches canonical target; `data.files_hydrated` is `40`; `data.hydration_method` is present; `data.bytes_shared_cow` or `data.bytes_copied` is greater than 0; `data.dirs_hydrated` contains `heavy`.
- **Verify disk contents.** `test -f "$FLASHWT_FIXTURE/target-dir/heavy/pkg00/nested/file-0.txt"` passes; `cat "$FLASHWT_FIXTURE/target-dir/heavy/pkg00/nested/file-0.txt"` matches fixture content `fake-heavy file 0 of 40`.
- **Nonexistent destination fails.** `flashwt --json hydrate "$FLASHWT_FIXTURE/does-not-exist"` returns error `status` and failure diagnostic `destination path ... does not exist`.
- **Custom manifest override.** `flashwt --json hydrate "$FLASHWT_FIXTURE/target-dir" --manifest /dev/null` reports `data.files_hydrated` of `0`.
- **Proof.** Save stdout JSON envelope and `ls -laR "$FLASHWT_FIXTURE/target-dir"` to `artifacts/verify-flashwt/<run-id>/`.

## Gotchas

- Target path must already exist on disk and be a directory, or `flashwt hydrate` exits with a usage error.
- When `--source` is omitted outside a git repository or worktree, `flashwt hydrate` cannot infer the source root and exits with an error.
- Hydration into an existing directory does not register a git worktree or create a git branch; it strictly materializes the heavy files.
- A lockfile-hit reports `hydration_method` `"clone"`, never `"snapshot"`; check `bytes_shared_cow` and `bytes_copied` to distinguish a true snapshot hit from per-file placement.
