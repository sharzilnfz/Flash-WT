# ADR-0006: Evaluate external snapshot/copy libraries before building v2

Date: 2026-08-23
Status: accepted

An external review proposed adopting three dependencies instead of
continuing in-house work: `snapdir` (content-addressed snapshots),
`clonetree`/`parcopy` (recursive reflink copying), and pnpm-style
hardlink-count garbage collection. Each was verified against primary
sources and measured against our own benchmarks. None is adopted.

## snapdir (snapdir.org, crates.io `snapdir`)

Real and well-built (v1.10.x, BLAKE3 merkle manifests, push/pull to
S3/GCS/S3-compatible stores, verified-on-fetch). Rejected because it
solves the distribution problem, not the local hydration problem:

- Its `pull` materializes trees by writing bytes; our hot path needs
  placement through `clonefile(2)` out of store-local hardlink trees,
  which snapdir has no concept of.
- Its snapshot IDs are BLAKE3 merkle roots. Keying on them would fork
  compatibility with every existing v1 manifest-hash snapshot - the
  exact mistake the revised v2 design note rejects.
- The claimed "eliminates selection-index work" is wrong for us: their
  catalog maps snapshot-to-store-location, not heavy-directory-to-
  previous-snapshot after a dependency bump.

## clonetree / parcopy

`clonetree` (cortesi, MIT, v0.0.2) exists. On macOS/APFS its `auto`
strategy issues a single recursive `clonefile(2)` - byte-for-byte what
our `ClonefileBackend.copy_dir` already does, measured at ~0.45s for a
40k-file tree. Its `full-traversal` is a plain per-file reflink walk,
strictly below our ladder. It lacks the destination rules (empty-dir
recovery, eviction-race ENOENT retry) our integration owns. `parcopy`
targets general parallel copying; our bottleneck measurements show
per-call overhead, not copy throughput. Nothing to gain; dependency and
behavior risk only.

## pnpm-style GC by hardlink count

Verified real: zkochan confirms `pnpm store prune` deletes store files
whose link count is 1. It works for pnpm because pnpm HARDLINKS from
store into projects. Our worktree copies are `clonefile(2)` CoW clones:
new inodes sharing extents, each with link count 1 and invisible to any
count-based scheme. Current pnpm ecosystem docs concede the same limit
("on reflink filesystems such as APFS, link counts cannot prove project
reachability"). Adopting this would collect live data. Store-local
mirrors remain the correct root set for flashwt.

## What we did take from the research

- Confirmation (Bun install engineering notes) that one-syscall
  recursive `clonefile` is the right hydration primitive; our Step 0
  instrumentation proved it (~0.45s at 40k files).
- `getattrlistbulk(2)` batch walking for the ingest stage, implemented
  in Step 0 follow-up (ingest 3.3s -> 0.49s warm at 40k files).

## Consequences

- No new runtime dependencies; the snapshot/GC/diff subsystem remains
  in-house (~600 lines, 158 tests).
- Note (2026-08-29): `clap_complete` was later added for shell
  completions. It is outside this ADR's scope — the no-new-dependencies
  rule above governs the snapshot/GC/diff subsystem only.
- v2 incremental rebuilds proceed per the revised design note, with the
  unit-clone mechanism later simplified to whole-tree clone plus delta
  after benchmarks showed per-unit call overhead dominating (see
  ticket 09).
