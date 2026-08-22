# Revised Agent Handoff Plan: `wt` Hydration Performance

**Date:** 2026-08-23  
**Repo:** `/Users/sharzilnafis/Desktop/Project/dumps/idea1`, branch `master`, HEAD `b652c0c`  
**Status:** Revised after design review. This supersedes `AGENT_HANDOFF_PLAN.md`.

## Decision

Implement in this order: **fixture → store-local mark-and-sweep → macOS whole-directory snapshots → optional miss-path parallelism**. The revised design fixes root discovery, snapshot eviction, integrity on snapshot hits, manifest representation, concurrent publish, and old-binary downgrade safety.

The target is to remove the approximately 0.5s per-create refcount write cost and replace the per-file placement train with one APFS recursive `clonefile(2)` on a snapshot hit. The current warm run is 2.15s on the 4,000-file fixture; the shared `git worktree add` floor is roughly 0.6–0.7s [file:1].

---

## Non-Negotiable Invariants

- A corrupted blob must never silently land in a fresh worktree.
- Existing object layout stays unchanged: `objects/<2 hex>/<62 hex>`.
- Hydrated regular files must be private, writable, normal-permission inodes.
- Every externally visible store record is published atomically.
- A kill at any point may leak reclaimable cache data, but must not cause premature collection of live data or silent corruption.
- No daemon, watcher, or repo-global registry outside the store.
- v1 snapshot acceleration is **macOS/APFS only**. Linux retains the existing per-file reflink/hardlink/byte-copy ladder because Linux `FICLONE` does not clone directories recursively in one syscall [web:6].

---

## Phase 0: Establish the Benchmark Gate

### Goal

Add a realistic fixture before merging algorithmic work, so a micro-fixture does not determine the architecture. The project’s current fixture has 4,000 small files and 500 package directories; the handoff explicitly calls for realistic 20k–200k-file shapes [file:1].

### Work

Extend `benchmarks/fixture.sh` and `benchmarks/run.sh` with Scenario D:

- Approximately 40,000 files
- Approximately 800 package directories
- High duplicate-content ratio across package directories
- Regular files, nested directories, empty directories, and `.bin`-style symlinks
- Same byte/symlink/mode verification requirements as existing `--verify`

Report separate timings for:

- `git worktree add`
- ingest/manifest construction
- reference bookkeeping
- snapshot lookup/build
- materialization
- end-to-end create

### Acceptance

`./benchmarks/run.sh --verify` runs scenarios A–D and verifies regular-file contents, symlink targets, and directory/file modes. Do not estimate snapshot gains; measure them after each phase.

---

## Phase 1: Store-Local Mark-and-Sweep

### Goal

Remove 4,000 per-blob temp-write-rename refcount updates from `wt create` while keeping `wt sweep` store-local. The old plan’s root discovery through scattered `<worktree gitdir>/wt-hydrated.tsv` files is invalid: a store-local sweep cannot discover all repositories on a machine.

### Store Layout

Keep the existing worktree-local sidecar for diagnostics and recovery:

```
<worktree gitdir>/wt-hydrated.tsv
```

Add the authoritative store-local mirror:

```
<store>/worktrees/<worktree-key>.tsv
```

`worktree-key` is SHA-256 of a canonicalized identity string:

```
version=1 + NUL + canonical worktree path + NUL + canonical gitdir path
```

The mirror file is written to `worktrees/tmp/<uuid>` and atomically renamed to its final name. It replaces thousands of ref writes with one atomic write per successful create.

### Typed Record Format

Both local sidecar and store mirror use the same forward-compatible TSV schema:

```
v1	worktree	<escaped-absolute-worktree-path>	<escaped-absolute-gitdir-path>
file	<64-hex-blob-id>
snapshot	<64-hex-manifest-hash>
```

Rules:

- A line without a terminal newline is ignored.
- Unknown record types are ignored by newer readers only if explicitly declared optional; otherwise reject the mirror as invalid and preserve it for diagnosis.
- Paths are UTF-8 percent-escaped or length-prefixed; choose one implementation and document it. Do not use raw tab/newline-delimited paths.
- A mirror is valid only if it begins with exactly one `v1` header.

For v1 snapshots, a successful snapshot hydration writes one `snapshot` record, not all child blobs. A non-snapshot fallback writes `file` records for every blob placed. This makes GC roots explicit and handles mixed paths correctly.

### Root Validation

`wt sweep` reads only `<store>/worktrees/*.tsv`.

A mirror is live when:

1. Its recorded worktree path exists and is a directory.
2. Its recorded gitdir exists.
3. The worktree still has the expected `wt-hydrated.tsv` sidecar, or the mirror’s age is within the grace period.

Do **not** call `git worktree list` during sweep. Git’s administrative records can persist after `rm -rf` until `git worktree prune` runs, so they are not a reliable liveness oracle [web:2][web:5]. The filesystem existence check is store-local, works for arbitrary repositories, and treats deleted worktrees as dead after the grace period.

If the local sidecar exists but the mirror is missing, the next `wt create` or `wt remove` rewrites the mirror. v1 does not scan the filesystem to self-heal a mirror; that would reintroduce the root-discovery problem.

### Mark-and-Sweep Algorithm

#### Mark

1. Read valid live store mirrors.
2. For each `file` record, mark the referenced blob.
3. For each `snapshot` record, open the published snapshot manifest and mark every `file` entry’s blob ID.
4. If a mirror references a missing, invalid, or incomplete snapshot, do not mark through it. The corresponding worktree remains usable because it contains private clones; a later create treats the snapshot as a miss and rebuilds it.

#### Sweep

1. Use a default **15-minute grace period**, configurable by `WT_GC_GRACE`. It must exceed observed maximum snapshot build duration plus a conservative allowance for concurrent creates.
2. Delete unmarked object blobs only when older than the grace period.
3. Delete unreferenced, valid published snapshots only when older than the same grace period.
4. Delete `snapshots/tmp/*` older than the grace period.
5. Delete stale store mirrors whose recorded worktree path is absent and whose mirror age exceeds the grace period.

No refcount files participate in liveness after activation.

### Snapshot Eviction Rule

Snapshots are rebuildable caches, not unconditional GC roots. A snapshot is retained only when:

- referenced by a live store mirror; or
- protected by an explicit, bounded cache policy.

**v1 policy:** no LRU retention. Unreferenced snapshots are eligible for collection after the grace period. This gives bounded behavior without a second metadata system. A future v2 may add a bounded LRU with one atomic store index, but it is out of scope.

A snapshot directory is valid only if it contains a parseable `manifest.tsv` and a `.complete` marker with the expected manifest hash. Any published snapshot lacking either is debris and is eligible for removal after the grace period.

### Crash Safety

- A kill before mirror rename leaves no new GC root; the object-age grace protects recently ingested blobs and snapshot temp data.
- A kill after mirror rename leaves a complete store-local root.
- A kill mid-sweep leaves unlinked or still-present cache entries; the next sweep reconciles them.
- A torn mirror line is ignored. A malformed mirror is quarantined/ignored for deletion decisions during the grace period, then reported rather than silently treated as empty.

### Downgrade-Safe Migration

**Do not delete or stop writing `refs/` immediately.** An old binary may interpret a missing ref file as zero and collect an in-use blob.

Use a compatibility transition:

1. **Release N (dual-write):** new binaries write store mirrors **and continue current refcount behavior**. New `sweep` may run in audit mode and compare mark results to refs without deleting based on marks.
2. **Release N+1 (activate mark sweep):** new binaries use mirrors for GC but still preserve legacy `refs/` files untouched and continue maintaining them when creating/removing worktrees.
3. **Release N+2 or documented store-format cutover:** after compatibility policy is met, stop ref updates and add `format-version` / `gc-mode=mark-sweep` marker in the store. Never remove legacy refs automatically while an older binary could plausibly operate on the store.
4. Legacy refs are removed only through an explicit `wt store migrate --drop-legacy-refs` command, which warns that pre-cutover binaries must not use the store.

This retains old-binary safety and makes the transition deliberately one-way only when the operator opts in.

### Phase 1 Tests

- Live worktree mirror retains blobs.
- `rm -rf` worktree without `git worktree prune` becomes collectable after grace.
- Missing mirror can be recreated by a subsequent create/remove.
- Torn and malformed mirror handling does not prematurely collect recently referenced data.
- Kill mid-mirror-write and kill mid-sweep are recoverable.
- Dual-write store remains safe when accessed by the previous binary.
- Mark/sweep audit output agrees with legacy refs on fixtures.

### Phase 1 Acceptance

- No full-machine/repo scan is needed for sweep.
- Warm create performs one mirror write, not one ref write per blob.
- During dual-write, timing improvement is not yet the final 0.5s; the performance win is realized only after the explicit cutover.
- GC behavior is correct under out-of-band worktree deletion and crash tests.

---

## Phase 2: Whole-Directory Snapshots (macOS/APFS v1)

### Scope Decision

**v1 is one snapshot per configured heavy directory**, such as one `node_modules` tree. Do not implement per-package/per-subtree snapshots in this phase. Per-subtree snapshots require hierarchical manifests, parent assembly, different invalidation rules, and different GC granularity; make that a separately designed v2 after v1 measurements.

### Feature Gate

Introduce the gate before integration:

- `WT_SNAPSHOTS=1`: enable snapshot lookup/build on supported macOS filesystems.
- `WT_SNAPSHOTS=0`: force existing per-file materialization.
- Initial rollout: opt-in only. Make default-on only after parity, crash, and benchmark gates pass.

On Linux and unsupported macOS filesystems, the gate is a no-op and the existing per-file ladder remains active. Do not spawn `cp --reflink`; it still walks directories and issues per-file operations [web:6][web:14].

### Snapshot Layout

```
<store>/snapshots/tmp/<uuid>/
<store>/snapshots/<64-hex-manifest-hash>/
    manifest.tsv
    .complete
    <hydrated tree>
```

The tree’s regular files are hardlinks to immutable object blobs. The snapshot and objects must be on the same filesystem for `link(2)`.

### Canonical Manifest Schema

`manifest.tsv` uses one record per tree entry:

```
v1	manifest-sha256	<64-hex-hash>
entry	<escaped-relpath>	<kind>	<octal-mode>	<escaped-ref>
```

Where:

- `kind` is exactly `file`, `symlink`, or `dir`.
- For `file`, `ref` is a 64-hex blob ID.
- For `symlink`, `ref` is the symlink target, encoded with the same escaping rules.
- For `dir`, `ref` is `-`.
- Relative paths use forward slashes, contain no empty component, no `.` or `..`, and cannot be absolute.
- Entries are sorted by raw canonical path bytes, then kind if needed for a total order.
- The manifest hash is SHA-256 of the exact canonical serialized entry bytes, excluding the header. Do not hash an in-memory platform-dependent representation.
- Empty directories are represented explicitly.
- File and directory modes are represented explicitly. Snapshot build creates entries with normalized modes; snapshot clone inherits those modes.

Before implementing, inspect existing ingest behavior for symlinks, FIFOs, sockets, devices, and executable modes. v1 must either faithfully represent a type or reject it with a clear error before placement. Do not silently skip special files.

### Integrity Model

The integrity point is **snapshot publish**.

Before hardlinking any file blob into a new snapshot, the builder invokes existing verification policy:

- Normal mode: consult `verified.tsv`; if `(blob id, size, mtime)` matches, trust it; otherwise hash the blob and update verification state.
- `WT_VERIFY=1`: hash every file blob during snapshot build, regardless of ledger state.

A snapshot hit is safe only if the snapshot is immutable and previously published after this verification. To protect against post-publish media corruption, define `WT_VERIFY=1` behavior explicitly:

- `WT_VERIFY=1` **bypasses snapshot hits** and rebuilds/validates the snapshot from blobs before clone, or validates every blob in the existing snapshot before clone.
- Choose bypass+rebuild for v1 because it is simple, deterministic, and preserves the meaning of “force full hashing every run.”

Normal snapshot hits rely on verified-at-publish plus the existing immutable-store assumption, exactly as normal verified-ledger materialization relies on the same assumption [file:1]. If stronger continuous corruption detection is desired later, add a separately specified scrub command or bounded random sampling; do not claim it is covered by a hit with no reads.

### Snapshot Build and Publish

1. Ingest the heavy directory and create the canonical manifest.
2. Compute manifest hash.
3. If a valid published snapshot exists, use it.
4. Otherwise create `<store>/snapshots/tmp/<uuid>`.
5. Create directories with manifest modes.
6. For each file entry, verify its blob per policy, then `link(2)` object blob to its target path.
7. For each symlink entry, create the recorded symlink target with `symlink(2)`.
8. Write canonical `manifest.tsv`.
9. Write `.complete` containing the manifest hash and schema version, then `fsync` required data/metadata according to existing durability conventions.
10. Rename temp directory atomically to `<store>/snapshots/<hash>`.
11. If publish loses with `EEXIST`/`ENOTEMPTY`, validate the winner’s manifest and `.complete`; if valid, discard the temp tree and use the winner. If invalid, treat the winner as debris, do not overwrite it, and return a diagnostic/retry after cleanup.

### Sweep Race and Blob Healing

A sweep could remove an unmarked blob between ingest and `link(2)` only if the blob is old and the process has not yet published a mirror/snapshot root. On `link(2)` `ENOENT`:

1. Re-run `put()` for the source content.
2. Re-run verification as required.
3. Retry the link once.
4. If it still fails, report a hard error.

The default grace period is the primary protection; this retry is the correctness backstop.

### Hydration Fast Path

On a valid snapshot hit:

1. Ensure destination heavy directory does not exist.
2. Call APFS recursive `clonefile(snapshot_tree, destination)`.
3. Write the local sidecar and store mirror with a typed `snapshot` record.

If clone fails with cross-device or unsupported errors, use the existing per-file ladder and write typed `file` records instead. If the destination exists:

- If it is an empty directory created by the command, remove it and retry clone.
- If it is non-empty, use the existing per-file merge behavior.
- If `clonefile` fails after creating a partial destination, remove only the command-owned destination tree before retry/fallback; never merge an unknown partial snapshot result silently.

### Snapshot Eviction

A snapshot referenced by a live mirror is retained. An unreferenced snapshot is cache debris after the grace period and is deleted before/with its now-unreachable blobs. Snapshot deletion is unlink/rmdir of the derived tree; a concurrent create that observes `ENOENT` treats it as a miss and rebuilds.

### Phase 2 Tests

- Manifest round trip: files, executable bits, empty directories, symlinks, nested paths.
- Invalid path/type rejection before placement.
- Snapshot miss builds, publishes, then clones correctly.
- Snapshot hit uses clone fast path and produces private writable files.
- `WT_VERIFY=1` bypasses hits and hashes all file blobs before clone.
- Two concurrent creates for identical content: one publish wins, loser consumes winner without failure.
- Snapshot deletion race: a mirror refers to an evicted/missing snapshot; next create rebuilds rather than corrupts/fails silently.
- `link(2)` `ENOENT` healing re-puts and retries.
- Kill mid-build creates only temp debris; kill mid-clone leaves destination cleaned or follows explicit fallback behavior.
- Cross-volume and non-empty-destination fallback preserves existing semantics.
- Scenario D `--verify` checks symlink targets, empty dirs, modes, and file bytes.

### Phase 2 Acceptance

On APFS with `WT_SNAPSHOTS=1`, a hit does one recursive clone operation per heavy directory and one mirror write. On Linux, behavior remains correct with no promise of recursive-reflink acceleration. Warm benchmark results, not projections, decide default-on status.

---

## Phase 3: Parallelize Only the Snapshot Miss Path

Do this only after Phase 2 benchmark data shows snapshot construction is material. Snapshot hits should not need a thread pool.

- Use `std::thread::scope`.
- Pre-create directory structure serially to avoid mkdir races.
- Use per-thread local result vectors; merge after join.
- Use an `AtomicBool` cancellation flag and first-error storage for verification/build failures.
- Do not put mutex-protected ledger writes in the file hot loop.

On Linux, this phase may also improve per-file reflink placement, because there is no single recursive clone primitive [web:6].

---

## Validation and Release Gates

- `cargo test` remains green; add focused tests above.
- Clippy clean.
- `./benchmarks/run.sh --verify` covers all fixtures and all supported modes.
- Run crash tests against mirror publication, snapshot publication, clone failure cleanup, and sweep.
- Do not make `WT_SNAPSHOTS` default-on until snapshot hit/miss parity and `WT_VERIFY=1` semantics are demonstrated.
- Do not complete refcount cutover until compatibility transition criteria are met and the operator has explicitly opted into dropping legacy refs.

## Out of Scope / Follow-ups

- Per-subtree snapshot hierarchy and diff-based snapshot rebuilds (v2).
- Snapshot LRU retention beyond live references (v2).
- Linux whole-tree snapshot optimization; there is no equivalent single-call directory `FICLONE` path, so investigate filesystem-native snapshots separately rather than masking the limitation with a spawned recursive copy [web:6][web:14].
- Background daemon, watcher, or automatic global repository discovery.
