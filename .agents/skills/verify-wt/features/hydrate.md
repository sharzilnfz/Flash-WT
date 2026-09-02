# In-place hydration

`wt hydrate` materializes heavy directories into an existing git worktree or pre-existing directory in-place from the content-addressed store without provisioning a new git branch or worktree.

## Sub-features

- `hydrate-existing-dest` hydrates heavy files into an existing target directory or worktree.
- `hydrate-source-discovery` infers the source repository from cwd or accepts an explicit `--source <path>`.
- `hydrate-manifest` honors `--manifest` overrides of default `.wtinclude`.
- `hydrate-cow` uses fast Copy-on-Write clones (`fclonefileat` on APFS) or falls back to byte copies.
- `hydrate-timing` measures stage durations and reports total files and bytes shared.

## How to get to it (user POV)

- Run `wt hydrate <path>` to hydrate heavy files into `<path>` using the current repository's `.wtinclude`.
- Run `wt hydrate <path> --source <repo-root>` to hydrate from an explicit source repository root.
- Run `wt hydrate <path> --manifest <manifest-file>` to select an explicit manifest.

## Driving it with the shell fixture

Preconditions:

- Fixture loaded (`WT_BIN`, `WT_ORIGIN`, `WT_STORE` set), cwd `$WT_ORIGIN`.
- Fixture contains `.wtinclude` and heavy files under `$WT_ORIGIN/heavy`.
- An existing directory exists at `$WT_FIXTURE/target-dir`.

- **Prepare target.** `mkdir -p "$WT_FIXTURE/target-dir"` creates the existing target directory.
- **Hydrate in-place.** `wt --json hydrate "$WT_FIXTURE/target-dir" --source "$WT_ORIGIN"`. Envelope `status` is `ok`; `command` is `hydrate`; `data.destination_path` matches canonical target; `data.files_hydrated` is `40`; `data.hydration_method` is present; `data.bytes_shared_cow` or `data.bytes_copied` is greater than 0; `data.dirs_hydrated` contains `heavy`.
- **Verify disk contents.** `test -f "$WT_FIXTURE/target-dir/heavy/pkg00/nested/file-0.txt"` passes; `cat "$WT_FIXTURE/target-dir/heavy/pkg00/nested/file-0.txt"` matches fixture content `fake-heavy file 0 of 40`.
- **Nonexistent destination fails.** `wt --json hydrate "$WT_FIXTURE/does-not-exist"` returns error `status` and failure diagnostic `destination path ... does not exist`.
- **Custom manifest override.** `wt --json hydrate "$WT_FIXTURE/target-dir" --manifest /dev/null` reports `data.files_hydrated` of `0`.
- **Proof.** Save stdout JSON envelope and `ls -laR "$WT_FIXTURE/target-dir"` to `artifacts/verify-wt/<run-id>/`.

## Gotchas

- Target path must already exist on disk and be a directory, or `wt hydrate` exits with a usage error.
- When `--source` is omitted outside a git repository or worktree, `wt hydrate` cannot infer the source root and exits with an error.
- Hydration into an existing directory does not register a git worktree or create a git branch; it strictly materializes the heavy files.
