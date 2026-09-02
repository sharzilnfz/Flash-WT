#!/usr/bin/env bash
# scripts/verify/suites/02_flash_apfs.sh — Flash WT APFS Snapshot Performance & Benchmarks.
#
# Verifies:
#  1. Cold build vs warm snapshot hit on large 40,000-file tree.
#  2. Snapshot v1 (whole-tree clonefile) vs per-file ladder fallback (WT_SNAPSHOTS=0).
#  3. Snapshot v2 diff-based incremental rebuild (WT_SNAPSHOTS_V2=1) on 3-package mutation.
#  4. Cache poisoning resilience: touch single file (.DS_Store) and measure recovery.
#  5. Stage timing breakdown with WT_TIMING=1 (ingest, references, materialize, total).
#  6. Comparison against raw recursive copy `cp -Rc`.

set -euo pipefail

SUITE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VERIFY_DIR="$(cd "$SUITE_DIR/.." && pwd)"

# shellcheck disable=SC1091
. "$VERIFY_DIR/harness.sh"
# shellcheck disable=SC1091
. "$VERIFY_DIR/generators.sh"

suite_init "02_flash_apfs" "Flash WT APFS Snapshot Performance & Scalability"

# Determine file count based on QUICK flag or WT_BENCH_FILES
QUICK_MODE="${QUICK:-0}"
if [ "$QUICK_MODE" -eq 1 ]; then
    BENCH_FILES=2000
else
    BENCH_FILES="${WT_BENCH_FILES:-40000}"
fi

setup_isolated_fixture "apfs-bench"

echo "Generating Scenario D tree with $BENCH_FILES files..."
mkdir -p "$FIXTURE_ORIGIN/node_modules"
generate_tree_d "$FIXTURE_ORIGIN/node_modules" "$BENCH_FILES"

cat << 'EOF' > "$FIXTURE_ORIGIN/.wtinclude"
node_modules/
EOF

cd "$FIXTURE_ORIGIN"
git add .wtinclude
git commit -qm "add manifest for apfs benchmark"

# -----------------------------------------------------------------------------
# 1. Cold Build vs Warm Snapshot Hit
# -----------------------------------------------------------------------------
test_start "cold_build" "Measure cold build hydration and stage timings"
t0=$(now)
WT_TIMING=1 wt create cold-wt --dir "$FIXTURE_DIR/cold-wt" 2> "$FIXTURE_DIR/cold.log"
t1=$(now)
cold_wall_sec=$(elapsed "$t0" "$t1")
cold_wall_ms=$(elapsed_ms "$t0" "$t1")

assert_file_exists "$FIXTURE_DIR/cold-wt/node_modules/.bin" "cold worktree files missing"
assert_triple_axis "$FIXTURE_ORIGIN/node_modules" "$FIXTURE_DIR/cold-wt/node_modules" "cold tree triple-axis failed"

# Parse stage timings
parse_stage_log "$FIXTURE_DIR/cold.log" > "$FIXTURE_DIR/cold_stages.env"
test_pass "cold_build" "{\"wall_ms\": $cold_wall_ms, \"files\": $BENCH_FILES}"

test_start "warm_snapshot_hit" "Measure warm snapshot hit (whole-tree APFS clonefile)"
t0=$(now)
WT_TIMING=1 wt create warm-wt --dir "$FIXTURE_DIR/warm-wt" 2> "$FIXTURE_DIR/warm.log"
t1=$(now)
warm_wall_sec=$(elapsed "$t0" "$t1")
warm_wall_ms=$(elapsed_ms "$t0" "$t1")

assert_file_exists "$FIXTURE_DIR/warm-wt/node_modules/.bin" "warm worktree files missing"
assert_triple_axis "$FIXTURE_ORIGIN/node_modules" "$FIXTURE_DIR/warm-wt/node_modules" "warm tree triple-axis failed"

# Warm hit should be significantly faster than cold
speedup_warm_vs_cold=$(awk -v c="$cold_wall_ms" -v w="$warm_wall_ms" 'BEGIN { printf "%.2f", (w > 0 ? c / w : c) }')

test_pass "warm_snapshot_hit" "{\"warm_wall_ms\": $warm_wall_ms, \"cold_wall_ms\": $cold_wall_ms, \"speedup_vs_cold\": $speedup_warm_vs_cold}"

# -----------------------------------------------------------------------------
# 2. Snapshot v1 vs Per-File Ladder Fallback (WT_SNAPSHOTS=0)
# -----------------------------------------------------------------------------
test_start "per_file_fallback" "Measure per-file ladder fallback with WT_SNAPSHOTS=0"
t0=$(now)
WT_SNAPSHOTS=0 WT_TIMING=1 wt create fallback-wt --dir "$FIXTURE_DIR/fallback-wt" 2> "$FIXTURE_DIR/fallback.log"
t1=$(now)
fallback_wall_ms=$(elapsed_ms "$t0" "$t1")

assert_file_exists "$FIXTURE_DIR/fallback-wt/node_modules/.bin" "fallback worktree files missing"
assert_triple_axis "$FIXTURE_ORIGIN/node_modules" "$FIXTURE_DIR/fallback-wt/node_modules" "fallback tree triple-axis failed"

speedup_snapshot_vs_fallback=$(awk -v f="$fallback_wall_ms" -v w="$warm_wall_ms" 'BEGIN { printf "%.2f", (w > 0 ? f / w : f) }')

test_pass "per_file_fallback" "{\"fallback_wall_ms\": $fallback_wall_ms, \"snapshot_v1_warm_ms\": $warm_wall_ms, \"speedup\": $speedup_snapshot_vs_fallback}"

# -----------------------------------------------------------------------------
# 3. Snapshot v2 Diff-Based Incremental Rebuild (WT_SNAPSHOTS_V2=1)
# -----------------------------------------------------------------------------
test_start "snapshot_v2_diff" "Measure Snapshot v2 incremental rebuild on 3 modified packages"
# Modify 3 packages in origin
printf '// v2 mutation 1\nmodule.exports = { bump: 1 };\n' > "$FIXTURE_ORIGIN/node_modules/pkg-00000/lib/mod-0.js"
printf '// v2 mutation 2\nmodule.exports = { bump: 2 };\n' > "$FIXTURE_ORIGIN/node_modules/pkg-00001/lib/mod-0.js"
printf '// v2 mutation 3\nmodule.exports = { bump: 3 };\n' > "$FIXTURE_ORIGIN/node_modules/pkg-00002/lib/mod-0.js"

t0=$(now)
WT_SNAPSHOTS=1 WT_SNAPSHOTS_V2=1 WT_TIMING=1 wt create v2-wt --dir "$FIXTURE_DIR/v2-wt" 2> "$FIXTURE_DIR/v2.log"
t1=$(now)
v2_wall_ms=$(elapsed_ms "$t0" "$t1")

assert_file_exists "$FIXTURE_DIR/v2-wt/node_modules/pkg-00000/lib/mod-0.js" "v2 worktree file missing"
assert_triple_axis "$FIXTURE_ORIGIN/node_modules" "$FIXTURE_DIR/v2-wt/node_modules" "v2 tree triple-axis parity check"

# Snapshot v2 should achieve sub-second rebuild
test_pass "snapshot_v2_diff" "{\"v2_wall_ms\": $v2_wall_ms, \"cold_wall_ms\": $cold_wall_ms}"

# -----------------------------------------------------------------------------
# 4. Cache Poisoning Resilience
# -----------------------------------------------------------------------------
test_start "cache_poisoning" "Resilience against tree poisoning (single extraneous .DS_Store file)"
touch "$FIXTURE_ORIGIN/node_modules/.DS_Store"

t0=$(now)
WT_TIMING=1 wt create poisoned-wt --dir "$FIXTURE_DIR/poisoned-wt" 2> "$FIXTURE_DIR/poison.log"
t1=$(now)
poison_wall_ms=$(elapsed_ms "$t0" "$t1")

assert_file_exists "$FIXTURE_DIR/poisoned-wt/node_modules/.DS_Store" "poisoned file missing in worktree"
assert_triple_axis "$FIXTURE_ORIGIN/node_modules" "$FIXTURE_DIR/poisoned-wt/node_modules" "poison recovery parity check"
test_pass "cache_poisoning" "{\"recovery_wall_ms\": $poison_wall_ms}"

# -----------------------------------------------------------------------------
# 5. Stage Timing Breakdown (WT_TIMING=1)
# -----------------------------------------------------------------------------
test_start "stage_timings" "Verify detailed wt-stage telemetry breakdowns from logs"
cold_stages=$(parse_stage_log "$FIXTURE_DIR/cold.log")
[ -n "$cold_stages" ] || test_fail "stage_timings" "no wt-stage timings found in cold.log"

stages_json=$(python3 -c "
import sys, json
data = {}
for line in sys.stdin:
    line = line.strip()
    if '=' in line:
        k, v = line.split('=', 1)
        data[k] = v
print(json.dumps(data))
" <<< "$cold_stages")

test_pass "stage_timings" "$stages_json"

# -----------------------------------------------------------------------------
# 6. Comparison Against Raw Recursive Copy cp -Rc
# -----------------------------------------------------------------------------
test_start "raw_copy_comparison" "Compare Flash WT warm snapshot against raw copy (cp -Rc)"
mkdir -p "$FIXTURE_DIR/cp-target"
t0=$(now)
cp -Rc "$FIXTURE_ORIGIN/node_modules" "$FIXTURE_DIR/cp-target/node_modules"
t1=$(now)
cp_wall_ms=$(elapsed_ms "$t0" "$t1")

speedup_vs_cp=$(awk -v cp="$cp_wall_ms" -v w="$warm_wall_ms" 'BEGIN { printf "%.2f", (w > 0 ? cp / w : cp) }')

test_pass "raw_copy_comparison" "{\"cp_wall_ms\": $cp_wall_ms, \"warm_wt_ms\": $warm_wall_ms, \"speedup\": $speedup_vs_cp}"

# Cleanup
teardown_isolated_fixture "$FIXTURE_DIR"

suite_finish
