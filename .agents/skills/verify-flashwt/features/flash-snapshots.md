# Flash snapshots and incremental rebuilds

Whole-directory snapshot caching (v1) and diff-based incremental rebuilds (v2) leverage macOS APFS `clonefile` to hydrate entire heavy directory trees in milliseconds and incrementally project directory changes.

## Sub-features

- `snapshots-v1-cache` caches whole heavy directory trees under `$FLASHWT_STORE/snapshots/<hash>/tree` for single-syscall `clonefile` hydration.
- `snapshots-v2-incremental` diffs canonical manifests against existing base snapshots to clone unchanged directory bases and link modified blobs.
- `snapshots-apfs-auto` enables snapshot acceleration automatically on macOS APFS volumes without requiring explicit environment variables.
- `snapshots-opt-out` disables snapshot fast paths via `FLASHWT_SNAPSHOTS=0`, falling back to per-file hydration.
- `snapshots-paranoid-bypass` forces full SHA-256 verification of every individual blob via `FLASHWT_VERIFY=1`, bypassing instant cache hits while still generating verified snapshots.

## How to get to it (user POV)

- On macOS APFS, `flashwt new` automatically uses snapshot caching and incremental rebuilds by default.
- Set `FLASHWT_SNAPSHOTS=0` to force per-file hydration.
- Set `FLASHWT_VERIFY=1` to force re-verification of all blobs.

## Driving it with the shell fixture

Preconditions:

- Fixture loaded on macOS APFS volume (`FLASHWT_BIN`, `FLASHWT_ORIGIN`, `FLASHWT_STORE` set).
- Fixture contains heavy directory with subdirectories and files.

- **First create builds snapshot.** `flashwt --json new snap1 --dir "$FLASHWT_FIXTURE/snap1"`.
  Envelope reports `status` is `ok`; `data.cache_hit` is `false`; `data.files_hydrated` is `40`;
  snapshot directory is published under `$FLASHWT_STORE/snapshots/<hash>/` containing `manifest.tsv`,
  `.complete` (format `v1\t<hash>\n`), and `tree/`.
- **Second create hits snapshot cache.** `flashwt --json new snap2 --dir "$FLASHWT_FIXTURE/snap2"`.
  Envelope reports `status` is `ok`; `data.cache_hit` is `true`; `data.hydration_method` is `"clone"`;
  duration is under 50 milliseconds.
- **Incremental rebuild (v2).** Modify one file in the origin repo (staying below the 10% diff ceiling):
  `echo "modified" >> "$FLASHWT_ORIGIN/heavy/pkg00/nested/file-0.txt"`.
  Run `flashwt --json new snap3 --dir "$FLASHWT_FIXTURE/snap3"`.
  `flashwt` clones the previous snapshot tree base as a single unit, applies the diff, and reports
  `data.incremental_decision` of `"hit"` alongside `data.incremental_hit_rate`.
- **Opt-out verification.** `FLASHWT_SNAPSHOTS=0 flashwt --json new snap-nosnap --dir "$FLASHWT_FIXTURE/snap-nosnap"`
  reports per-file hydration (`data.cache_hit: false`).
- **Paranoid verification.** `FLASHWT_VERIFY=1 flashwt --json new snap-verify --dir "$FLASHWT_FIXTURE/snap-verify"`
  bypasses instant cache hits and re-hashes every blob before snapshot materialization.
- **Proof.** Save envelopes and `ls -R "$FLASHWT_STORE/snapshots"` to `artifacts/verify-flashwt/<run-id>/`.

## Gotchas

- Snapshot hits emit `data.hydration_method: "clone"`, never `"snapshot"`.
- `data.cache_hit` is false on initial snapshot generation and true on subsequent hits.
- Incremental rebuilds require matching lockfile hashes and an overall difference ratio at or below 10%. Exceeding 10% diff triggers full snapshot builds.
- Snapshot caching requires filesystem clonefile support (macOS APFS). On Linux or non-APFS volumes, `flashwt` falls back to per-file hydration.
- Snapshots are rebuildable caches: `flashwt sweep` evicts unreferenced snapshots exceeding retention caps (`FLASHWT_SNAPSHOT_CAP`, `FLASHWT_MAX_SNAPSHOT_BYTES`).
