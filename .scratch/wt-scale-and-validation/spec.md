# Spec: wt scale and validation

## Problem Statement

`wt` makes a fresh worktree with heavy directories hydrated in about 1.3 seconds on large trees. On tiny trees it loses to plain copy. On Linux it falls back silently. Agents and humans cannot inspect resolved config, store size, or JSON stability from one place. The product promise in CONTEXT.md names a watcher/sync layer that does not exist yet. The question behind this spec is whether `wt` is good, what it is and is not, and what must change for millions of daily users and agent fleets.

## Solution

Keep the Store as truth and the Tree as projection. Remove the fixed floor on the warm path. Fix the two correctness gaps first. Add the observability and contract work that lets agents drive `wt` without parsing human text. Prove each win with the existing verify rig before the next change.

## User Stories

1. As a macOS developer, I want a feature worktree with hydrated dependencies in near clone time, so that I start work without reinstalling.
2. As a Linux developer, I want an honest diagnostic when copy acceleration is unavailable, so that slow runs have a reason attached.
3. As a developer on a tiny repo, I want tiny creates to bypass the Store, so that I never wait 1.3 seconds for 200 files.
4. As a developer on a large monorepo, I want snapshot hits to stay one cheap clone, so that warm creates stay flat as file count grows.
5. As a developer editing deps, I want incremental rebuilds to fall back to full clone past a diff threshold, so that small edits stay fast and large edits never take the slow path.
6. As a human running many worktrees, I want store size plus per worktree savings in one view, so that disk pressure never surprises me.
7. As a human, I want `wt doctor` to print resolved config plus filesystem probe results, so that env mistakes surface in one command.
8. As a human, I want zero manifest onboarding with a working default, so that first run succeeds before I read docs.
9. As an agent orchestrator, I want stable JSON with error codes plus receipt files, so that crashed runs resume from the receipt instead of reparsing text.
10. As an agent orchestrator, I want lease show plus sweep dry run, so that reaping idle work is a query followed by a delete.
11. As a repo owner, I want lockfile keyed sharing to refuse mismatched deps loudly, so that a branch never silently gets wrong dependencies.
12. As a maintainer, I want GC to converge with grace plus dry run signals, so that crash safety stays and disk growth stays visible.
13. As a contributor, I want the perf floor attributed by stage timing, so that the next optimization targets the measured bottleneck.

## Implementation Decisions

- Seam choice: reuse the existing HydrationEngine seam for create plus hydrate plus scratch. Reuse the Store seam for ingest plus snapshot publish plus projection. Reuse the Materializer seam for per file placement. Reuse the WorkspaceEngine seam for git calls. No new top level seams.
- Ingest becomes parallel with streaming hash. Cache lookup stays serial. Durability batches so many blobs share one directory sync.
- Tiny bypass is a policy above the Store seam, keyed on file count plus byte total. It never writes to the Store.
- Incremental rebuild gets a diff size guard. Past threshold it takes the full clone path. The threshold ships as a constant with a timing test.
- Git subprocesses coalesce. Rev parse results cache per process. Worktree creation stays a real git call. No reimplementation of git semantics.
- Validation cache key gains inode plus ctime alongside size plus mtime. Near now mtimes rehash. The verified ledger stays as is because Store blobs are immutable and scrub covers bit rot.
- Mirror cutover keeps writing refs for snapshot members until no legacy sweeper remains, or gates the switch on audit parity for snapshot children.
- Onboarding ships zero manifest default plus demo as trial path plus a no savings warning.
- Linux fallback prints mechanism plus reason in human output. Per OS speedup numbers publish with the release notes.
- Doctor plus store du plus sweep dry run are additive commands. They read existing state. They change no lifecycle.
- Envelope v1 freezes. Only optional additive fields change. A published schema file plus changelog guards the contract. Completions stays human only.
- This spec respects ADR-0001 store as truth, ADR-0004 mark and sweep GC, ADR-0005 snapshots as cache. It conflicts with nothing. It defers the ADR-0003 watcher daemon until the explicit path is fast and observable.

## Testing Decisions

- Good tests assert external behavior: wall time bands, byte identical trees, isolation of mutations, JSON contract shape, GC convergence after kill.
- Target the HydrationEngine seam with cold versus warm versus incremental fixtures around 2k files for speed and 40k files for scale shape.
- Target the Store seam with fast rewrite fixtures for the validation fix and mixed lockfile fixtures for sharing refusal.
- Target the CLI seam with JSON golden tests for doctor plus receipt plus lease show.
- Prior art: the five verify suites under scripts/verify plus the unit suites run with cargo test --lib. Timing breakdowns use WT_TIMING=1 stage lines.

## Out of Scope

- Watcher daemon or background sync. Docs get fixed to stop promising it. A dry run log command is the most allowed.
- Runtime isolation: ports, databases, containers, sandboxes. `wt` owns source state plus heavy files, not processes or networks.
- Package manager replacement. `wt` hydrates whole directories. It never resolves versions.
- Hash algorithm migration away from SHA-256. Cost dwarfs gain.
- Windows native support beyond current fallback.

## Further Notes

- Market context lives in the validation verdict attached to this conversation: native agent worktree flags from Claude Code plus Cursor plus Codex, plus CoW rivals cow plus mantle plus husk plus grove plus rift plus hz, all racing the same per worktree install tax. `wt` wins only if the Store plus snapshot plus GC story stays crash safe and observable across macOS and Linux.
- Throughput checkpoint: n/a, read-only investigation for the analysis phase. Build phases that follow use per ticket verification.
