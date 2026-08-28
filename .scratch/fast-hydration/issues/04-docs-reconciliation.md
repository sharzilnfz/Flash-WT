# 04: Docs reconciliation

**What to build:** The project documents describe what the binary actually does. The feature spec's copy-strategy section still promises whole-directory clonefile hydration; it now names the shipped order — CoW clone by default, byte-copy fallback, experimental hardlink opt-in — and describes materialization from the store accurately. README performance language waits for benchmark numbers rather than asserting them, and the help text matches the flag behavior shipped in ticket 03.

**Blocked by:** 03 (CoW materialization as default) — docs must describe final behavior, not intermediate state.

**Status:** ready-for-agent

- [ ] Feature spec's hydration/copy-strategy lines match shipped behavior; stale whole-directory-clonefile phrasing gone
- [ ] Materialization is described accurately: store-backed, per-file CoW clones, verification at materialize time
- [ ] Hardlink mode documented as experimental opt-in, with the reason (in-place rewrite failures) stated plainly
- [ ] No doc claims a speedup the benchmark suite has not produced
- [ ] Help text for the opt-in flag consistent with documentation
