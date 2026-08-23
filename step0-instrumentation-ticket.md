# Ticket: Step 0 — `WT_TIMING=1` Breakdown to Resolve H1 vs H2

**Date:** 2026-08-23  
**Priority:** P0 (blocks all v2 modeling and default-on decision)  
**Owner:** Next agent  
**ETA:** 4–6 hours

---

## Problem

The reported 40k numbers cannot all be true under one cost model [file:46]:

| Measured | Cheap-clonefile model (~15 µs/file → 0.6s) | Expensive-clonefile model (~75 µs/file → 3s) |
|----------|---------------------------------------------|----------------------------------------------|
| warm snapshot hit: 6.2s | 0.7 git + 0.6 clone + **~4.9s unexplained** | 0.7 + 3.0 + ~1.5 ingest/ledger — fits |
| ladder warm: 11.78s | 0.7 + ~9.5 fclonefileat train + ~1.6 other — fits | same — fits |
| cold build: 24s | 0.7 + ~12 link train + 0.6 + **~9s unexplained** | 0.7 + ~19 link train + 3.0 — fits |

Either:

- **H1:** Recursive `clonefile` is cheap and `wt`'s snapshot path carries 3–4s of unattributed overhead — prime suspects: ingest-cache TSV lookup that isn't a HashMap, per-run ledger parsing, or benchmark `--verify` contamination.
- **H2:** Recursive `clonefile` at 40k files genuinely costs ~3s and the overhead story is fine.

**These two hypotheses predict opposite v2 payoffs:**

- Under H1, v2's subtree clones are cheap (~0.6s), and the win is dominated by avoiding the 12s link train. Post-bump create should land at ~2.5–4s.
- Under H2, v2's subtree clones cost ~2.9s and the final clonefile ~3s, so post-bump lands at ~7–9s: still 3×·better than 24s, but roughly *equal to `cp -Rc`*, and the "23×·speedup" claim becomes ~2.6×·.

Until the stage breakdown is published, every model in the v2 note is building on sand.

---

## Work

### 1. Add `WT_TIMING=1` Stage Breakdown

The handoff already specifies `WT_TIMING=1` emits `wt-stage ingest=N`, `references=N`, `materialize=N`, `snapshot=N`, `total=N` (ms) on stderr [file:46]. Extend this to:

**For snapshot hit:**
- `git-worktree`: `git worktree add` time.
- `ingest`: donor walk + manifest construction.
- `references`: mirror write (and legacy refcount writes if dual-write).
- `snapshot-lookup`: index lookup + snapshot validation.
- `clonefile`: recursive `clonefile(snapshot, dest)` time.
- `total`: end-to-end.

**For snapshot miss (cold build):**
- `git-worktree`: `git worktree add` time.
- `ingest`: donor walk + manifest construction.
- `references`: mirror write.
- `snapshot-build`:
  - `verify`: blob verification time (ledger trust vs full hash).
  - `link-train`: `link(2)` calls for snapshot assembly.
  - `publish`: atomic rename + index update.
- `clonefile`: recursive `clonefile(snapshot, dest)` time.
- `total`: end-to-end.

**For per-file ladder:**
- `git-worktree`: `git worktree add` time.
- `ingest`: donor walk + manifest construction.
- `references`: mirror write.
- `materialize`:
  - `verify`: blob verification time.
  - `place`: per-file `fclonefileat`/`link(2)`/byte-copy train.
- `total`: end-to-end.

### 2. Run Breakdown on 40k Fixture

**Scenarios:**

1. **Warm snapshot hit:** `WT_SNAPSHOTS=1`, second create with unchanged tree.
2. **Warm ladder:** `WT_SNAPSHOTS=0`, second create with unchanged tree.
3. **Cold snapshot build:** `WT_SNAPSHOTS=1`, first create after dependency bump.
4. **Cold ladder build:** `WT_SNAPSHOTS=0`, first create.

**Environment:** Release build, Darwin arm64, APFS, same machine as original measurements [file:46].

**Runs:** 5 runs per scenario, report median and interquartile range.

### 3. Reconcile the Numbers

**Arithmetic checks:**

- **H1 test:** If `clonefile` stage is <1s on warm hit, then H1 is true — the 4.9s unexplained must be elsewhere (ingest, references, overhead).
- **H2 test:** If `clonefile` stage is >2.5s on warm hit, then H2 is true — the model fits.
- **Overhead audit:** Sum of all stages should equal `total` within ~10%. If not, there's unattributed overhead to find.

**Suspects if H1 is true:**

- Ingest-cache TSV lookup: is it a HashMap or a linear scan?
- Per-run ledger parsing: are `verified.tsv` and `ingest-cache.tsv` parsed fresh every run, or cached in memory?
- Benchmark `--verify`: is the verification step contaminating the timing?
- Mirror write: is it one atomic rename or multiple syscalls?

### 4. Fix Any Superlinear Overhead

If H1 is true and unattributed overhead is found:

- Fix the offending stage (e.g., cache TSV lookups in a HashMap, memoize ledger parsing).
- Re-run the breakdown.
- Update the 6.2s and 24s numbers if they change.

**Goal:** Warm snapshot hit should land at ~2–3s under H1 after fixes, not 6.2s.

---

## Acceptance Criteria

- [ ] `WT_TIMING=1` breakdowns published for all four scenarios (warm hit, warm ladder, cold build, cold ladder).
- [ ] H1 vs H2 resolved: `clonefile` stage cost measured and reported.
- [ ] Sum of stages equals `total` within ~10% (or unattributed overhead explained).
- [ ] If H1 is true, overhead fixed and numbers updated.
- [ ] Updated numbers recorded in `HANDOFF.md` §4.

---

## Deliverables

- Patch to `wt-cli` adding extended `WT_TIMING=1` output.
- Benchmark results (markdown table with median and IQR).
- Updated `HANDOFF.md` §4 with reconciled numbers.
- If H1 is true: patch fixing the overhead, with before/after numbers.

---

## Notes

- Do not proceed to v2 implementation until this ticket is complete. The v2 performance model depends entirely on H1 vs H2.
- If H1 is true and overhead is fixed, re-evaluate whether v2 is still worth building — the cold build might drop from 24s to ~12s with overhead fixes alone.
- If H2 is true, the v2 model in the revised design note is valid as-is (~4–8×·speedup).

---

## References

- `HANDOFF.md` §4: measured performance [file:46].
- Original v2 note: `v2-diff-rebuild-design-note.md` (superseded).
- Revised v2 note: `v2-diff-rebuild-design-note-revised.md`.