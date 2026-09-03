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

- `cargo test -p wt-store --lib`
- Time cold ingest before and after with `WT_TIMING=1`.

## Comments

- Context: crates/wt-store ingest plus disk put path.
