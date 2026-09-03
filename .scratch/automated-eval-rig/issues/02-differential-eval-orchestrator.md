# 02: Automated baseline-versus-candidate runner

**What to build:** An automated runner that checks out two git revisions (or accepts prebuilt binaries), compiles release targets, provisions isolated temporary storage roots, executes randomized/interleaved cold and warm benchmark runs, and computes differential statistics and stage deltas.

**Blocked by:** 01: JSON metrics schema and stage metrics collector

**Status:** ready-for-agent

- [ ] Automated git worktree provisioning for compiling baseline and candidate binaries in isolation.
- [ ] Test orchestrator managing isolated `FLASHWT_STORE` and throwaway origin git repositories per run.
- [ ] Interleaved run scheduler alternating baseline and candidate executions across N runs (default N=5) to eliminate thermal and disk cache bias.
- [ ] Automated differential analysis calculating absolute differences and percentage changes for every stage metric and wall-clock total.
- [ ] Integration tests demonstrating end-to-end differential comparison between two builds on small fixtures.
