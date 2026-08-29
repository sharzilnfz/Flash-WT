# Scrub the store

`wt scrub` re-hashes every blob against its content address and repairs
corruption, closing the trust-model gap where a bit flip that preserves size
and mtime slips past the verified-blob ledger.

## Sub-features

- `scrub-detect` finds blobs that no longer match their address.
- `scrub-dry-run` reports corruption without deleting anything.
- `scrub-repair` deletes corrupt blobs so the next create re-ingests good bytes.

## How to get to it (user POV)

- Run `wt scrub --dry-run` to audit the store read-only.
- Run `wt scrub` to repair (corrupt blobs are deleted).

## Driving it with the shell fixture

Preconditions:

- Fixture loaded, cwd `$WT_ORIGIN`.
- A store with at least one blob: `wt --json create demo --dir "$WT_FIXTURE/demo"`
  returned `ok` with `files_hydrated` > 0, then
  `wt --json remove demo --dir "$WT_FIXTURE/demo"`.

- **Corrupt a blob.** Pick one object and overwrite it:

  ```sh
  BLOB=$(find "$WT_STORE/objects" -type f | head -1)
  printf 'CORRUPTED!!' > "$BLOB"
  ```

- **Dry run.** `wt --json scrub --dry-run`. Envelope `status` is `ok`;
  `data.dry_run` is `true`; `data.corrupt` lists the blob's full content
  address (dir prefix + filename, e.g. `ee5f87…`); `data.deleted` is `0`.
- **Repair.** `wt --json scrub`. Same `corrupt` list; `data.deleted` is `1`.
- **Verify re-ingestion heals.** `wt --json create demo2 --dir "$WT_FIXTURE/demo2"`
  returns `ok` and
  `cat "$WT_FIXTURE/demo2/heavy/pkg00/nested/file-0.txt"` matches the fixture
  content — the corrupt blob was gone, so create re-ingested from source.
- **Proof.** Save both scrub envelopes and the healed file read to
  `artifacts/verify-wt/<run-id>/`.

## Gotchas

- `data.corrupt` holds full 64-char addresses while the objects live in
  two-char-sharded directories (`objects/ee/5f87…`); join prefix + filename
  when matching. `find -type f` handles it either way.
- Dry run deletes nothing and does not touch the verified-blob ledger — do not
  use it as repair evidence.
- Only corrupt blobs in the fixture store. Corrupting the machine store is
  irreversible data loss.
