# Flash snapshots and incremental rebuilds

Whole-directory snapshot caching (v1) and diff-based incremental rebuilds (v2) leverage macOS APFS `clonefile(2)` to hydrate entire heavy directory trees in milliseconds and incrementally project directory changes without re-linking from scratch.

## Sub-features

- `snapshots-v1-cache` caches whole heavy directory trees under `$WT_STORE/snapshots/<hash>/tree` for single-syscall `clonefile(2)` hydration.
- `snapshots-v2-incremental` diffs canonical manifests against existing base snapshots to construct new snapshots by cloning unchanged subtrees and linking only modified/added blobs (`WT_SNAPSHOTS_V2=1`).
- `snapshots-apfs-auto` enables snapshot acceleration automatically on macOS APFS volumes without requiring explicit environment variables.
- `snapshots-opt-out` disables snapshot fast paths via `WT_SNAPSHOTS=0`, falling back to per-file hydration.
- `snapshots-paranoid-bypass` forces full SHA-256 verification of every individual blob via `WT_VERIFY=1`, bypassing snapshot hits.

## How to get to it (user POV)

- On macOS APFS, `wt new` automatically uses snapshot caching and incremental rebuilds by default.
- Set `WT_SNAPSHOTS_V2=1` to ensure v2 incremental diff rebuilds are active.
- Set `WT_SNAPSHOTS=0` to force per-file hydration.
- Set `WT_VERIFY=1` to force re-verification of all blobs, bypassing cached snapshot hits.

## Driving it with the shell fixture

Preconditions:

- Fixture loaded on macOS APFS volume (`WT_BIN`, `WT_ORIGIN`, `WT_STORE` set).
- Fixture contains heavy directory with subdirectories and files.

- **First create builds snapshot.** `wt --json new snap1 --dir "$WT_FIXTURE/snap1"`. Envelope reports `data.files_hydrated` of `40`; on APFS, snapshot directory is published under `$WT_STORE/snapshots/<hash>/` containing `manifest.tsv`, `.complete`, and `tree/`.
- **Second create hits snapshot cache.** `wt --json new snap2 --dir "$WT_FIXTURE/snap2"`. Envelope reports `status` is `ok`, `data.cache_hit` is `true` (or `data.hydration_method` is `"snapshot"`), completing in single-digit milliseconds.
- **Incremental rebuild (v2).** Modify one file in the origin repo: `echo "modified" >> "$WT_ORIGIN/heavy/pkg00/nested/file-0.txt"`. Run `wt --json new snap3 --dir "$WT_FIXTURE/snap3"`. With `WT_SNAPSHOTS_V2=1`, `wt` computes the manifest diff against `snap1`, clones the unchanged `pkg01`..`pkg19` subtrees as whole units, links the modified blob, and publishes the new snapshot.
- **Opt-out verification.** `WT_SNAPSHOTS=0 wt --json new snap-nosnap --dir "$WT_FIXTURE/snap-nosnap"` reports per-file hydration without snapshot hits.
- **Paranoid verification.** `WT_VERIFY=1 wt --json new snap-verify --dir "$WT_FIXTURE/snap-verify"` bypasses snapshot hits and re-hashes every blob.
- **Proof.** Save envelopes and `ls -R "$WT_STORE/snapshots"` to `artifacts/verify-wt/<run-id>/`.

## Gotchas

- Snapshot caching requires filesystem reflink/clonefile support (macOS APFS). On Linux or non-APFS volumes, `wt` falls back to per-file hydration.
- Snapshots are rebuildable caches, not GC roots: `wt sweep` reclaims unreferenced snapshots after the grace period or retention cap (`WT_SNAPSHOT_CAP`).
- Corrupted snapshot directories or missing `.complete` markers are identified as debris by `wt scrub` and cleaned up.
