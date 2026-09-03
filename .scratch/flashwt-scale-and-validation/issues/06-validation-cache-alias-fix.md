# 06 - Validation cache alias fix

Status: ready-for-agent

## Problem

The ingest validation cache trusts size plus mtime. A same size rewrite within one mtime tick reuses the old blob id and hydrates stale content.

## Solution

Mix inode plus ctime into the cache key. Rehash on hit when mtime is near now. Correctness before speed.

## Acceptance

- Fast rewrite fixture with identical size and mtime hydrates fresh content.
- Normal warm path still skips hashing.

## Verification

- New regression test with forced mtime collision plus `cargo test -p flashwt-store --lib`.

## Comments

- The verified ledger needs no change. Store blobs are immutable and scrub covers bit rot.
- Sibling of the ticket 05 dir mode strictness item in `.scratch/deep-hydration-architecture`. That one covers missing modes. This one covers stale hits. Land both; neither duplicates the other.
