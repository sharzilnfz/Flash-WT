# Initialize starter manifest

`wt init` creates a starter `.wtinclude` manifest in the repository root or a specified target directory, declaring standard heavy directories (such as `node_modules/`, `target/`, `build/`) for fast hydration.

## Sub-features

- `init-create` writes a starter `.wtinclude` manifest file.
- `init-dir` supports targeting a specific directory via `--dir <path>`.
- `init-safety` refuses to overwrite an existing manifest unless `--force` is specified.
- `init-atomic` writes via a temporary file and atomic rename to prevent partial writes.

## How to get to it (user POV)

- Run `wt init` in the repository root to generate `.wtinclude`.
- Run `wt init --dir <path>` to initialize `.wtinclude` in a specific directory.
- Run `wt init --force` (or `-f`) to overwrite an existing `.wtinclude`.

## Driving it with the shell fixture

Preconditions:

- Fixture loaded, cwd `$WT_ORIGIN`.
- Remove fixture's existing `.wtinclude`: `rm -f "$WT_ORIGIN/.wtinclude"`.

- **Generate starter manifest.** `wt --json init`. Envelope `status` is `ok`; `command` is `init`; `data.created` is `true`; `data.manifest_path` ends with `.wtinclude`.
- **Verify file content.** `test -f "$WT_ORIGIN/.wtinclude"` succeeds; `grep -E 'node_modules|target' "$WT_ORIGIN/.wtinclude"` confirms standard starter patterns.
- **Overwrite protection.** Run `wt --json init` again without `--force`. Exit code is non-zero, envelope `status` is `error`, and error message mentions `already exists (use --force to overwrite)`.
- **Forced overwrite.** `wt --json init --force` succeeds with `data.created: true`.
- **Custom directory.** `mkdir -p "$WT_ORIGIN/subdir" && wt --json init --dir "$WT_ORIGIN/subdir"` creates `$WT_ORIGIN/subdir/.wtinclude`.
- **Proof.** Save the JSON envelopes and the generated `.wtinclude` content to `artifacts/verify-wt/<run-id>/`.

## Gotchas

- If `.wtinclude` is absent during `wt new` or `wt hydrate`, `wt` applies default rules in memory without writing a manifest on disk. Running `wt init` is required to commit a persistent manifest to git.
- Passing a directory without write permissions causes `wt init` to fail with an IO error.
- `--force` completely replaces existing custom include patterns with the default starter template.
