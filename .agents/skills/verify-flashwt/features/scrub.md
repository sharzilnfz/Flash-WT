# Scrub the store

`flashwt scrub` re-hashes every blob against its content address and purges
corruption, closing the trust-model gap where a bit flip that preserves size
and mtime slips past the verified-blob ledger.

## Sub-features

- `scrub-detect` finds blobs that no longer match their address.
- `scrub-dry-run` reports corruption without deleting anything.
- `scrub-repair` deletes corrupt blobs and purges broken snapshot directories.
- `scrub-snapshots` audits published snapshot directories and complete markers.

## How to get to it (user POV)

- Run `flashwt scrub --dry-run` to audit the store read-only.
- Run `flashwt scrub` to repair (corrupt blobs and broken snapshot directories are deleted).

## Driving it with the shell fixture

Preconditions:

- Fixture loaded, cwd `$FLASHWT_ORIGIN`.
- `FLASHWT_SNAPSHOTS=0` exported so verification isolates blob re-ingestion from snapshot caches.
- A store with at least one blob: `flashwt --json create demo --dir "$FLASHWT_FIXTURE/demo"`
  returned `ok` with `files_hydrated` > 0, then
  `flashwt --json remove demo --dir "$FLASHWT_FIXTURE/demo"`.

- **Corrupt a blob.** Store blobs are written with read-only permissions (`0444`). Add write permission before modifying:

  ```sh
  BLOB=$(find "$FLASHWT_STORE/objects" -type f | head -1)
  chmod u+w "$BLOB"
  printf 'CORRUPTED!!' > "$BLOB"
  ```

- **Dry run.** `flashwt --json scrub --dry-run`. Envelope `status` is `ok`;
  `data.dry_run` is `true`; `data.scanned` reports total blobs inspected;
  `data.corrupt` lists the blob full content address; `data.deleted` is `0`.
  When corrupted items are found, `diagnostics` contains warnings with codes
  `CORRUPT_BLOB` or `CORRUPT_SNAPSHOT`.
- **Repair.** `flashwt --json scrub`. Same `corrupt` list; `data.deleted` is `1`.
  Any corrupted snapshot directories report in `data.snapshot_dirs_deleted`.
- **Verify re-ingestion heals.** `flashwt --json new demo2 --dir "$FLASHWT_FIXTURE/demo2"`
  returns `ok` and `cat "$FLASHWT_FIXTURE/demo2/heavy/pkg00/nested/file-0.txt"`
  matches fixture content. The corrupt blob was removed, so re-ingestion restored clean bytes.
- **Proof.** Save both scrub envelopes and the healed file read to
  `artifacts/verify-flashwt/<run-id>/`.

## Gotchas

- `data.corrupt` holds full 64-character addresses while objects live in
  two-character sharded directories like `objects/ee/5f87...`. Join prefix and
  filename when matching.
- Store blobs are written read-only (`0444`). Shell scripts that attempt to overwrite a blob without running `chmod u+w` first will fail with Permission Denied.
- Snapshot directories are rebuildable caches: corrupt snapshot directories are purged via directory removal, not repaired in place.
- On macOS APFS, `flashwt create` hits existing directory snapshots if present. When proving blob healing after scrub, pass `FLASHWT_SNAPSHOTS=0` or corrupt the snapshot marker as well.
- Only corrupt blobs in the fixture store. Corrupting the machine store causes data loss.
