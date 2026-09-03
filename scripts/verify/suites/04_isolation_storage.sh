#!/usr/bin/env bash

set -euo pipefail

SUITE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VERIFY_DIR="$(cd "$SUITE_DIR/.." && pwd)"

. "$VERIFY_DIR/harness.sh"
. "$VERIFY_DIR/generators.sh"

suite_init "04_isolation_storage" "APFS CoW Storage Deduplication & Mutation Isolation"

QUICK_MODE="${QUICK:-0}"
if [ "$QUICK_MODE" -eq 1 ]; then
    BENCH_FILES=2000
else
    BENCH_FILES="${FLASHWT_BENCH_FILES:-40000}"
fi

setup_isolated_fixture "iso-storage"

echo "Generating Scenario D tree with $BENCH_FILES files..."
mkdir -p "$FIXTURE_ORIGIN/node_modules"
generate_tree_d "$FIXTURE_ORIGIN/node_modules" "$BENCH_FILES"

cat << 'EOF' > "$FIXTURE_ORIGIN/.flashwtinclude"
node_modules/
EOF

cd "$FIXTURE_ORIGIN"
git add .flashwtinclude
git commit -qm "manifest for storage isolation"

test_start "volume_accounting" "Measure volume disk delta across 1, 3, and 5 concurrent worktrees"

read -r app_bytes alloc_bytes <<< "$(tree_disk_usage "$FIXTURE_ORIGIN/node_modules")"
v0_free=$(volume_free_bytes "$FIXTURE_DIR")

flashwt_json new flashwt-iso-1 --dir "$FIXTURE_DIR/flashwt-iso-1" >/dev/null
v1_free=$(volume_free_bytes "$FIXTURE_DIR")
delta_1=$(awk -v v0="$v0_free" -v v1="$v1_free" 'BEGIN { d = v0 - v1; printf "%.0f", (d > 0 ? d : 0) }')

flashwt_json new flashwt-iso-2 --dir "$FIXTURE_DIR/flashwt-iso-2" >/dev/null
flashwt_json new flashwt-iso-3 --dir "$FIXTURE_DIR/flashwt-iso-3" >/dev/null
v3_free=$(volume_free_bytes "$FIXTURE_DIR")
delta_3=$(awk -v v0="$v0_free" -v v3="$v3_free" 'BEGIN { d = v0 - v3; printf "%.0f", (d > 0 ? d : 0) }')

flashwt_json new flashwt-iso-4 --dir "$FIXTURE_DIR/flashwt-iso-4" >/dev/null
flashwt_json new flashwt-iso-5 --dir "$FIXTURE_DIR/flashwt-iso-5" >/dev/null
v5_free=$(volume_free_bytes "$FIXTURE_DIR")
delta_5=$(awk -v v0="$v0_free" -v v5="$v5_free" 'BEGIN { d = v0 - v5; printf "%.0f", (d > 0 ? d : 0) }')

logical_total_5=$(awk -v alloc="$alloc_bytes" 'BEGIN { printf "%.0f", alloc * 5 }')

read -r store_app store_alloc <<< "$(tree_disk_usage "$FIXTURE_STORE")"

dedup_ratio=$(python3 -c "
logical_unshared = $logical_total_5
physical_shared = max($delta_5, $store_alloc, $alloc_bytes, 1)
ratio = logical_unshared / physical_shared if physical_shared > 0 else 5.0
print(f'{min(ratio, 5.0):.2f}')
")

echo "  Physical storage consumed for 5 worktrees: $(awk -v d="$delta_5" 'BEGIN { printf "%.2f MB", d / 1048576 }') (Logical Unshared: $(awk -v l="$logical_total_5" 'BEGIN { printf "%.2f MB", l / 1048576 }'), Dedup: ${dedup_ratio}x)"

test_pass "volume_accounting" "{\"logical_5_flashwt_bytes\": $logical_total_5, \"physical_delta_bytes\": $delta_5, \"store_allocated_bytes\": $store_alloc, \"dedup_ratio\": $dedup_ratio}"

test_start "mutation_isolation" "Verify file edits, creations, and deletions in Worktree 1 do not alter Worktree 2 or Store"

target_rel="node_modules/pkg-00000/lib/mod-0.js"
orig_file="$FIXTURE_ORIGIN/$target_rel"
flashwt1_file="$FIXTURE_DIR/flashwt-iso-1/$target_rel"
flashwt2_file="$FIXTURE_DIR/flashwt-iso-2/$target_rel"

orig_content=$(cat "$orig_file")

echo "// MUTATED_IN_WT1" > "$flashwt1_file"

wt2_content=$(cat "$flashwt2_file")
if [ "$wt2_content" != "$orig_content" ]; then
    test_fail "mutation_isolation" "Worktree 2 file was corrupted by Worktree 1 modification"
fi

orig_current=$(cat "$orig_file")
if [ "$orig_current" != "$orig_content" ]; then
    test_fail "mutation_isolation" "Origin donor file was modified by Worktree 1 mutation"
fi

echo "new file content" > "$FIXTURE_DIR/flashwt-iso-1/node_modules/pkg-00000/lib/new_module.js"
if [ -e "$FIXTURE_DIR/flashwt-iso-2/node_modules/pkg-00000/lib/new_module.js" ]; then
    test_fail "mutation_isolation" "Newly created file leaked into Worktree 2"
fi

rm "$FIXTURE_DIR/flashwt-iso-1/node_modules/pkg-00000/lib/mod-1.js"
if [ ! -f "$FIXTURE_DIR/flashwt-iso-2/node_modules/pkg-00000/lib/mod-1.js" ]; then
    test_fail "mutation_isolation" "Deleted file disappeared from Worktree 2"
fi

if [ -d "$FIXTURE_STORE/objects" ]; then
    while read -r blob; do
        prefix=$(basename "$(dirname "$blob")")
        fname=$(basename "$blob")
        expected_hash="${prefix}${fname}"
        actual_hash=$(sha256_hash "$blob")
        if [ "$expected_hash" != "$actual_hash" ]; then
            test_fail "mutation_isolation" "Store blob corrupted: $blob"
        fi
    done < <(find "$FIXTURE_STORE/objects" -type f)
fi

test_pass "mutation_isolation" "{}"

test_start "triple_axis_fidelity" "Verify SHA256 byte parity, symlinks, and POSIX permissions between Worktree 2 and Origin"
assert_triple_axis "$FIXTURE_ORIGIN/node_modules" "$FIXTURE_DIR/flashwt-iso-2/node_modules" "Worktree 2 vs Origin fidelity failed"
test_pass "triple_axis_fidelity" "{}"

teardown_isolated_fixture "$FIXTURE_DIR"

suite_finish

