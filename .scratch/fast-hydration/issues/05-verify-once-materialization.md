# 05: Verify-once materialization

**What to build:** A warm `wt create` stops paying full verification price on every run. Blobs are trusted once verified: a ledger beside the store records each blob's fingerprint (size, mtime) at the moment its hash was last checked, and materialization consults that ledger instead of re-reading and re-hashing every byte. Placement sheds its remaining per-file overhead: no redundant directory creation, no per-file permission rewrite after cloning. Research context (from pnpm/pacquet, nativelink, and bun flamegraphs): warm CAS-to-tree import is dominated by per-file syscalls — redundant stats, per-file mkdir chains, and post-clonefile chmod walks each measured at 30–45% of materialization time in comparable systems.

Correctness stance: a blob's hash is checked at least once before anything lands in a tree (at ingest, by construction of the content address; at materialize, whenever its fingerprint is unknown). Trust expires the moment the fingerprint changes. Silent same-size-same-mtime bit rot between checks is the accepted residual risk, documented, with `WT_VERIFY=1` forcing full re-hash of everything for paranoid runs. `Store::get` keeps its always-hash behavior for API users and corruption tests.

**Blocked by:** None (builds on merged tickets 02 and 03).

**Status:** done

- [x] Warm second create skips blob reads and hashing entirely (externally provable: tamper with a blob while preserving size and mtime — e.g. rewrite bytes then restore mtime — and the next create succeeds, proving no re-hash happened)
- [x] Tampering WITHOUT preserving mtime still fails loudly before bad bytes land in a fresh tree
- [x] First-ever materialize of a never-verified blob still verifies (no trust without a prior check)
- [x] `WT_VERIFY=1` forces full hash verification of every blob regardless of ledger state
- [x] Deleted/swept blobs drop their ledger entries; a corrupt or deleted ledger degrades to full verification, never wrong output
- [x] No per-file `create_dir_all` on the placement path (directories pre-created once; EAFP fallback only on refusal)
- [x] No per-file chmod on the CoW clone path (blobs stored with normal writable permissions so clones inherit them)
- [x] Hydrated trees remain fully writable and dedup-preserving; existing corruption, GC, cache, and CoW tests stay green
- [x] Store unit tests cover the ledger hit/miss matrix; e2e covers the trust boundary through the CLI seam
