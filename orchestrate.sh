#!/usr/bin/env bash
# Fleet driver for the instant-worktrees build.
# Run this from INSIDE a Herdr-managed pane, at the repo root.
#
#   test "${HERDR_ENV:-}" = 1 && ./orchestrate.sh
#
# Env knobs:
#   AGENT_KIND   herdr agent kind to launch (default: opencode)
#   PHASE        run a single phase: 1|2|3|4 (default: all, pausing between)

set -euo pipefail

test "${HERDR_ENV:-}" = 1 || { echo "Not inside Herdr. Start it from a Herdr pane."; exit 1; }
command -v jq >/dev/null || { echo "jq is required"; exit 1; }

KIND="${AGENT_KIND:-opencode}"
PHASE="${PHASE:-all}"
REPO="$PWD"
ISSUES=".scratch/instant-worktrees/issues"
TICKET_PROMPT='Implement the ticket at %s exactly. Read AGENTS.md and follow it (glossary, ADRs, codebase-memory conventions). Work through the CLI test seam. Commit on your branch when green.'

say() { printf '\n=== %s ===\n' "$*"; }

spawn() { # spawn <name> <ticket-file> [worktree-subdir]
  local name="$1" ticket="$2" wt="${3:-}" pane prompt cwd
  cwd="$REPO"; [ -n "$wt" ] && cwd="$REPO/$wt"
  pane=$(herdr pane split --current --direction down --cwd "$cwd" --no-focus | jq -r '.result.pane.pane_id')
  herdr agent start "$name" --kind "$KIND" --pane "$pane" >/dev/null
  prompt=$(printf "$TICKET_PROMPT" "$ticket")
  herdr agent prompt "$name" "$prompt" --wait --timeout 7200000 \
    || echo "!! agent $name needs attention; inspect with: herdr agent read $name"
  echo "$name"
}

gate() { # gate <label> — human checkpoint + test gate
  say "CHECKPOINT: $1"
  read -r -p "Tests green and work reviewed? Press Enter to continue, Ctrl+C to stop. " _
}

run_tests() {
  say "Running full test suite"
  cargo test --quiet
}

# ---- Phase 1: contracts ----------------------------------------------------
if [[ "$PHASE" == "all" || "$PHASE" == "1" ]]; then
  say "Phase 1: skeleton, contracts, test rig"
  spawn builder-01 "$ISSUES/01-skeleton-contracts-test-rig.md"
  run_tests
  gate "Phase 1 done — contracts are now frozen"
fi

# ---- Phase 2: three agents in parallel -------------------------------------
if [[ "$PHASE" == "all" || "$PHASE" == "2" ]]; then
  say "Phase 2: fan out CLI / backends / store"
  git branch -f fleet/02-cli-and-manifest main 2>/dev/null || git branch -f fleet/02-cli-and-manifest master
  git branch -f fleet/03-copy-backends   main 2>/dev/null || git branch -f fleet/03-copy-backends master
  git branch -f fleet/04-store           main 2>/dev/null || git branch -f fleet/04-store master
  git worktree add .fleet/02 fleet/02-cli-and-manifest
  git worktree add .fleet/03 fleet/03-copy-backends
  git worktree add .fleet/04 fleet/04-store
  pids=()
  spawn cli-agent    "$ISSUES/02-worktree-command-manifest.md" .fleet/02 &
  pids+=($!)
  spawn backend-agent "$ISSUES/03-copy-backends.md"            .fleet/03 &
  pids+=($!)
  spawn store-agent  "$ISSUES/04-content-addressed-store.md"   .fleet/04 &
  pids+=($!)
  wait "${pids[@]}"
  gate "Phase 2 done — three branches ready to merge"
fi

# ---- Phase 3: convergence ---------------------------------------------------
if [[ "$PHASE" == "all" || "$PHASE" == "3" ]]; then
  say "Phase 3: merge branches and wire hydration end to end"
  spawn merger-05 "$ISSUES/05-wire-hydration-through-store.md"
  run_tests
  gate "Phase 5 wired — the product moment is green"
fi

# ---- Phase 4: four agents in parallel ---------------------------------------
if [[ "$PHASE" == "all" || "$PHASE" == "4" ]]; then
  say "Phase 4: GC, hardlink safety, benchmarks, distribution"
  for n in 06-gc 07-hardlink-safety 08-benchmarks 09-distribution; do
    git branch -f "fleet/$n" main 2>/dev/null || git branch -f "fleet/$n" master
    git worktree add ".fleet/$n" "fleet/$n"
  done
  pids=()
  spawn gc-agent     "$ISSUES/06-garbage-collection.md"      .fleet/06-gc             &
  pids+=($!)
  spawn safety-agent "$ISSUES/07-hardlink-safety.md"         .fleet/07-hardlink-safety &
  pids+=($!)
  spawn bench-agent  "$ISSUES/08-benchmark-suite.md"         .fleet/08-benchmarks     &
  pids+=($!)
  spawn dist-agent   "$ISSUES/09-distribution.md"            .fleet/09-distribution   &
  pids+=($!)
  wait "${pids[@]}"
  run_tests
  say "All phases complete. Merge fleet branches in numeric order, re-index codebase-memory, then clean .fleet/ worktrees."
fi
