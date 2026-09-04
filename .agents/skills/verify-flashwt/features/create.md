# Create a worktree

`flashwt new` (and its classic equivalent `flashwt create`) provisions a new git
worktree on a new branch with the heavy directories listed in `.flashwtinclude`
already materialized from the content-addressed store.

## Sub-features

- `create-branch` creates a git branch and worktree at `--dir` (or a sibling `<repo>-<name>` by default).
- `create-hydrate` materializes manifest directories as private writable files.
- `create-manifest` honors `--manifest` overrides of the default `.flashwtinclude`.
- `create-base` honors `--base` for the starting ref.
- `create-modern-verb` supports `flashwt new` as an alias to `flashwt create`, with `"command": "create"` in the JSON envelope.
- `create-resumption` detects an interrupted operation receipt and resumes hydration rather than failing.
- `create-lockfile-guard` refuses dependency hydration when `new --base <ref>` targets a branch whose lockfile differs from the donor checkout, leaving the worktree in place with a clean message naming the package manager to run.

## How to get to it (user POV)

- Run `flashwt new <name>` (modern verb) or `flashwt create <name>` (classic verb) inside a git repo that has a `.flashwtinclude`.
- Optionally pass `--base <ref>` to choose the starting ref.
- Optionally pass `--manifest <path>` to choose an explicit manifest.
- Optionally pass `--dir <path>` to target a custom destination.

## Driving it with the shell fixture

Preconditions:

- Fixture loaded (`FLASHWT_BIN`, `FLASHWT_ORIGIN`, `FLASHWT_STORE` set), cwd `$FLASHWT_ORIGIN`.
- `FLASHWT_NO_TINY_BYPASS=1` exported to ensure small fixtures exercise store ingestion.
- Fixture ships `.flashwtinclude` containing `heavy/` and 40 files under `heavy/`.

- **Create.** `flashwt --json new demo --dir "$FLASHWT_FIXTURE/demo"`. Envelope
  `status` is `ok`; `command` is `create`; `data.branch` is `demo`;
  `data.files_hydrated` is `40`; `data.hydration_method` is present;
  `data.cache_hit` is boolean; `data.bytes_shared_cow` is numeric; `data.duration_ms` is present.
- **Verify branch.** `git -C "$FLASHWT_FIXTURE/demo" branch --show-current` prints
  `demo`; `git -C "$FLASHWT_ORIGIN" worktree list` shows the new worktree.
- **Verify hydration on disk.** `ls "$FLASHWT_FIXTURE/demo/heavy"` shows `pkg00`
  through `pkg19`; `cat "$FLASHWT_FIXTURE/demo/heavy/pkg00/nested/file-0.txt"` matches the
  fixture content `fake-heavy file 0 of 40`.
- **Verify isolation.** Write `echo edited > "$FLASHWT_FIXTURE/demo/heavy/pkg00/nested/file-0.txt"`.
  The write succeeds because hydrated files are private and writable. Sibling worktrees remain unaffected.
- **Manifest override.** `flashwt --json create alt --dir "$FLASHWT_FIXTURE/alt" --manifest /dev/null`
  reports `data.files_hydrated` of `0` and `data.hydration_method` of `none`.
  This proves that the manifest, not gitignore, drives hydration.
- **Duplicate destination error.** Running create against an existing directory with no active receipt fails with exit code 2 and returns a JSON error envelope with `status: "error"`.
- **Cross-branch lockfile mismatch.** From a checkout whose lockfile differs from `--base <ref>`, run `flashwt --json new <name> --base <ref> --dir <outside>`. Envelope `status` is `ok`; `data.hydration_method` is `"none"`; `data.files_hydrated` is `0`; one warning diagnostic carries `code` `"LOCKFILE_MISMATCH"` with a message naming the mismatched lockfile and the package manager command to run. Human output prints `flashwt: lockfile mismatch in <ref> (...)` plus the same command.
- **Proof.** Save both envelopes plus `ls -R` of the hydrated tree and the
  `find "$FLASHWT_STORE" -maxdepth 2` listing to `artifacts/verify-flashwt/<run-id>/`.

## Gotchas

- An absent `.flashwtinclude` does not fail. Instead, `flashwt` applies default rules in memory
  without writing to disk and hydrates any matching paths. (Use `flashwt init` to write a starter manifest file).
  Pass `--manifest /dev/null` to enforce zero hydration.
- Repositories with under 500 files and under 8 MB trigger tiny repository bypass by default, skipping store ingestion. Set `FLASHWT_NO_TINY_BYPASS=1` in fixtures to verify store hydration.
- `hydration_method` is filesystem-dependent: `byte_copy` plus a
  `CROSS_DEVICE_COPY_DEGRADATION` diagnostic on tmpfs; CoW/hardlink
  elsewhere. Do not assert a specific method unless you control the filesystem.
- The default worktree destination is a sibling of the repo. Always pass
  `--dir` into the fixture, or proofs litter the real filesystem.
- If an interrupted create leaves an incomplete receipt behind, a second create with the same name resumes hydration rather than failing.
- A lockfile-guard refusal exits successfully with an empty heavy directory on purpose: run the named package manager inside the new worktree instead of treating the empty directory as broken. The guard only runs when `--base` is passed.
