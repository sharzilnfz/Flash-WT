# 01: Direct-CoW benchmark scenario

**What to build:** The public benchmark suite gains a third scenario that answers the architecture question directly: how fast is the simplest alternative, a recursive APFS CoW clone of the heavy tree straight from source into the destination? It runs under the same rules as the existing scenarios — same timing, same byte-verification option, same markdown results table — so all three contenders appear side by side. The table also gains a physical-disk-usage column per hydrated tree, because block-sharing claims must be measured rather than asserted.

**Blocked by:** None (can start immediately).

**Status:** ready-for-agent

- [ ] Benchmark output includes three scenarios: worktree-plus-install baseline, direct recursive CoW clone, and `wt create` (cold and warm)
- [ ] The new scenario's hydrated tree passes the same verification as the others (byte-compare with `--verify`, file-count otherwise)
- [ ] `--quick` mode covers all three scenarios so CI exercises each
- [ ] Results report physical disk usage of the destination tree alongside wall time
- [ ] Physical usage measurement distinguishes shared blocks from duplicated bytes (naive `du` overcounts on APFS)
- [ ] Suite remains hermetic: throwaway temp directory, no machine state touched, reproducible numbers from one command
