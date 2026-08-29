# 06: Unified Hydration Filter and Compiler Cache Exclusions

**What to build:** Consolidate `.wtinclude` pattern parsing, starter manifest generation, and volatile compiler cache detection into a single deep `HydrationFilter` module. Eliminate duplicate exclusion rules across pattern defaults and toolchain filters, and resolve the domain naming collision with snapshot manifests.

**Blocked by:** 04: CLI Hydration Cutover and Shim Deletion

**Status:** ready-for-agent

- [x] Consolidate `.wtinclude` parsing and volatile compiler cache filtering into `HydrationFilter`
- [x] Rename `crates/wt-cli/src/manifest.rs` to reflect hydration filter responsibilities and eliminate the naming collision with snapshot manifests
- [x] Ensure default pattern rules and compiler cache filters share a single source of truth for exclusions (`.vite/`, `target/debug/incremental/`, `.next/cache/`)
- [x] Update call sites in ingestion and command setup to consume `HydrationFilter`
- [x] Integration tests verify that volatile compiler caches are excluded during hydration across all supported toolchains
