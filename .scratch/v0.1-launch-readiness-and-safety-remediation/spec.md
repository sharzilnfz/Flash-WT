Status: ready-for-agent

# Specification: v0.1.0 Launch Readiness, Safety Hardening & Architecture Remediation

## Problem Statement

Users and autonomous coding agents need instant, copy-on-write hydration of heavy untracked project directories (`node_modules`, `target`, `.venv`) when launching parallel Git worktrees. 

Currently, the tool suffers from critical data safety defects, storage engine corruption bugs, broken distribution packaging, and naming conflicts that make it unsafe for public release (`v0.1.0`):
1. Worktree cleanup (`clean`) forcefully deletes unmerged worktrees and branches without user consent, swallows removal errors, and prints misleading success receipts.
2. In-flight scratch lease cleanup over-decrements content-addressed store (CAS) blob reference counts on duplicate files, causing live blobs to be prematurely deleted by garbage collection.
3. The APFS lockfile fast-path serves stale cached snapshots when nested files are modified without changing the lockfile.
4. Linux server-side copy (`copy_file_range`) silently truncates files on early EOF or short writes, while Btrfs subvolumes are unnecessarily locked out of reflink acceleration.
5. Interrupting operations with `SIGINT` (`Ctrl+C`) leaks orphan worktrees, branches, and active store leases because no process signal handlers exist.
6. The CLI binary name collides directly with *Worktrunk* (`flashwt`), causing package manager and executable conflicts.
7. Automated release workflows produce tarballs with mismatched filename prefixes (`flashwt-0.1.0-*` vs `flashwt-v0.1.0-*`), breaking Homebrew installation and the curl installer.

## Solution

Harden the tool into a safe, reliable, and honestly marketed local hydration engine:
1. **Enforce Strict Safety Invariants:** Require explicit `--force` to delete dirty worktrees or unmerged branches. Verify Git and filesystem status before and after deletion, failing loudly on errors.
2. **Prevent CAS & Snapshot Inode Corruption:** Deduplicate blob identifiers before refcount decrements during lease sweeps, and eliminate in-place permission mutations on shared store inodes.
3. **Correct Fast-Path Cache Invalidation:** Ensure the snapshot engine verifies full directory contents before skipping tree walks.
4. **Harden Cross-Platform Copy Operations:** Handle short writes, signal interruptions (`EINTR`), and subvolume device boundaries properly across Linux filesystems.
5. **Add Transactional Rollbacks & Signal Safety:** Register signal hooks to clean up temporary scratch leases on cancellation, and roll back partially created Git worktrees if hydration fails.
6. **Fix Release Packaging & Provide Clear Extension Points:** Fix archive naming conventions in CI, scope mirror operations to the active repository, and expose a standalone command to hydrate existing worktrees.

---

## User Stories

1. As a software developer, I want `clean` to refuse to delete worktrees with uncommitted changes unless I pass `--force`, so that I never lose work in progress.
2. As a software developer, I want `clean` to check whether a worktree branch has been merged into `HEAD` before deleting it, so that unmerged feature branches are not lost.
3. As a CI/CD engineer, I want `clean` to exit with a non-zero status code when filesystem or Git deletions fail, so that pipeline scripts detect failed cleanup operations.
4. As an autonomous coding agent, I want `clean` JSON output to accurately report only worktrees that were genuinely removed from disk and Git tracking, so that my state machine never loses track of active workspaces.
5. As an autonomous coding agent, I want concurrent scratch worktrees with duplicate files (such as license files and header stubs) to be cleaned up without prematurely deleting shared store blobs, so that sibling worktrees never crash with missing blob errors.
6. As a developer modifying a dependency file inside `node_modules`, I want subsequent worktree creations to detect my modifications even if the lockfile is unchanged, so that my new worktree does not receive a stale snapshot.
7. As a Linux developer on an ext4/xfs filesystem, I want `copy_file_range` to copy all requested bytes without silent truncation on short writes, so that large assets are never corrupted.
8. As a Linux developer on Btrfs with subvolumes, I want cross-subvolume worktree creation to leverage `ioctl(FICLONE)` reflinks, so that I get copy-on-write speed without falling back to full byte copies.
9. As a developer running scratch tasks in terminal, I want pressing `Ctrl+C` to cleanly delete the ephemeral worktree and branch, so that interrupted commands do not leave garbage across my repository.
10. As a Python developer using editable packages (`pip install -e .`), I want worktree hydration to rewrite absolute paths in `.pth` files, so that my virtual environment imports code from the new worktree checkout rather than the source directory.
11. As a Python developer with binary launcher scripts in `.venv/bin`, I want non-UTF-8 executables with shebang lines to be handled gracefully without crashing hydration, so that virtual environments remain fully functional.
12. As a developer working across multiple distinct repositories on the same machine, I want branch stacking diagnostics to only check the active repository, so that similar branch names in unrelated projects never trigger false warnings.
13. As an operator configuring the tool with environment variables, I want `FLASHWT_SNAPSHOTS=false` and `FLASHWT_SNAPSHOTS=no` to disable snapshots as expected, so that standard boolean flag formats work intuitively.
14. As a developer installing the tool via Homebrew or curl, I want release archive names to match what the installer and formula request, so that installation succeeds on every published tag.
15. As a developer running in a Docker container as `root`, I want hardlinked worktrees to prevent accidental mutation of the global content store, so that containerized test runs do not poison other worktrees.
16. As an autonomous coding agent executing a command via `scratch --run`, I want child output to stream cleanly without corrupting the parent JSON stdout envelope, so that automation parsers do not encounter syntax errors.
17. As a developer creating a worktree with `new`, I want any hydration failure to automatically remove the newly created Git worktree and branch, so that failed creations do not leave half-initialized directories behind.
18. As an open-source developer using an existing worktree manager, I want a standalone command to hydrate heavy directories into an existing directory, so that I can accelerate worktree setups without changing my workspace orchestration workflow.
19. As a developer auditing disk usage with `list`, I want storage savings to be clearly labeled as estimated logical reuse, so that I understand how much data is shared across worktrees.
20. As a package maintainer, I want the canonical binary name to not conflict with *Worktrunk*, so that both tools can be installed on the same system without path collisions.

---

## Implementation Decisions

### 1. Unified Worktree Cleanup Safety Contract
- Cleanup must explicitly inspect worktree status using porcelain Git commands (`git status --porcelain`) before attempting removal.
- Targeted cleanup of a single worktree must require `--force` if dirty files or unmerged commits are present.
- Batch cleanup (`clean` / `clean --all`) must strictly filter candidates by default to merged worktrees only. Unmerged worktrees must require an explicit `--force` flag.
- Deletion operations must verify that Git worktree metadata and filesystem paths are fully removed before retiring the store mirror and recording the worktree in the success receipt. Any failure must append to a failure list, return an error diagnostic, and produce a non-zero exit code.

### 2. Reference Count Deduplication in Lease Sweep
- The lease cleanup engine must collect all blob identifiers from the target worktree's hydration record into a deduplicated set (`Set<ContentId>`) before invoking the store reference decrement interface.
- Each distinct blob identifier must have its refcount decremented exactly once per retired worktree.

### 3. Lockfile Fast-Path Staleness Validation
- The lockfile projection engine must not rely solely on top-level directory modification timestamps (`mtime`).
- Fast-path verification must execute a rapid bulk directory stat scan to ensure that no nested file timestamps are newer than the candidate snapshot creation timestamp before accepting an O(1) directory clone hit.

### 4. Linux Kernel Copy & Subvolume Support
- `copy_file_range` materialization must check if `rc == 0` while bytes remain to be copied; if remaining bytes $> 0$, it must treat the condition as an error or fall back to standard buffered copy.
- Sycall retry loops must intercept `EINTR` and resume copying rather than failing immediately.
- Filesystem device probing must identify Btrfs subvolumes and avoid disabling `FICLONE` reflinks merely because `stat.st_dev` differs across subvolumes.

### 5. Process Signal Handling & Transactional Rollbacks
- The CLI entry point must register signal handlers (`SIGINT`, `SIGTERM`) that trigger cleanup of ephemeral scratch guards and active temporary directories before process exit.
- Worktree creation must be transactional: if store ingestion, snapshot projection, or toolchain relocation fails, the newly created Git worktree and its tracking branch must be automatically pruned and removed.

### 6. Toolchain & Python Relocation Enhancements
- Virtual environment relocation must discover and sanitize `.pth` and `_editable.*.pth` files inside `site-packages/`, replacing source checkout paths with destination worktree paths.
- Binary executable patching in `bin/` must locate the shebang line directly within binary buffers without requiring the entire file to be valid UTF-8.
- Directory tree traversal for virtual environment discovery must prevent infinite loops by maintaining a set of visited device/inode pairs to guard against recursive symlinks.

### 7. Scoped Base Branch Stacking & Boolean Config Normalization
- Branch stacking and mirror verification must filter store mirrors by repository root path, preventing false-positive diagnostics across unrelated projects.
- Environment variable parsing for boolean flags must recognize `"0"`, `"false"`, `"no"`, and `"off"` (case-insensitive) as false, and treat empty strings as omitted/default.

### 8. Standalone Hydration Interface & Packaging Normalization
- Expose a dedicated command (`hydrate <path>`) that ingests and materializes heavy untracked directories for an already existing worktree or directory.
- Align release packaging scripts and GitHub Actions workflows to use consistent `v`-prefixed archive filenames (`flashwt-v<version>-<target>.tar.gz`).
- Standardize on single-binary release distribution and update Homebrew formula templates accordingly.

---

## Testing Decisions

### Seams for Testing
All testing will occur at the **highest external CLI binary seam**:
- Test through the compiled binary entry point (`assert_cmd` / `Command`) driving real Git repositories and isolated temporary store instances (`FLASHWT_STORE=<tempdir>`).
- Driving through the top CLI boundary guarantees end-to-end verification of arguments, environment parsing, Git interactions, filesystem clone/copy mechanics, and JSON envelope output simultaneously.
- No internal private struct unit testing where an external CLI invocation can prove the behavior.

### Good Test Criteria
1. **External Behavior Only:** Tests must assert against stdout/stderr JSON envelopes, exit status codes, and actual on-disk filesystem state (inode sharing, file modes, contents, Git branch lists).
2. **Crash & Signal Resilience:** Assert that interrupted operations leave zero orphaned directories, branches, or store references.
3. **Multi-Platform Matrix:** Test APFS clone mechanics on macOS, and reflink / `copy_file_range` / fallback behavior on Linux.

### Prior Art in Codebase
- [`crates/flashwt-cli/tests/clean.rs`](file:///Users/sharzilnafis/Projects/dumps/idea1/crates/flashwt-cli/tests/clean.rs): Worktree cleanup candidate selection and deletion tests.
- [`crates/flashwt-cli/tests/cow_materialization.rs`](file:///Users/sharzilnafis/Projects/dumps/idea1/crates/flashwt-cli/tests/cow_materialization.rs): Private write isolation and block-level copy-on-write assertions.
- [`crates/flashwt-cli/tests/lease_sweep.rs`](file:///Users/sharzilnafis/Projects/dumps/idea1/crates/flashwt-cli/tests/lease_sweep.rs): Ephemeral lease expiration and mark-and-sweep lifecycle tests.
- [`crates/flashwt-cli/tests/toolchain_relocation.rs`](file:///Users/sharzilnafis/Projects/dumps/idea1/crates/flashwt-cli/tests/toolchain_relocation.rs): Python virtualenv and Cargo build artifact relocation assertions.

---

## Out of Scope

1. **In-Kernel Filesystem Drivers (FUSE / KEXT):** The tool remains strictly a userspace CLI.
2. **Real-Time Filesystem Watcher Daemon:** No long-running background daemon or file system event listener will be introduced in v0.1.0.
3. **General Workspace Port & Database Orchestration:** Managing container ports, background services, and databases belongs to tools like Workz and remains out of scope.
4. **General Package Manager Dependency Resolution:** The tool does not resolve or download packages from remote registries (`npm`, `cargo`, `pypi`); it hydrates existing local trees.
5. **Windows Native Support:** v0.1.0 targets macOS (APFS) and Linux (Btrfs, XFS, ext4). Windows support is deferred.

---

## Further Notes

- **ADR Alignment:** Decisions adhere to [ADR-0002](file:///Users/sharzilnafis/Projects/dumps/idea1/docs/adr/0002-tool-agnostic-worktree-hydration-first.md) (Tool-agnostic hydration), [ADR-0004](file:///Users/sharzilnafis/Projects/dumps/idea1/docs/adr/0004-mark-and-sweep-gc.md) (Store-local mark-and-sweep GC), and [ADR-0005](file:///Users/sharzilnafis/Projects/dumps/idea1/docs/adr/0005-directory-snapshots.md) (Whole-directory APFS snapshots).
- **Ponytail Deletions:** Post-v0.1.0 architecture work will remove legacy `refs/` directory machinery and simplify the snapshot WAL journal into an atomic TSV file once v0.1.0 safety is certified.
