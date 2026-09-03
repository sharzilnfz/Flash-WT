# 03: decompose flashwt-cli, liberate the matcher, typed errors

Status: ready-for-agent
Owner branch: `arch/cli-refactor`
Owns: `crates/flashwt-cli/**`.

## Problem

main.rs is a god file: CLI defs, a hand-rolled gitignore glob engine, a
~300-line create() whose emit closure takes 14 positional arguments beside 15
mutable timing accumulators, and inline git plumbing duplicated with gc.rs.
Zero unit tests exist in flashwt-cli because every pure function is trapped behind
the binary boundary. Errors are `Result<_, String>` everywhere, every failure
exits 1, and env vars are hidden globals with three different activation
semantics (FLASHWT_HARDLINK=0 currently turns hardlink mode ON).

## Work

1. New `manifest.rs`: move DEFAULT_PATTERNS, STARTER_MANIFEST, parse_patterns,
   pattern_matches, glob_match, segment_match, collect_matches, load_patterns.
   Split load_patterns' side effect from its decision: return
   `Loaded { patterns } | CreatedStarter { patterns }`, let the caller print.
   Add unit tests: anchored vs unanchored, ** across segments, negation,
   nested-match dedup. While moving, convert the strip_prefix/rel_text expects
   to Results ("pattern matched path outside repository root").
2. Decompose main.rs:
   - `cli.rs`: Cli, FlashwtCommand, StoreAction definitions.
   - `commands/{create,remove,sweep,migrate}.rs`: thin handlers; extract
     hydrate_one_dir from the create loop.
   - `timing.rs`: StageTimings struct replacing the emit closure and 15
     accumulators; call sites become `timings.snapshot_ms += ...`.
   - `gitops.rs`: repo_root(), git_dir(worktree), run(dir,args),
     default_worktree_dest(root,name). Deduplicate main.rs vs gc.rs
     (run_git, inline rev-parse, sibling-path computation).
3. Parse env config once at startup into `RunConfig { strategy_policy,
   verify, snapshots, v2, timing }`; thread slices through materialize,
   snapshots::hydrate, ingest_dir. Normalize FLASHWT_HARDLINK/FLASHWT_NO_HARDLINK to
   honor value "0"/"1"/presence consistently and align long_about docs.
   Check tests/hardlink_safety.rs before choosing semantics; keep e2e tests
   passing.
4. Typed errors: introduce a thiserror `Error` enum (Git, Store, Io{path,
   source}, Usage). main returns mapped exit codes; clap keeps exit 2 for
   parse errors. Keep message text identical where tests grep stderr.
5. Clap should own validation: ArgGroup (required) for the migrate cutover
   mutual exclusion; custom value_parser wrapping parse_age so bad durations
   die at parse time before side effects.
6. `parse_age` overflow: count * secs must be checked_mul; None flows into
   the existing invalid-age error path. This is a real release-mode wraparound
   hazard in the GC safety story.
7. Write the starter manifest via temp + rename (house style) instead of bare
   fs::write.

## Constraints

- The `flashwt-stage` stderr contract stays byte-identical; tests parse it.
- Pluralization micro-logic and strategy-summary wording may move into a small
  Reporter only if rendered strings stay identical. Skip if it balloons.
- No signal handling additions beyond optional signposting; grace period +
  mirror repair is the designed interrupt story (ADR-0004).

## Done when

- `cargo test -p flashwt-cli` passes (all integration suites).
- `cargo clippy -p flashwt-cli --all-targets` clean.
- main.rs under ~120 lines; matcher has unit tests runnable without spawning
  the binary.

## Comments

Completed on `arch/hardening-and-simplify` via wave 2 of the orchestrated build. See git log for the implementing commits.
