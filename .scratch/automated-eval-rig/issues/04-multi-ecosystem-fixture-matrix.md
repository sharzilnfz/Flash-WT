# 04: Multi-ecosystem fixture matrix and fan-out generator

**What to build:** Deterministic fixture generation modules for non-JS ecosystems (Rust `target/` trees and Python `.venv` virtualenvs) and concurrent fan-out workload generators simulating multiple agents creating worktrees simultaneously.

**Blocked by:** 02: Automated baseline-versus-candidate runner

**Status:** ready-for-agent

- [ ] Rust `target/` fixture generator producing realistic debug binaries, incremental compilation artifact hashes, and rlibs across multi-crate layouts.
- [ ] Python `.venv` fixture generator creating nested `site-packages`, bytecode cache `.pyc` files, and executable wrapper symlinks in `bin/`.
- [ ] Concurrent fan-out generator launching 10 to 50 parallel `wt create` worker processes targeting the same shared store root.
- [ ] Concurrency metrics collector tracking lock wait durations, race conditions, and peak memory consumption during parallel hydration.
- [ ] Fixture validation tests verifying that generated directory trees conform to expected file counts, directory depths, and deduplication ratios.
