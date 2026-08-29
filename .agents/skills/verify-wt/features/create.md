# Create a worktree

`wt new` (and its classic equivalent `wt create`) provisions a new git
worktree on a new branch with the heavy directories listed in `.wtinclude`
already materialized from the content-addressed store.

## Sub-features

- `create-branch` creates a git branch and worktree at `--dir` (or a sibling `<repo>-<name>` by default).
- `create-hydrate` materializes manifest directories as private writable files.
- `create-manifest` honors `--manifest` overrides of the default `.wtinclude`.
- `create-base` honors `--base` for the starting ref.
- `create-modern-verb` supports `wt new` as the primary verb with `"command": "new"`.
## How to get to it (user POV)

- Run `wt new <name>` (modern verb) or `wt create <name>` (classic verb) inside a git repo that has a `.wtinclude`.
- Optionally pass `--base <ref>` to choose the starting ref.
- Optionally pass `--manifest <path>` to choose an explicit manifest.

## Driving it with the shell fixture

Preconditions:

- Fixture loaded (`WT_BIN`, `WT_ORIGIN`, `WT_STORE` set), cwd `$WT_ORIGIN`.
- Fixture ships `.wtinclude` containing `heavy/` and 40 files under `heavy/`.

- **Create.** `wt --json new demo --dir "$WT_FIXTURE/demo"`. Envelope
  `status` is `ok`; `command` is `new`; `data.branch` is `demo`;
  `data.files_hydrated` is `40`; `data.hydration_method` is present;
  `data.duration_ms` is present.
- **Verify branch.** `git -C "$WT_FIXTURE/demo" branch --show-current` prints
  `demo`; `git -C "$WT_ORIGIN" worktree list` shows the new worktree.
- **Verify hydration on disk.** `ls "$WT_FIXTURE/demo/heavy"` shows `pkg00`
  …`pkg19`; `cat "$WT_FIXTURE/demo/heavy/pkg00/nested/file-0.txt"` matches the
  fixture content `fake-heavy file 0 of 40`.
- **Verify isolation.** `cat "$WT_FIXTURE/demo/heavy/pkg00/nested/file-0.txt" > /dev/null` and
  a second write `echo edited > "$WT_FIXTURE/demo/heavy/pkg00/nested/file-0.txt"` succeeds
  (hydrated files are private and writable; restore by recreating the fixture
  or use a fresh name).
- **Manifest override.** `wt --json create alt --dir "$WT_FIXTURE/alt" --manifest /dev/null`
  reports `data.files_hydrated` of `0` and `data.hydration_method` of `none`.
  This proves that the manifest, not gitignore, drives hydration.
- **Proof.** Save both envelopes plus `ls -R` of the hydrated tree and the
  `find "$WT_STORE" -maxdepth 2` listing to `artifacts/verify-wt/<run-id>/`.

## Gotchas

- An absent `.wtinclude` does not fail. Instead, `wt` generates a starter
  `.wtinclude` template with default rules and hydrates any matching paths.
  Pass `--manifest /dev/null` to enforce zero hydration.
- `hydration_method` is filesystem-dependent: `byte_copy` plus a
  `CROSS_DEVICE_COPY_DEGRADATION` diagnostic on tmpfs; CoW/hardlink
  elsewhere. Do not assert a specific method unless you control the fs.
- The default worktree destination is a sibling of the repo. Always pass
  `--dir` into the fixture, or proofs litter the real filesystem.
- Second create with the same name fails; use a new name or remove first.
