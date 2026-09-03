# Spec: Agent Workflows, Filesystem Optimization, and Store Reliability

Status: ready-for-agent

## Problem Statement

Autonomous AI agent swarms and parallel coding agents (such as Antigravity, Claude Code, and Herdr) create and destroy dozens of git worktrees concurrently to run tests, explore branches, and execute evaluations in parallel.

When driving `flashwt` today, agents face several concrete friction points:
1. Command outputs are emitted only as unstructured human text. Agents must use fragile regular expressions to parse directory paths, strategy choices, and file counts.
2. Throwaway agent tasks require manually orchestrating worktree creation, branch naming, and cleanup. Process-local cleanup guards fail during unmaskable process termination (`SIGKILL`), terminal detach, out-of-memory kills, and machine reboots.
3. Cold snapshot creation serializes file hardlinks into single directories on a single thread. This underutilizes multi-core hardware and suffers from filesystem directory-entry lock contention.
4. Linux hydration on ext4 filesystems unconditionally fails `ioctl(FICLONE)` with `EOPNOTSUPP`. Without first-class in-kernel zero-copy handling, Linux fallback hydration incurs high user-space buffer overhead.
5. Cross-volume boundaries silently degrade fast copy-on-write clones into slow full copies on APFS and Linux without giving telemetry to agent orchestrators.
6. Toolchain directories such as python virtual environments (`.venv`) and compiler incremental caches (`target/`) contain hardcoded absolute host paths. Hydrating them into sibling worktrees breaks virtual environment activation scripts, shebangs, and compiler caches.
7. High concurrency across multiple agents running simultaneous creates causes lost update anomalies and lock contention in whole-file snapshot index and LRU metadata files.
8. Store garbage collection bounds snapshot accumulation only by integer item count rather than true disk byte budgets.
9. Store integrity scrubbing inspects only raw object blobs and ignores published snapshot trees.

## Solution

Upgrade `flashwt` with first-class agent orchestration primitives, multi-core filesystem optimizations, toolchain path relocation, concurrency-safe write-ahead metadata logs, and dual-budget snapshot lifecycle management.

Key capabilities delivered:
1. Versioned machine-readable JSON envelopes via `--json` across all commands, emitting single-line NDJSON on standard output and routing diagnostics to standard error.
2. Lease-backed ephemeral sandbox worktrees via `flashwt scratch` and `flashwt isolate`, persisting time-to-live and process start-time fingerprints in the store so garbage collection reaps orphans reliably across crashes and reboots.
3. Subtree-partitioned parallel snapshot construction using standard library scoped threads, assigning whole directory subtrees to worker threads to avoid directory lock contention.
4. Upfront filesystem capability probing that caches device identifiers and filesystem types, enabling first-class parallel `copy_file_range(2)` on ext4 while surfacing cross-volume degradation warnings in JSON diagnostics.
5. Thread-local allocation-free directory traversal in the macOS bulk walker with strict length bounds checking.
6. Automated toolchain relocation that patches `pyvenv.cfg`, `activate` scripts, and script shebangs upon `.venv/` hydration while excluding volatile machine-specific compiler caches.
7. Tiered lockfile invalidation checking lockfile hashes in O(1) time while forcing safe revalidation for mutable dependencies (such as local path, symlink, or unpinned git references).
8. Append-only write-ahead transaction log (`journal.tsv`) for snapshot metadata updates under high concurrency, compacted asynchronously during store sweep.
9. Manifest-recorded logical size accounting for dual-budget garbage collection (count cap plus disk byte limit) with anti-thrashing grace windows.
10. Sharded parallel store and snapshot scrubbing across 256 hash prefix shards, verifying published snapshot trees alongside raw blobs.
11. Symbolic branch reference tracking for stacked PR workflows with base branch movement diagnostics.

## User Stories

1. As an autonomous agent orchestrator, I want `flashwt create --json` to emit a versioned single-line JSON envelope on standard output, so that I can reliably extract the worktree path, branch, cache hit status, and duration without regex parsing.
2. As an autonomous agent orchestrator, I want `flashwt remove --json` to report released reference counts and mirror status in structured JSON, so that I can track resource deallocation programmatically.
3. As an autonomous agent orchestrator, I want `flashwt sweep --json` to output reclaimed entry counts and disk metrics, so that I can observe storage health programmatically.
4. As an autonomous agent orchestrator, I want `flashwt scrub --json` to report scanned and corrupted blob details in structured JSON, so that automated health checks can trigger automated repairs.
5. As an autonomous agent orchestrator, I want `--json` output to include diagnostic warnings when cross-volume boundaries force fallback to byte copies, so that I can detect storage misconfiguration immediately.
6. As an agent executing a single test or build task, I want `flashwt scratch --run "<command>"` to create an isolated worktree, execute the command, and register a time-to-live lease in the store, so that the worktree is cleaned up on exit and reaped by garbage collection if the process is killed with `SIGKILL`.
7. As an agent exploring an experimental patch, I want `flashwt scratch` without arguments to generate a uniquely named temporary worktree tagged for automated sweep, so that I do not have to manage branch naming collisions.
8. As a developer stacking feature branches, I want `flashwt create <name> --base <ref>` to store parent branch references as symbolic names, so that `flashwt` can detect when parent branches have moved after a stack rebase.
9. As a developer on a multi-core workstation, I want cold snapshot creation to partition directory subtrees across worker threads, so that directory linking runs in parallel without contending on parent directory locks.
10. As a Linux developer on an ext4 filesystem, I want `flashwt` to use a worker pool of `copy_file_range(2)` operations without attempting futile `FICLONE` ioctls, so that hydration achieves maximum kernel-side throughput.
11. As a developer working in a Python codebase, I want `flashwt` to sanitize `pyvenv.cfg`, `activate` scripts, and `bin/*` shebang lines upon hydrating `.venv/`, so that virtual environments work immediately inside the new worktree without re-creation.
12. As a Rust developer working in a Cargo workspace, I want `flashwt` starter manifests and hydration logic to omit path-poisoned incremental compiler caches while retaining compiled dependencies, so that cargo builds in new worktrees achieve maximum cache reuse without compiler errors.
13. As an agent coordinator running parallel subagents, I want `flashwt create` to record snapshot hits and publishes to an append-only write-ahead log using atomic POSIX appends, so that concurrent agents never serialize on a global file lock.
14. As a platform engineer managing shared build nodes, I want `flashwt sweep` to enforce a maximum snapshot byte budget (such as `FLASHWT_MAX_SNAPSHOT_BYTES=20GB`) using pre-computed manifest sizes, so that large monorepo snapshots do not exhaust disk space without double-counting cloned storage.
15. As a platform engineer managing shared build nodes, I want snapshot garbage collection to protect recently published snapshots from thrashing, so that parallel builds do not evict snapshots that sibling agents are actively using.
16. As a developer auditing store integrity, I want `flashwt scrub` to verify published snapshot manifests, `.complete` markers, and snapshot tree files alongside raw object blobs, so that corrupted snapshot directories are caught and purged before serving bad data.
17. As a developer auditing store integrity, I want `flashwt scrub` to run blob SHA-256 verification in parallel across 256 hash shards, so that scrubbing a large multi-gigabyte store finishes in seconds.
18. As an agent running on macOS, I want directory traversal in the bulk walker to use thread-local scratch buffers and exact length bounding, so that parallel walks avoid memory allocations and ghost entry corruptions.
19. As an agent switching between branches with identical dependency lockfiles, I want `flashwt create` to validate snapshot matches in O(1) time against dependency lockfile hashes while verifying safe dependency types, so that unchanged heavy directories hydrate instantly without scanning tens of thousands of files.

## Implementation Decisions

1. **Versioned JSON Output Contract.**
   - Add a global `--json` option to the command line interface parser.
   - Suppress human-readable stdout messages when `--json` is active. Route all diagnostic logs to stderr.
   - Emit a single line of NDJSON on stdout containing: `flashwt_version`, `schema_version` (integer `1`), `command`, `status` (`"ok"` or `"error"`), a typed `data` payload, and a `diagnostics` array.
   - Include `hydration_method` (`"clone"`, `"reflink"`, `"copy_file_range"`, or `"byte_copy"`), `bytes_shared_cow`, and `bytes_copied` in create data payloads.
   - Emit diagnostic warnings with code `CROSS_DEVICE_COPY_DEGRADATION` when storage boundaries force fallback copies.

2. **Lease-Backed Ephemeral Sandboxes.**
   - Implement the `scratch` (and `isolate`) subcommand in the CLI command dispatcher.
   - On creation, persist a lease record in `<store>/worktrees/scratch-<id>.lease` containing worktree path, git directory, process identifier, process start time (to guard against PID reuse), and expiration timestamp.
   - Use an in-process RAII guard implementing `Drop` for immediate cleanup on clean exit.
   - In `flashwt sweep`, inspect all active `.lease` files. Automatically reap worktrees whose process is dead, whose start time has shifted, or whose expiration timestamp has passed.

3. **Subtree-Partitioned Parallel Snapshot Building.**
   - Refactor snapshot tree building into two distinct phases.
   - Phase 1 creates all directory entries sequentially in sorted order to establish directory trees.
   - Phase 2 groups file and symlink entries by parent directory into subtree batches (500 to 2000 files per batch).
   - Dispatch batches across a worker pool bounded by available CPU parallelism via standard library `std::thread::scope`.
   - Each worker operates exclusively within its assigned subdirectories to eliminate APFS and ext4 directory-entry lock contention.
   - Use relative directory operations with `openat` and close handles eagerly to bound file descriptor consumption.

4. **Upfront Filesystem Probing and Linux Copy Acceleration.**
   - At store initialization, probe filesystem parameters via `statfs(2)` once and cache a tuple containing `(device_id, fs_type, reflink_capable)`.
   - On Linux ext4, bypass failing `FICLONE` calls entirely. Dispatch parallel `copy_file_range(2)` across 4 to 8 worker threads for in-kernel zero-copy page splicing.
   - On Linux btrfs and XFS, execute `ioctl(FICLONE)` for extent sharing.
   - Cross-device or unsupported mounts fall back to parallel buffered copy with `posix_fadvise(POSIX_FADV_SEQUENTIAL)`. Explicitly exclude `sendfile(2)` as it requires socket or pipe descriptors.

5. **Thread-Local Reusable Buffers in Bulk Directory Traversal.**
   - In the macOS bulk walker, equip each worker thread with a `thread_local!` scratch buffer initialized to 32KB.
   - Read the kernel returned entry count and byte length strictly, parsing only valid records to prevent ghost entry corruption.
   - Double buffer capacity dynamically when receiving `libc::ERANGE` up to a 1MB ceiling.

6. **Comprehensive Toolchain Relocation and Cache Invalidation.**
   - Execute a post-hydration toolchain sanitization pass after materialization completes.
   - For python environments, rewrite `pyvenv.cfg` base paths, `bin/activate*` shell scripts, and shebang lines in all executable scripts under `bin/`.
   - Leave python `.pyc` files alone, relying on python's built-in modification time checks for bytecode self-healing.
   - Exclude volatile toolchain caches (`target/debug/incremental/`, `.next/cache`, `node_modules/.vite`) from starter manifests, ensuring compilers regenerate fresh local machine caches.

7. **Tiered Lockfile Fast Path with Safety Classification.**
   - Add dependency safety classification to lockfile validation.
   - If a project lockfile contains local mutable references (`file:`, `link:`, `workspace:`, or unpinned git branch refs), bypass the fast path and execute full directory ingestion.
   - If all dependencies are pinned, compare lockfile SHA-256 with the snapshot manifest header. Return an O(1) hit if hashes match and the heavy directory root modification time is unchanged.

8. **Append-Only Snapshot WAL Journal.**
   - Replace whole-file load-modify-rename cycles with an append-only write-ahead log (`<store>/snapshots/journal.tsv`).
   - On snapshot publish or hit, append a single TSV line using atomic POSIX append mode (`O_APPEND`).
   - In `select_old_snapshot`, read recent entries from the journal.
   - During `flashwt sweep`, acquire an exclusive lock, compact journal entries into `index.tsv` and `lru.tsv`, fsync, and truncate the journal.

9. **Manifest-Recorded Logical Size Accounting and Sharded Scrubbing.**
   - Compute the sum of unique blob sizes during ingestion and record the total in `manifest.tsv`.
   - In `flashwt sweep`, use manifest-recorded sizes to enforce disk byte budgets (`FLASHWT_MAX_SNAPSHOT_BYTES`) alongside count caps (`FLASHWT_SNAPSHOT_CAP`), eliminating double-counting issues across hardlinks and clones.
   - Protect snapshots younger than the grace period from budget eviction to prevent cache thrashing.
   - Partition store scrubbing across 256 hash prefix shards (`00` to `ff`) and process shards in parallel. Audit published snapshot manifests, `.complete` markers, and referenced blob integrity.

10. **Ref-Aware Branch Stacking Metadata.**
    - Store parent branch references as symbolic names in worktree mirror metadata.
    - Check whether the base branch reference has moved during worktree operations and surface diagnostic warnings in `--json` output.

## Testing Decisions

- **Testing Philosophy.** Tests must verify external system contracts and observable behaviors, never private internal implementation helpers. All tests must run inside isolated temporary directories without modifying user data or existing checkouts.
- **CLI and Agent Workflow Seams.** Tested by invoking the compiled binary directly via `std::process::Command` against temporary git repositories. Assertions verify versioned JSON schema validity, exit code enums, branch creation, and lease cleanup under simulated process termination.
- **Store Concurrency and Durability Seams.** Tested by spawning parallel worker threads and processes executing simultaneous create, sweep, and scrub actions on a shared store directory. Assertions verify ledger consistency, absence of lost updates in the WAL journal, and zero data corruption.
- **Filesystem Fidelity Seams.** Tested by comparing hydrated trees against source donor directories using full byte diffs, file permission checks, and symlink target validations across APFS and Linux filesystems.
- **Prior Art.** Follows patterns established in `crates/flashwt-cli/tests/cli.rs`, `crates/flashwt-cli/tests/snapshots_v2.rs`, `crates/flashwt-store/tests/store.rs`, and `benchmarks/run.sh`.

## Out of Scope

- Implementing a background daemon, long-running filesystem watcher, or FUSE filesystem driver.
- Modifying standard git worktree storage layouts outside standard git conventions.
- Supporting non-POSIX operating systems (such as Windows NTFS without POSIX symlink support).
- Automatic lockfile resolution or package manager dependency resolution.

## Further Notes

Every performance optimization introduced by this spec must be verified using the automated benchmark suite (`./benchmarks/run.sh --verify` and `./benchmarks/v2-bench.sh`). Stage timings (`FLASHWT_TIMING=1`) must be measured before and after changes to confirm that cold snapshot creation, scrubbing, and Linux hydration metrics improve without regressions in warm snapshot hit times.
