# Design Note: v2 Diff-Based Snapshot Rebuilds (Revised)

**Date:** 2026-08-23  
**Status:** Proposed v2 after Phase 2 measurements showed cold builds cost ~24s at 40k files [file:46].  
**Goal:** Make the first `wt create` after a dependency change fast by rebuilding snapshots incrementally from the previous snapshot, without new formats or Merkle trees.

---

## Problem Statement (Updated)

v1 whole-directory snapshots win on hits (one `clonefile(2)` per heavy directory) but lose on cold builds: ~24s at 40k files because APFS serializes `link(2)` calls [file:46]. A typical agent workflow is:

1. Create worktree A (first create after dependency bump) — 24s cold build.
2. Create worktrees B, C, D (same tree content) — ~6.2s each via snapshot hit.
3. Developer bumps a dependency.
4. Create worktree E (new tree content) — back to 24s cold build.

Step 4 is the pain point. The tree changed by 1–3 packages out of 800, but v1 rebuilds the entire snapshot from blobs. The goal of v2 is to make step 4 cost **O(changed files)** instead of **O(total files)**, while preserving v1's integrity, crash safety, and GC properties.

**Additional motivation (hit-rate poisoning):** With whole-directory snapshots, one mutable file inside a heavy directory (a `.DS_Store`, a timestamped cache) turns every subsequent `wt create` into a full 24s rebuild. Subtree-aware rebuilds bound that damage structurally — a single changed file only invalidates its containing subtree, not the entire tree.

---

## Why the Original v2 Note Was Wrong

The original design proposed hierarchical Merkle trees (`tree.tsv`, subtree hashes, etc.). That was the wrong mechanism for three reasons:

1. **Unimplementable:** The algorithm assumed "given an old snapshot (root H_old)" but never specified how H_old is found. The store is content-keyed; after a bump, the new root is unknown until you walk the donor, and nothing maps "this project's heavy dir" → "its previous snapshot" [file:46].

2. **Solves a problem you don't have:** Git/OSTree/Nix use subtree hashes because they *store* trees hierarchically and must skip recursion into unexplored subtrees. `wt` walks the entire donor tree every create — both manifests are fully materialized in memory before any diff begins. Directory-level identity falls out of a sorted merge for free.

3. **Breaks compatibility:** Keying v2 snapshots by the Merkle root means a v2 client misses every existing v1 snapshot (different key), and a v1 client can never find a v2 snapshot. The claimed "v1 clients can still use v2 snapshots" was false.

**The right design:** Keep the v1 flat-manifest key forever. Add a best-effort selection index. Diff two flat manifests by sorted merge. Clone unchanged path-prefix blocks. Link only changed files. No new formats, no migration, zero compatibility break.

---

## Core Insight: Flat Manifests Are Already Sorted

A v1 snapshot's `manifest.tsv` is already a flat, sorted serialization of the tree [file:46]:

```
v1	manifest-sha256	<64-hex-hash>
entry	<escaped-relpath>	<kind>	<octal-mode>	<escaped-ref>
```

Entries are sorted by raw canonical path bytes [file:46]. This means:

- **Diff is a sorted merge:** Load old and new manifests, walk them in parallel, emit add/modify/delete entries.
- **Clone units fall out naturally:** The maximal common path-prefix blocks with identical entry runs are exactly your "unchanged subtrees."
- **No subtree hashes needed:** You already have the full tree in memory; the expensive part was the link train, not the diff.

---

## Design

### 4.1 Selection Index — `snapshots/index.tsv`

**Problem:** After a dependency bump, how do you find the previous snapshot for this heavy directory?

**Solution:** A best-effort selection index:

```
<store>/snapshots/index.tsv

# Format: TSV, one line per (repo root, include pattern, heavy-dir name)
<abs repo root>\t<include pattern>\t<heavy dir name>\t<ring of last K root hashes>\t<last mtime>
```

**Update policy:** On every snapshot publish, update the index entry for that (repo root, include pattern, heavy dir) with the new root hash and mtime. Maintain a ring of last K=3 hashes for cheap rollback.

**Selection algorithm:**

1. Look up the index entry for (repo root, include pattern, heavy dir).
2. Try each ring entry newest-first:
   - If the snapshot exists and is valid, use it as the old snapshot.
   - If the snapshot is missing or invalid, try the next ring entry.
3. If no ring entry works, fall back to the newest snapshot on disk (by mtime).
4. If no snapshot exists, do a full v1 build.

**Correctness:** The index is best-effort only. A stale pointer just makes the diff bigger; correctness never depends on it. A missing index entry falls back to full v1 build.

**Bonus:** The ring gives you cheap rollback retention (answers original Open Question 2 — do it now, it's five lines).

### 4.2 Diff Algorithm — Sorted Merge of Flat Manifests

**Input:**

- Old snapshot's `manifest.tsv` (from the selection index).
- New manifest (computed from the current donor walk).

**Algorithm:**

1. Load both manifests into memory (both are sorted by relpath).
2. Walk them in parallel:
   - If an entry exists in both with identical (relpath, kind, mode, ref), emit **unchanged**.
   - If an entry exists in both with different (mode, ref), emit **modified**.
   - If an entry exists only in the new manifest, emit **added**.
   - If an entry exists only in the old manifest, emit **deleted**.
3. Group unchanged entries into maximal common path-prefix blocks (clone units).
   - A clone unit is a contiguous run of unchanged entries sharing a common parent directory.
   - In practice, this lands on package dirs naturally.

**Complexity:** O(N), where N is the total number of entries. The diff is milliseconds; the link train was seconds.

### 4.3 Rebuild Algorithm

**Input:**

- Old snapshot (from selection index).
- Diff result (unchanged clone units, modified/added/deleted entries).

**Steps:**

1. **Create temp directory:** `<store>/snapshots/tmp/<uuid>/`.
2. **Pre-create all directories top-down:**
   - For every directory that will exist in the new snapshot (boundary directories between cloned subtrees, added directories), `mkdir(temp/<relpath>)`.
   - This kills mkdir races and ensures clone destinations exist.
3. **Clone unchanged subtrees:**
   - For each clone unit (a path-prefix block of unchanged entries):
     - `clonefile(old_snapshot/tree/<subtree>, temp/<subtree>)`.
     - If `clonefile` fails (cross-volume, etc.), fall back to per-file `link(2)` for that subtree.
4. **Link modified/added files:**
   - For each modified/added file entry:
     - Verify the blob per policy (ledger trust, full hash under `WT_VERIFY=1`).
     - `link(2)` the blob into `temp/<relpath>`.
5. **Recreate symlinks:**
   - For each added/modified symlink entry:
     - `symlink(2)` the target into `temp/<relpath>`.
   - **Note:** Symlinks in cloned subtrees already exist — do not recreate them.
6. **Delete removed files/directories:**
   - For each deleted file: ensure it's absent (it won't exist in temp yet — this is a no-op).
   - For each deleted directory: ensure it's absent (also a no-op; boundary directories were created in step 2).
7. **Write `manifest.tsv` and `.complete`, then atomic rename.**
   - The snapshot key is the **unchanged v1 flat-manifest hash** — no format change.

**Crash safety:** Inherited wholesale from v1 [file:46]:

- Publish is atomic (`rename`).
- Kill mid-build leaves an orphan under `tmp/`, collected after grace.
- Kill mid-`clonefile` leaves a partial destination; `wt create` cleans its own dest on failure.
- Eviction race: snapshot deleted while another `wt create` clones from it → `ENOENT` → treat as miss and rebuild.

**Integrity:** One new trust surface: cloned subtree files were verified at the *old* snapshot's publish, not this one. That's the same accepted class as the verified-ledger (same size/mtime bit flip) [file:46], but document it explicitly. Under `WT_VERIFY=1`, hash cloned subtree files against their manifest blob-ids so paranoid mode covers the new path.

### 4.4 GC

**Unchanged.** Mark from mirrors → manifests → blobs [file:46]. Flat manifests are still the canonical mark source (another reason not to switch to `tree.tsv`). Shared blocks between snapshots are CoW; deleted snapshots' tree dirs are just dirent space.

### 4.5 Platform Scope

**v2 is macOS-only, same as the hit-path win.** On Linux, there is no recursive `clonefile` primitive, so unchanged subtrees still need O(N) `link(2)` calls — no rebuild win. Say so explicitly; do not claim Linux benefits from v2.

---

## Performance Model (Revised)

Let:

- `N` = total files in the tree (40,000 at Scenario D scale).
- `C` = changed files (e.g., 3 packages × 500 files = 1,500).
- `L` = `link(2)` cost (~300 µs/file on APFS, serialized) [file:46].
- `K` = `clonefile(2)` cost for a subtree (~100–150 ms for a 10k-file tree) [file:1].
- `W` = donor walk + manifest construction (~1.5s measured in stage timing).
- `F` = final `clonefile` into worktree (~0.6–3.0s, depends on H1 vs H2 — see §5).

**v1 cold build cost:** `W + N × L + K + publish` = ~24s measured [file:46].

**v2 incremental rebuild cost:** `W + C × L + (unchanged subtrees) × K + F + publish`.

If 96% of the tree is unchanged (38,400 files) and we clone it in 4 subtrees of 9,600 files each:

- Donor walk: ~1.5s.
- Changed files: 1,500 × 300 µs = ~0.45s.
- Unchanged subtrees: 4 × 150 ms = ~0.6s.
- Final clonefile: ~0.6–3.0s (H1 vs H2).
- Publish: negligible.
- **Total: ~3.15–5.55s** (vs. 24s for v1).

That's a **4–8×·speedup** on the cold build after a small dependency change.

**Break-even:** v2 wins when `C < N - (unchanged subtrees × K / L)`. At the measured numbers, v2 wins when <10% of the tree changes.

**Hit-rate poisoning scenario (new benchmark F):** One `.DS_Store` change in a 40k tree:

- v1: full 24s rebuild (entire tree invalidated).
- v2: one file changed → one `link(2)` + final clonefile → ~2–4s.

That's the stronger argument for v2, not just cold-build speed.

---

## Implementation Plan (Revised)

### Phase 2.1: Selection Index

- Add `snapshots/index.tsv` with (repo root, include pattern, heavy dir) → ring of last K=3 root hashes + mtime.
- Update index on every snapshot publish.
- Add selection algorithm (§4.1) to `wt create`.
- No incremental rebuild yet — just use the index to find the old snapshot for diffing (but still do full v1 build).

**Acceptance:** Index is valid, selection works, GC unchanged.

### Phase 2.2: Diff Algorithm

- Implement the sorted-merge diff algorithm (§4.2).
- Add unit tests for diff correctness (unchanged, added, modified, deleted).
- Benchmark diff cost on 40k trees with 1%, 5%, 10% changes.

**Acceptance:** Diff is O(N), not O(subtree hashes).

### Phase 2.3: Incremental Rebuild

- Implement the rebuild algorithm (§4.3).
- Add `WT_SNAPSHOT_V2=1` gate (opt-in until validated).
- Add benchmark scenarios:
  - **Scenario E:** bump 3 of 800 packages, measure end-to-end.
  - **Scenario F:** one junk file (`.DS_Store`) in the heavy dir, measure end-to-end.
- Benchmark cold builds after small/large changes.

**Acceptance:** Post-bump create ≤ ~2×·warm-hit cost (under H1); bounded miss cost under F.

### Phase 2.4: Integrity Under `WT_VERIFY=1`

- Extend `WT_VERIFY=1` to hash cloned subtree files against their manifest blob-ids.
- Document the trust model explicitly: cloned files are trusted from the old snapshot's publish-time verification, same class as the verified-ledger gap [file:46].

**Acceptance:** `WT_VERIFY=1` covers the new path; no silent corruption.

### Phase 2.5: GC Validation

- Verify GC marks blobs through both old and new snapshot manifests correctly.
- Test snapshot deletion with shared subtrees.
- Add crash tests for kill mid-`clonefile` of unchanged subtrees.

**Acceptance:** GC behavior unchanged; no premature blob collection.

---

## Risks & Mitigations (Revised)

| Risk | Mitigation |
|------|------------|
| Selection index stale/missing | Best-effort only; fall back to full v1 build. |
| `clonefile` of unchanged subtrees fails (cross-volume, etc.) | Fallback to per-file `link(2)` for that subtree; log and continue. |
| GC leaks space from unreferenced subtrees | Unchanged from v1; flat manifests are still the mark source. |
| v1/v2 compatibility | Key stays the v1 flat-manifest hash — zero format fork, zero migration. |
| APFS `clonefile` on directories is slower than expected | Measure; if slower than `link(2)` train, skip subtree clone and link files individually. |
| Linux users expect v2 benefits | Explicitly document: v2 is macOS-only; Linux has no recursive reflink primitive. |

---

## Open Questions (Revised)

1. **Subtree granularity:** How large should unchanged subtrees be? The design says "maximal common path-prefix blocks" — in practice this lands on package dirs. Tune based on measurements if needed.
2. **LRU retention:** The ring of K=3 hashes gives cheap rollback. Is bounded LRU retention for unreferenced snapshots worth adding? (Probably not for v2; defer.)
3. **Cross-snapshot dedup:** If two snapshots share 90% of their subtrees, should we store a "delta snapshot" that references the base? CoW already dedups blocks; snapshot trees cost only dirents. Probably overkill for v2.
4. **Linux support:** No recursive `clonefile`, so unchanged subtrees need O(N) `link(2)` calls. Is the win still worth it? (No — v2 is macOS-only.)

---

## References

- v1 snapshot design: `HANDOFF.md` §2 [file:46].
- Sorted merge diff: standard algorithm for sorted lists.
- APFS `clonefile` performance: ~100–150ms for 10k-file tree [file:1].
- `link(2)` serialization on APFS: ~300 µs/file [file:46].
- Original (flawed) v2 note: `v2-diff-rebuild-design-note.md` (superseded).