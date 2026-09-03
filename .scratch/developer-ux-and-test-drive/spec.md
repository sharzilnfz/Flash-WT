# Spec: Seamless Developer UX & Zero-Setup Test Drive

Status: ready-for-agent

## Problem Statement

Evaluating, testing, and adopting `flashwt` is currently burdened by steep operational and cognitive overhead:
- **High Setup Friction for Testing**: A developer or tester must find or assemble a massive 40,000-file repository with heavy `node_modules` just to observe the performance benefits. Testing on a clean or small repository makes `flashwt` appear slow (~1.5s overhead for trivial worktrees).
- **Hidden Performance Flags**: The flagship performance features (directory snapshots and v2 incremental rebuilds) are hidden behind environment variables (`FLASHWT_SNAPSHOTS=1`, `FLASHWT_SNAPSHOTS_V2=1`) instead of being enabled automatically on supported platforms (APFS on macOS).
- **Leaked Internal Mechanics**: CLI help text and stdout messages present low-level storage implementation details (mirror TSVs, hash verification ledgers, legacy ref migration flags) rather than high-level developer outcomes.
- **Missing Core Workflow Verbs**: There is no built-in `flashwt list` command to inspect active worktrees and disk savings, no copy-pasteable next-action prompt after creation, and no interactive batch cleanup for merged or stale branches.

## Solution

Redesign the front-door developer experience around three core principles:
1. **Zero-Setup Test Drive (`flashwt demo`)**: A single self-contained command that synthesizes an isolated fixture, benchmarks standard copies against `flashwt` copy-on-write hydration, validates file isolation, cleans up automatically, and renders a visual terminal scorecard in five seconds.
2. **The 3-Verb Core Workflow (`flashwt new`, `flashwt list`, `flashwt clean`)**: Standardize daily usage around intuitive verbs, format crisp terminal receipts with actionable `cd` hints, and calculate real disk space savings across active worktrees.
3. **Smart Platform Defaults**: Automatically detect APFS on macOS and enable whole-directory snapshots and diff-based rebuilds out of the box with graceful fallbacks.
4. **Interactive Multi-Select Cleanup**: Provide interactive terminal selection for `flashwt clean` to prune merged worktrees and reclaim unreferenced store storage in one action.

## User Stories

1. As a developer evaluating `flashwt` for the first time, I want to run `flashwt demo`, so that I can see hydration speed, disk deduplication, and CoW isolation verified live in five seconds without preparing test repositories.
2. As a team lead onboarding teammates, I want a 2-minute quick-start workflow, so that any engineer can immediately understand and use `flashwt` without reading storage architecture documentation.
3. As a developer using `flashwt` on macOS, I want directory snapshots and incremental diff rebuilds enabled by default on APFS, so that I get maximum performance without exporting environment variables.
4. As a developer working across multiple branches, I want to run `flashwt list`, so that I can see all active worktrees, their branch names, paths, hydrated directory sizes, and disk space saved.
5. As an AI agent or CI pipeline, I want `flashwt list --json` to output a structured JSON envelope, so that automation can inspect active worktrees and storage utilization reliably.
6. As a developer creating a worktree with `flashwt new my-feature`, I want a clean terminal receipt with a copy-pasteable `cd` command, so that I know exactly where my worktree is and how to enter it.
7. As a developer finishing a batch of tasks, I want to run `flashwt clean` to interactively select and delete merged worktrees, so that I can tidy my workspace and reclaim disk space in one step.
8. As a developer removing a specific worktree with `flashwt clean my-feature`, I want store references to be released and unreferenced data swept automatically, so that storage does not accumulate silently.
9. As a developer on Linux or non-APFS storage, I want `flashwt` to automatically fall back to supported copy acceleration mechanisms, so that my worktrees hydrate reliably without runtime errors.
10. As a developer testing file isolation, I want `flashwt demo` to verify that edits inside a hydrated worktree never mutate source files in the store, so that I have complete confidence in copy-on-write safety.
11. As a developer reading `flashwt --help`, I want concise, task-focused descriptions instead of implementation trivia, so that I can quickly find the command I need.
12. As an existing `flashwt` user, I want legacy commands (`flashwt create`, `flashwt remove`, `flashwt sweep`) to remain fully supported as backward-compatible aliases, so that existing scripts and habits do not break.

## Implementation Decisions

### Decision 1: Built-In Test Drive Engine (`flashwt demo`)
- Add a top-level `flashwt demo` (and `flashwt test-drive`) command.
- The command creates a temporary directory structure under the system temp path, builds a realistic multi-package synthetic directory tree (10,000 files across nested subpackages), and runs an end-to-end benchmark.
- Stages measured:
  1. Synthetic fixture generation.
  2. Standard filesystem copy baseline.
  3. `flashwt` copy-on-write hydration.
  4. Copy-on-write mutation isolation test (modifying a hydrated file and asserting donor blob integrity).
  5. Worktree removal and storage sweep.
- Output renders a formatted terminal comparison table with visual progress indicators and timing/storage bars. Supports `--json` for automated verification.

### Decision 2: Worktree Discovery & Disk Accounting (`flashwt list`)
- Add a top-level `flashwt list` (and `flashwt ls`) command.
- The command scans the git repository's worktrees using git worktree metadata, correlates each worktree with its `flashwt-hydrated.tsv` sidecar ledger and store mirrors, and computes:
  - Worktree directory path and branch name.
  - Hydrated directory names and file counts.
  - Disk bytes saved through copy-on-write deduplication against store blobs.
  - Ephemeral lease status and TTL (for scratch/isolate worktrees).
  - Age / creation timestamp.
- Emits a clean aligned terminal table for human users and a versioned envelope for `--json`.

### Decision 3: 3-Verb Workflow & Human-Centric Terminal Receipts
- Introduce `flashwt new <name>` as the primary creation verb (aliasing `flashwt create`).
- Introduce `flashwt clean [name]` as the primary removal and reclamation verb (aliasing `flashwt remove` + `flashwt sweep`).
- Consolidate `flashwt isolate` as a transparent alias to `flashwt scratch`.
- Replace raw log statements with structured terminal receipts:
  - Formatted glyphs (`✓`).
  - Humanized file counts and byte sizes (e.g. `41.8k files`, `184 MB`).
  - Distinct elapsed timings.
  - Actionable next steps (`cd ../<path>`).

### Decision 4: Automatic APFS & Diff-Rebuild Runtime Detection
- Update configuration defaults so that on macOS with APFS filesystems, snapshot hydration (`FLASHWT_SNAPSHOTS`) and v2 incremental diff rebuilds (`FLASHWT_SNAPSHOTS_V2`) default to active (`true`).
- Environment variables `FLASHWT_SNAPSHOTS=0` and `FLASHWT_SNAPSHOTS_V2=0` act as explicit opt-out toggles.
- Systems on Linux or non-APFS volumes automatically fall back to per-file cloning / reflink / byte copies without warnings.

### Decision 5: Interactive Worktree Cleanup Prompt
- When `flashwt clean` is invoked without arguments in an interactive terminal (TTY), present a terminal checkbox list of active worktrees.
- Identify and badge worktrees whose branches are already merged into the current default branch (e.g. `merged into main`).
- On confirmation, remove selected worktrees and run storage reclamation, outputting the total disk space reclaimed.
- In non-interactive environments (CI / pipes), require explicit branch names or `--all` / `--force` flags.

## Testing Decisions

- **Testing High Seams**: Test commands at the CLI binary level using `assert_cmd` and in-tree integration harnesses, asserting user-visible output, stdout envelopes, exit codes, and filesystem state.
- **Fixture-Independent Verification**: `flashwt demo` serves as both an end-user test drive and an in-tree integration test executed by `cargo test`.
- **Isolation Invariant Checks**: Integration tests explicitly modify materialized files in hydrated worktrees and verify that source store blobs and snapshot directories remain unmodified.
- **Prior Art**: Extends integration test patterns in `crates/flashwt-cli/tests/cli.rs`, `crates/flashwt-cli/tests/cache_flow.rs`, and `crates/flashwt-cli/tests/branch_stacking.rs`.

## Out of Scope

- Shell autocomplete script generators (can be added in a separate follow-up).
- GUI / desktop application wrappers (flashwt remains a single terminal binary).
- Daemon processes or persistent background workers.

## Further Notes

- Maintains complete backward compatibility: all existing flags, subcommands (`create`, `remove`, `sweep`), and JSON envelope schemas remain intact.
- Complies with all existing ADRs (ADR-0001 through ADR-0006).
