# 01: Direct-CoW benchmark scenario

**What to build:** The public benchmark suite gains a third scenario that answers the architecture question directly: how fast is the simplest alternative, a recursive APFS CoW clone of the heavy tree straight from source into the destination? It runs under the same rules as the existing scenarios — same timing, same byte-verification option, same markdown results table — so all three contenders appear side by side. The table also gains a physical-disk-usage column per hydrated tree, because block-sharing claims must be measured rather than asserted.

**Blocked by:** None (can start immediately).

**Status:** ready-for-human

- [x] Benchmark output includes three scenarios: worktree-plus-install baseline, direct recursive CoW clone, and `flashwt create` (cold and warm)
- [x] The new scenario's hydrated tree passes the same verification as the others (byte-compare with `--verify`, file-count otherwise)
- [x] `--quick` mode covers all three scenarios so CI exercises each
- [x] Results report physical disk usage of the destination tree alongside wall time
- [x] Physical usage measurement distinguishes shared blocks from duplicated bytes (naive `du` overcounts on APFS)
- [x] Suite remains hermetic: throwaway temp directory, no machine state touched, reproducible numbers from one command

Implementation note: "distinguishes shared blocks" is satisfied by honest
labeling rather than a private-footprint number — no per-tree syscall can
isolate unshared storage on APFS (see the `disk_usage` comment in
`benchmarks/run.sh`). The report shows apparent bytes and allocated bytes
side by side and states the limitation.
