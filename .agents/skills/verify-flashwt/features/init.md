# Initialize starter manifest

`flashwt init` creates a starter `.flashwtinclude` manifest in the repository root or a specified target directory, declaring standard heavy directories (such as `node_modules/`, `target/`, `build/`) for fast hydration.

## Sub-features

- `init-create` writes a starter `.flashwtinclude` manifest file.
- `init-dir` supports targeting a specific directory via `--dir <path>`.
- `init-safety` refuses to overwrite an existing manifest unless `--force` is specified.
- `init-atomic` writes via a temporary file and atomic rename to prevent partial writes.

## How to get to it (user POV)

- Run `flashwt init` in the repository root to generate `.flashwtinclude`.
- Run `flashwt init --dir <path>` to initialize `.flashwtinclude` in a specific directory.
- Run `flashwt init --force` (or `-f`) to overwrite an existing `.flashwtinclude`.

## Driving it with the shell fixture

Preconditions:

- Fixture loaded, cwd `$FLASHWT_ORIGIN`.
- Remove fixture's existing `.flashwtinclude`: `rm -f "$FLASHWT_ORIGIN/.flashwtinclude"`.

- **Generate starter manifest.** `flashwt --json init`. Envelope `status` is `ok`; `command` is `init`; `data.created` is `true`; `data.manifest_path` ends with `.flashwtinclude`.
- **Verify file content.** `test -f "$FLASHWT_ORIGIN/.flashwtinclude"` succeeds; `grep -E 'node_modules|target' "$FLASHWT_ORIGIN/.flashwtinclude"` confirms standard starter patterns.
- **Overwrite protection.** Run `flashwt --json init` again without `--force`. Exit code is non-zero, envelope `status` is `error`, and error message mentions `already exists (use --force to overwrite)`.
- **Forced overwrite.** `flashwt --json init --force` succeeds with `data.created: true`.
- **Custom directory.** `mkdir -p "$FLASHWT_ORIGIN/subdir" && flashwt --json init --dir "$FLASHWT_ORIGIN/subdir"` creates `$FLASHWT_ORIGIN/subdir/.flashwtinclude`.
- **Proof.** Save the JSON envelopes and the generated `.flashwtinclude` content to `artifacts/verify-flashwt/<run-id>/`.

## Gotchas

- If `.flashwtinclude` is absent during `flashwt new` or `flashwt hydrate`, `flashwt` applies default rules in memory without writing a manifest on disk. Running `flashwt init` is required to commit a persistent manifest to git.
- Passing a directory without write permissions causes `flashwt init` to fail with an IO error.
- `--force` completely replaces existing custom include patterns with the default starter template.
