# 05: Automated chaos and fault-injection runner

**What to build:** An automated chaos test harness that injects asynchronous process terminations (`SIGKILL`, `SIGTERM`), simulated I/O write failures, and corrupted store blobs at precise lifecycle phases, verifying self-healing and zero projection corruption.

**Blocked by:** 02: Automated baseline-versus-candidate runner

**Status:** ready-for-agent

- [ ] Lifecycle phase signal interceptor triggering `SIGKILL` during active blob ingest, directory link train construction, snapshot atomic rename, and GC sweep.
- [ ] Post-crash integrity validator asserting that interrupted operations leave no partially written projections and zero corrupted CAS entries.
- [ ] Self-healing verification asserting that subsequent `wt create` commands successfully repair missing blobs or staged files and complete cleanly.
- [ ] GC resilience validator asserting that crash interrupted sweeps never collect referenced live data.
- [ ] Chaos suite runner producing structured recovery reports and exit codes for CI pipelines.
