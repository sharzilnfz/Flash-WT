# 04: CLI Hydration Cutover and Shim Deletion

**What to build:** Migrate `flashwt create`, `flashwt scratch`, and `flashwt demo` to call `store.hydrate(request)`. Delete the 172-line shallow forwarding shim in `crates/flashwt-cli/src/snapshots.rs`. Delete manual sidecar parsing, mirror writing, and device probing from `crates/flashwt-cli/src/hydrate.rs`. Verify all existing CLI test flows against the deepened engine.

**Blocked by:** 03: Deep Hydration Engine and Storage Ledger in flashwt-store

**Status:** ready-for-agent

- [x] `flashwt create`, `flashwt scratch`, and `flashwt demo` invoke `DiskStore::hydrate` and format user-visible receipts from the returned structure
- [x] The shallow forwarding module `crates/flashwt-cli/src/snapshots.rs` is deleted
- [x] Manual sidecar TSV writing and mirror publishing logic are removed from `crates/flashwt-cli/src/hydrate.rs`
- [x] Terminal I/O and configuration parsing are decoupled from storage hydration orchestration
- [x] Full CLI test suite (`new.rs`, `scratch.rs`, `demo.rs`, `snapshots.rs`, `snapshots_v2.rs`) passes without regression
