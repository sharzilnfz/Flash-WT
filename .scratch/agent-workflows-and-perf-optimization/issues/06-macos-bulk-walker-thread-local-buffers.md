# 06: macOS bulk directory walker thread-local scratch buffers

**What to build:** Refactor the macOS bulk directory walker to allocate a reusable 32KB `thread_local!` scratch buffer per worker thread. Parse kernel return entry counts and byte lengths strictly to prevent ghost entry corruptions. Double buffer capacity dynamically when receiving `libc::ERANGE` errors up to a 1MB ceiling. Ensure thread-safe, allocation-free directory traversal during parallel heavy directory scanning.

**Blocked by:** None (can start immediately).

**Status:** ready-for-agent

- [x] Worker threads in the macOS bulk walker reuse a `thread_local!` 32KB scratch buffer.
- [x] Traversal loop parses returned entry count and byte length strictly, reading only valid records.
- [x] Buffer capacity doubles dynamically upon receiving `libc::ERANGE` up to a maximum 1MB ceiling.
- [x] Allocation overhead during repeated directory traversals drops to near zero.
- [x] Bulk walk unit tests verify entry fidelity, length boundary safety, and absence of ghost entries.
