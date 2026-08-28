# 10: Append-only snapshot write-ahead journal and sweep compaction

**What to build:** Replace whole-file `index.tsv` and `lru.tsv` load-modify-rename cycles with an append-only write-ahead log located at `<store>/snapshots/journal.tsv`. Record snapshot publishes and cache hits using atomic POSIX appends (`O_APPEND`) without holding a global store lock. Update `select_old_snapshot` to read recent entries directly from the journal. During `wt sweep`, acquire an exclusive lock, compact journal records into canonical index files, fsync, and truncate the journal.

**Blocked by:** None (can start immediately).

**Status:** ready-for-agent

- [ ] Snapshot hit and publish operations append single-line TSV entries to `<store>/snapshots/journal.tsv` using `O_APPEND`.
- [ ] Concurrency-sensitive snapshot lookup logic reads live state from the journal.
- [ ] Snapshot metadata updates eliminate global read-modify-write file locking.
- [ ] `wt sweep` acquires exclusive metadata lock, compacts `journal.tsv` into `index.tsv` and `lru.tsv`, fsyncs, and truncates the journal.
- [ ] Crash resilience and concurrency stress tests verify zero lost updates across concurrent agent processes.
