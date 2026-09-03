# AGENTS.md

Always use codebase-memory MCP for all searching and querying

## Agent skills

### Issue tracker

Issues live as local markdown under `.scratch/<feature>/`. See `docs/agents/issue-tracker.md`.

### Triage labels

Default five-role vocabulary (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`), recorded as a `Status:` line. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context layout: `CONTEXT.md` glossary plus `docs/adr/`. See `docs/agents/domain.md`.

### Codebase memory

The repo is indexed in the codebase-memory MCP as project `instant-worktrees`.
Refresh the index after pulling or landing code. See `docs/agents/codebase-memory.md`.

## Verification and test performance

- Run `cargo test --lib` for fast unit feedback (under 2 seconds across all crates).
- Run `cargo test -p <crate> --lib` when modifying an isolated crate.
- Run `cargo test` for full workspace verification.
- Run `cargo test -- --ignored` to run the expensive 10,000-file benchmark fixtures.
- Run `scripts/verify/run_all.sh --quick` for end-to-end test suite checks.

### Test optimizations and conventions

- Dependency compilation uses `opt-level = 2` in dev and test profiles via `Cargo.toml` so crypto hashing and I/O loops run fast.
- Store flushes bypass hardware `fsync` in test environments (`WT_TEST_NO_SYNC=1` or `cfg!(test)`). Do not add manual fsync calls into test paths.
- Keep synthetic test fixtures sized around 50 to 200 files. Fixtures exceeding 1,000 files saturate APFS and must carry `#[ignore]`.


