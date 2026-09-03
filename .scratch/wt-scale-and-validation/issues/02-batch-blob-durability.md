# 02 - Batch blob durability

Status: ready-for-agent

## Problem

Every new blob pays a file sync plus a parent directory sync, which dominates cold creates with many unique files.

## Solution

Write new blobs, sync files in one pass, then sync each touched shard directory once. Rename still follows file sync.

## Acceptance

- Cold create with 4k unique files is faster.
- Kill between write and sync leaves only reclaimable cache, never partial truth.

## Verification

- `cargo test -p wt-store --lib`
- Crash recovery suite passes.

## Comments

- Context: DiskStore put path. Tests already bypass hardware sync with WT_TEST_NO_SYNC=1.
