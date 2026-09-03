#!/usr/bin/env bash

set -euo pipefail

SUITE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VERIFY_DIR="$(cd "$SUITE_DIR/.." && pwd)"

. "$VERIFY_DIR/harness.sh"
. "$VERIFY_DIR/generators.sh"

suite_init "02_flash_apfs" "Flash-WT APFS Snapshot Performance & Scalability"

QUICK_MODE="${QUICK:-0}"
if [ "$QUICK_MODE" -eq 1 ]; then
    BENCH_FILES=2000
else
    BENCH_FILES="${FLASHWT_BENCH_FILES:-40000}"
fi

setup_isolated_fixture "apfs-bench"

echo "Generating Scenario D tree with $BENCH_FILES files..."
mkdir -p "$FIXTURE_ORIGIN/node_modules"
generate_tree_d "$FIXTURE_ORIGIN/node_modules" "$BENCH_FILES"

cat << 'EOF' > "$FIXTURE_ORIGIN/.flashwtinclude"
node_modules/
EOF

cd "$FIXTURE_ORIGIN"
git add .flashwtinclude
git commit -qm "add manifest for apfs benchmark"

test_start "cold_build" "Measure cold build hydration and stage timings"
t0=$(now)
FLASHWT_TIMING=1 flashwt create cold-worktree --dir "$FIXTURE_DIR/cold-worktree" 2> "$FIXTURE_DIR/cold.log"
t1=$(now)
cold_wall_sec=$(elapsed "$t0" "$t1")
cold_wall_ms=$(elapsed_ms "$t0" "$t1")

assert_file_exists "$FIXTURE_DIR/cold-worktree/node_modules/.bin" "cold worktree files missing"
assert_triple_axis "$FIXTURE_ORIGIN/node_modules" "$FIXTURE_DIR/cold-worktree/node_modules" "cold tree triple-axis failed"

parse_stage_log "$FIXTURE_DIR/cold.log" > "$FIXTURE_DIR/cold_stages.env"
test_pass "cold_build" "{\"wall_ms\": $cold_wall_ms, \"files\": $BENCH_FILES}"

test_start "warm_snapshot_hit" "Measure warm snapshot hit (whole-tree APFS clonefile)"
t0=$(now)
FLASHWT_TIMING=1 flashwt create warm-worktree --dir "$FIXTURE_DIR/warm-worktree" 2> "$FIXTURE_DIR/warm.log"
t1=$(now)
warm_wall_sec=$(elapsed "$t0" "$t1")
warm_wall_ms=$(elapsed_ms "$t0" "$t1")

assert_file_exists "$FIXTURE_DIR/warm-worktree/node_modules/.bin" "warm worktree files missing"
assert_triple_axis "$FIXTURE_ORIGIN/node_modules" "$FIXTURE_DIR/warm-worktree/node_modules" "warm tree triple-axis failed"

speedup_warm_vs_cold=$(awk -v c="$cold_wall_ms" -v w="$warm_wall_ms" 'BEGIN { printf "%.2f", (w > 0 ? c / w : c) }')

test_pass "warm_snapshot_hit" "{\"warm_wall_ms\": $warm_wall_ms, \"cold_wall_ms\": $cold_wall_ms, \"speedup_vs_cold\": $speedup_warm_vs_cold}"

test_start "per_file_fallback" "Measure per-file ladder fallback with FLASHWT_SNAPSHOTS=0"
t0=$(now)
FLASHWT_SNAPSHOTS=0 FLASHWT_TIMING=1 flashwt create fallback-worktree --dir "$FIXTURE_DIR/fallback-worktree" 2> "$FIXTURE_DIR/fallback.log"
t1=$(now)
fallback_wall_ms=$(elapsed_ms "$t0" "$t1")

assert_file_exists "$FIXTURE_DIR/fallback-worktree/node_modules/.bin" "fallback worktree files missing"
assert_triple_axis "$FIXTURE_ORIGIN/node_modules" "$FIXTURE_DIR/fallback-worktree/node_modules" "fallback tree triple-axis failed"

speedup_snapshot_vs_fallback=$(awk -v f="$fallback_wall_ms" -v w="$warm_wall_ms" 'BEGIN { printf "%.2f", (w > 0 ? f / w : f) }')

test_pass "per_file_fallback" "{\"fallback_wall_ms\": $fallback_wall_ms, \"snapshot_v1_warm_ms\": $warm_wall_ms, \"speedup\": $speedup_snapshot_vs_fallback}"

test_start "snapshot_v2_diff" "Measure Snapshot v2 incremental rebuild on 3 modified packages"
printf '// v2 mutation 1\nmodule.exports = { bump: 1 };\n' > "$FIXTURE_ORIGIN/node_modules/pkg-00000/lib/mod-0.js"
printf '// v2 mutation 2\nmodule.exports = { bump: 2 };\n' > "$FIXTURE_ORIGIN/node_modules/pkg-00001/lib/mod-0.js"
printf '// v2 mutation 3\nmodule.exports = { bump: 3 };\n' > "$FIXTURE_ORIGIN/node_modules/pkg-00002/lib/mod-0.js"

t0=$(now)
FLASHWT_SNAPSHOTS=1 FLASHWT_SNAPSHOTS_V2=1 FLASHWT_TIMING=1 flashwt create v2-worktree --dir "$FIXTURE_DIR/v2-worktree" 2> "$FIXTURE_DIR/v2.log"
t1=$(now)
v2_wall_ms=$(elapsed_ms "$t0" "$t1")

assert_file_exists "$FIXTURE_DIR/v2-worktree/node_modules/pkg-00000/lib/mod-0.js" "v2 worktree file missing"
assert_triple_axis "$FIXTURE_ORIGIN/node_modules" "$FIXTURE_DIR/v2-worktree/node_modules" "v2 tree triple-axis parity check"

test_pass "snapshot_v2_diff" "{\"v2_wall_ms\": $v2_wall_ms, \"cold_wall_ms\": $cold_wall_ms}"

test_start "cache_poisoning" "Resilience against tree poisoning (single extraneous .DS_Store file)"
touch "$FIXTURE_ORIGIN/node_modules/.DS_Store"

t0=$(now)
FLASHWT_TIMING=1 flashwt create poisoned-worktree --dir "$FIXTURE_DIR/poisoned-worktree" 2> "$FIXTURE_DIR/poison.log"
t1=$(now)
poison_wall_ms=$(elapsed_ms "$t0" "$t1")

assert_file_exists "$FIXTURE_DIR/poisoned-worktree/node_modules/.DS_Store" "poisoned file missing in worktree"
assert_triple_axis "$FIXTURE_ORIGIN/node_modules" "$FIXTURE_DIR/poisoned-worktree/node_modules" "poison recovery parity check"
test_pass "cache_poisoning" "{\"recovery_wall_ms\": $poison_wall_ms}"

test_start "stage_timings" "Verify detailed flashwt-stage telemetry breakdowns from logs"
cold_stages=$(parse_stage_log "$FIXTURE_DIR/cold.log")
[ -n "$cold_stages" ] || test_fail "stage_timings" "no flashwt-stage timings found in cold.log"

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

test_start "raw_copy_comparison" "Compare Flash-WT warm snapshot against raw copy (cp -Rc)"
mkdir -p "$FIXTURE_DIR/cp-target"
t0=$(now)
cp -Rc "$FIXTURE_ORIGIN/node_modules" "$FIXTURE_DIR/cp-target/node_modules"
t1=$(now)
cp_wall_ms=$(elapsed_ms "$t0" "$t1")

speedup_vs_cp=$(awk -v cp="$cp_wall_ms" -v w="$warm_wall_ms" 'BEGIN { printf "%.2f", (w > 0 ? cp / w : cp) }')

test_pass "raw_copy_comparison" "{\"cp_wall_ms\": $cp_wall_ms, \"warm_flashwt_ms\": $warm_wall_ms, \"speedup\": $speedup_vs_cp}"

teardown_isolated_fixture "$FIXTURE_DIR"

suite_finish

