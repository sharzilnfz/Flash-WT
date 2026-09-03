# 01 - Parallel streaming ingest

Status: ready-for-agent

## Problem

Cold ingest reads each file fully into memory and hashes serially, so large trees pay one slow pass plus per blob sync costs.

## Solution

Hash in streaming chunks on a small thread pool. Keep cache lookup serial. Keep Store as truth and Tree as projection vocabulary.

## Acceptance

- Cold ingest on the standard 2k fixture is faster with identical blob ids.
- Large files never hold full contents in memory.
- No change to manifest bytes for identical inputs.

## Verification

- `cargo test -p flashwt-store --lib`
- Time cold ingest before and after with `FLASHWT_TIMING=1`.

## Comments

- Context: crates/flashwt-store ingest plus disk put path.
- Builds on ticket 05 in `.scratch/deep-hydration-architecture`, which restores parallel verify inside the batch. This ticket is the sibling pass over ingest hashing. Land that first, then this, so measurements isolate.
