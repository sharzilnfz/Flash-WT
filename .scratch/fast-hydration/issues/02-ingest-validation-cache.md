# 02: Ingest validation cache

**What to build:** Warm `wt create` stops paying for unchanged files. A validation cache kept beside the store records, for every ingested path, its size, mtime, and content id from the last run. On the next ingest, files whose size and mtime still match reuse their existing blob without being read or hashed; everything else goes through the normal read-and-hash path. A user who edits one package between worktree creations pays only for that package. Existing stores keep working — the cache is additive, not a store format change.

Correctness does not depend on trusting mtimes: hash verification at materialize time stays in place, so if cached metadata ever lies about content, the mismatch fails loudly before bad bytes land in a fresh tree.

**Blocked by:** None (can start immediately).

**Status:** ready-for-agent

- [ ] Second create against an unchanged source adds no new store content and reads no unchanged file bytes (warm ingest cost scales with what changed, not tree size)
- [ ] Editing a file between creations puts its new content in the next hydrated tree
- [ ] Touching a file (mtime bump, same bytes) cannot produce wrong bytes in a hydrated tree
- [ ] Changing file size with matching mtime is treated as a miss and re-hashed
- [ ] A pre-existing store with no cache yet ingests correctly and populates the cache
- [ ] Deleting or corrupting the cache degrades to full re-ingest, never to wrong output
- [ ] Store unit tests cover hit/miss matrix; e2e tests cover staleness through the CLI seam
