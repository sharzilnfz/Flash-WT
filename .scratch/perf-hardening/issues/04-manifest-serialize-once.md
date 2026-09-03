# 04: serialize the manifest once; reuse bytes for hash and write

Status: ready-for-agent

Every snapshot build serializes entries twice: inside Manifest::new to compute
the content hash, and again in stage_and_publish to write manifest.tsv. Each
pass allocates per entry (escape String, octal mode format, blob id String):
roughly a quarter-million short-lived allocations per pass at 40k entries.

## Work

1. Serialize once into a pre-reserved Vec<u8> during Manifest::new; store the
   bytes (private field) and reuse them for the durable_write of
   manifest.tsv in snapshot/publish.rs. Hash the same bytes.
2. Same treatment for StoreMirror::serialize (crates/flashwt-store/src/mirror.rs)
   where cheap: pre-reserve, avoid per-id to_string() chains. Do not change
   output formats — tests pin them.
3. Watch out for borrows: if self-referential storage of bytes fights the
   borrow checker, prefer returning bytes alongside the Manifest from the
   constructor over lifetime gymnastics.

## Done when

- cargo test --workspace green; clippy both targets clean; fmt clean.
- serialize_entries is called exactly once per build (grep-verifiable).
