# 12: 256-shard parallel store and snapshot integrity scrubbing

**What to build:** Partition store scrubbing across 256 hash prefix shards (`00` to `ff`) and process shards in parallel worker threads. Verify published snapshot manifests, `.complete` directory markers, and referenced snapshot file trees alongside raw object blobs. Surface corrupted blobs and broken snapshot trees in structured JSON and human reports so orchestrators can automate repair operations.

**Blocked by:** 01: Versioned JSON output envelope across CLI commands.

**Status:** ready-for-agent

- [ ] Store blob verification splits across 256 hash prefix shards and executes in parallel.
- [ ] Published snapshot directories are scrubbed for missing `.complete` markers, unparseable manifests, and broken file trees.
- [ ] Corrupted blobs and broken snapshot directories are flagged and reported in scan summaries.
- [ ] Structured `--json` envelopes emit detailed diagnostic arrays on corruption detection.
- [ ] Scrubbing integration tests verify parallel scaling and detection of simulated disk corruptions.
