# 04 - Incremental rebuild guard

Status: ready-for-agent

## Problem

Incremental snapshot rebuild can cost more than a warm full clone when the diff is wide.

## Solution

Add a diff size guard in the projection path. Past 10 percent changed entries, or on lockfile miss, take the full clone path.

## Acceptance

- Wide diff fixture hydrates in near warm clone time.
- Narrow diff still takes the incremental path.
- Hit rate is visible in JSON output.

## Verification

- Snapshot CLI tests plus `WT_TIMING=1` comparison on both fixtures.

## Comments

- Context: snapshot projection plus snapdiff selection.
- Builds on ticket 05 in `.scratch/deep-hydration-architecture`, which returns Manifest directly from ingest and removes the travelling params helper. Write the guard against that shape, not the current bridge.
