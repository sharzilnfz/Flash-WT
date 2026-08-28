# 03: snapindex skips the save when record_hit changed nothing

Status: ready-for-agent

The method `SelectionIndex::record_hit` early-returns when the hash is
already at the ring front, but the free function `record_hit`
(crates/wt-store/src/snapindex.rs ~216-266) ignores that and unconditionally
loads the index and saves it through temp+rename. Every snapshot hit pays a
pointless write on the fastest path the tool has.

## Work

1. Make the no-op observable (return `bool changed` from the method or a
   small enum) and skip the save in the free function when nothing changed.
2. Optional if trivial: thread one loaded SelectionIndex through a single
   create instead of reloading per heavy dir. Skip if it turns invasive.
3. Keep the TSV format byte-identical.

## Done when

- cargo test --workspace green; clippy both targets clean; fmt clean.
- A repeated create of an unchanged tree performs zero index writes on the
  hit path (code-readable guarantee).
