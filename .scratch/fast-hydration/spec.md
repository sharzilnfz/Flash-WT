# Spec: fast hydration

Status: ready-for-agent

## Problem Statement

`flashwt create` is slower than the thing it replaces. The public benchmark shows
3.79s cold and 3.26s warm against a 1.69s baseline of plain `git worktree add`
plus a fresh install. A developer or agent reaching for `flashwt` today pays a tax
on every worktree, on the promise of speed that the tool does not yet deliver.

Two causes, both measured:

1. Every run re-reads and re-hashes every file in every heavy directory,
   even when nothing changed since the previous run. Warm runs should cost
   almost nothing; they currently cost nearly as much as cold ones.
2. Materialization hard-links store blobs and strips their write bits. The
   shared inode is safe but changes file semantics: tools that rewrite files
   in place fail loudly, and hydrated trees behave differently from normal
   checkouts.

Separately, the benchmark suite cannot answer the main architectural
question. It compares `flashwt` against worktree-plus-install, but never against
the simplest alternative: a direct recursive APFS clone from an existing
tree. Until that number exists, nobody can say whether the content-addressed
store earns its place in the hot path.

## Solution

Make hydration faster than the baseline on both cold and warm paths, and
make hydrated trees behave like normal writable checkouts.

- Add a direct-CoW scenario to the public benchmark so the store has to beat
  (or justify itself against) the simplest alternative.
- Cache validation state per ingested file so warm ingest skips reading and
  hashing unchanged content.
- Materialize via copy-on-write clones where the platform supports it
  (`fclonefileat` on macOS), producing private, fully writable files that
  share blocks until first write. Hardlinks stay available behind a flag as
  an experimental mode.
- Bring the feature spec's copy-strategy language back in line with what
  actually ships.

## User Stories

1. As an agent creating many worktrees across a fleet, I want warm
   `flashwt create` to complete well under a second, so that spinning up ten
   parallel agents does not serialize on filesystem work.
2. As a developer whose teammate just ran `npm install`, I want my next
   `flashwt create` to skip re-hashing unchanged files, so that hydration cost
   scales with what changed rather than with tree size.
3. As a developer running `cargo clean` in my main checkout, I want the
   next worktree to hydrate anyway, because the store still holds the
   artifacts.
4. As a developer running builds inside a hydrated worktree, I want
   hydrated files to be private and writable like any normal checkout, so
   that compilers and package managers work without surprises.
5. As a user of build tools that rewrite files in place, I want such writes
   to succeed silently and privately, instead of failing with permission
   errors.
6. As a maintainer of the benchmark suite, I want a direct-CoW clone
   scenario alongside worktree-plus-install and store hydration, so that
   architecture debates are settled by numbers.
7. As a potential contributor evaluating this project, I want published
   benchmarks to include the strongest simple alternative, so that I can
   trust the performance claims.
8. As a CI runner, I want the new benchmark scenario to byte-verify its
   output like every other scenario, so that a fast-but-wrong result can
    never ship.
9. As a user on a non-APFS filesystem, I want materialization to fall back
   gracefully (reflink where verified, then byte copies), so that `flashwt`
   works everywhere even if CoW is unavailable.
10. As a cautious user, I want an escape hatch that forces byte-copy
    materialization, so that exotic filesystem behavior can always be
    ruled out.
11. As a power user who prefers the old behavior, I want to opt into
    hardlinked materialization explicitly, so that maximum space sharing
    remains available while knowing it is experimental.
12. As a reader of the project docs, I want the spec's copy-strategy section
    to describe the shipped strategy, so that the documents and the binary
    tell the same story.
13. As a user upgrading an existing store, I want previously ingested blobs
    to remain valid, so that the cache lands without forcing a full
    re-ingest.
14. As a user who edits a heavy file between worktree creations, I want the
    changed file's new content in the next hydrated tree, so that staleness
    never silently spreads.
15. As a user whose file was touched (mtime bumped) without content change,
    I want hydration to stay correct, so that cache shortcuts cannot serve
    wrong bytes.
16. As someone auditing disk usage, I want CoW-materialized trees to keep
    sharing physical blocks before first write, so that the deduplication
    promise survives the change in linking strategy.
17. As a garbage-collection user, I want reference counting to keep working
    unchanged under the new materialization path, so that sweeps neither
    leak nor over-delete.

## Implementation Decisions

- Ingest gains a validation cache keyed by repo-relative path, recording
  size, mtime, and content id from the previous run. A file whose current
  size and mtime match the cache keeps its existing blob; everything else
  is read and hashed as today. The cache lives beside the store, not inside
  it, so store format stays untouched and old stores keep working.
- Because mtime alone is forgeable, a cache hit is trusted but the existing
  hash verification at materialize time remains the safety net: if cached
  metadata ever lies, the mismatch surfaces at `Store::get` exactly as it
  does today.
- Materialization routes through the existing copy-backend trait rather
  than hardcoding links. macOS uses `fclonefileat` per file from the store
  blob to the destination, giving a fresh private inode with shared blocks.
  Filesystems that refuse the call fall back to hardlink (opt-in) and then
  byte copy, in that order.
- Hydrated files produced by CoW materialization carry normal writable
  permissions. No write bits are stripped; divergence happens through the
  filesystem's own copy-on-write.
- The hardlink path stays in the codebase behind an explicit opt-in flag
  and is documented as experimental. It is no longer the default.
- The default backend order becomes: CoW clone where supported, byte copy
  otherwise. Reflink on Linux joins the front of that order once Linux CI
  validates it; until then Linux gets byte copies.
- `benchmarks/run.sh` gains a third scenario: recursive direct CoW clone
  from the source tree into the destination (macOS `clonefile`, equivalent
  mechanism elsewhere). It runs under the same timing, verification, and
  reporting rules as the existing scenarios, and appears in the results
  table.
- Benchmark reporting adds raw physical-disk usage per hydrated tree
  alongside wall time, since block-sharing claims must be measurable, not
  asserted.
- The feature spec's copy-strategy lines are rewritten to describe the
  shipped order (CoW clone, byte-copy fallback, experimental hardlink), and
  the whole-directory-clonefile phrasing is corrected to name the actual
  mechanism.

## Testing Decisions

Tests assert external behavior only: bytes in trees, permissions on files,
counts in reports, timings in the benchmark table. Nothing inspects internal
cache structures or backend dispatch.

- Existing flashwt-cli e2e seams (`FLASHWT_STORE` isolation, temp fixtures) cover:
  hydration after source edits reflects the edits; hydration after touching
  a file without editing it still produces correct bytes; hydrated files
  are writable and st_nlink-independent under the default path; a second
  create adds no new store content (dedup preserved); GC references still
  release cleanly after removal.
- Store unit tests extend the existing suite for the validation cache:
  hit on unchanged file, miss on content change, miss on size change, miss
  on mtime change with same size, correct behavior with a pre-existing
  store that has no cache yet.
- The benchmark suite is its own seam: the new scenario must pass the same
  `--verify` byte comparison as the others, and `--quick` mode must include
  it so CI exercises all three.
- Prior art: `crates/flashwt-cli/tests/store_flow.rs` for dedup and corruption
  assertions, `cli.rs` for output contracts, and the hardlink torture tests
  for permission-behavior patterns.

## Out of Scope

- Artifact validity policy (per-directory reuse/warn/forbid rules). Deferred
  until the performance architecture settles; it may be written against a
  different design afterward.
- GC crash-recovery torture tests (kill during ingest, concurrent sweep,
  ledger corruption). Real concerns, wrong moment; revisit once the hot
  path is settled.
- Linux reflink enablement and Linux CI. Blocked on real filesystem
  validation.
- Watcher/sync layer, cross-machine store sharing, and any mutation
  semantics beyond one-shot hydration.
- Removing the content-addressed store from the architecture. This spec
  makes it competitive or the benchmark will say otherwise, but deletion is
  not on the table here.

## Further Notes

The architectural question this spec answers: does the store earn its place
in the hot path? The direct-CoW benchmark scenario exists specifically to
answer it. If direct cloning beats optimized store hydration decisively on
both cold and warm runs, that is a signal to revisit, not to bury. If the
store wins on warm runs (plausible, since a validated cache reads zero file
bytes while a clone still copies metadata for the whole tree), the store's
remaining advantages — survival of source-tree cleans, integrity checks,
GC — come for free.

`fclonefileat` clones a single existing file's contents into a new file on
the same volume. Unlike directory `clonefile(2)` it composes with the
store's per-blob layout, which is what makes the middle path possible:
CAS bookkeeping above, CoW physics below.
