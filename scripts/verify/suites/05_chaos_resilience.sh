#!/usr/bin/env bash

set -euo pipefail

SUITE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VERIFY_DIR="$(cd "$SUITE_DIR/.." && pwd)"

. "$VERIFY_DIR/harness.sh"
. "$VERIFY_DIR/generators.sh"

suite_init "05_chaos_resilience" "Concurrency, Bit-Rot, Crypto Verify & Crash Resilience"

setup_isolated_fixture "chaos"

echo "Generating test tree for chaos verification..."
mkdir -p "$FIXTURE_ORIGIN/node_modules"
generate_tree_d "$FIXTURE_ORIGIN/node_modules" 2000

cat << 'EOF' > "$FIXTURE_ORIGIN/.flashwtinclude"
node_modules/
EOF

cd "$FIXTURE_ORIGIN"
git add .flashwtinclude
git commit -qm "manifest for chaos tests"

test_start "concurrency_5x" "Execute 5 parallel flashwt new commands concurrently against single store"

pids=()
for i in 1 2 3 4 5; do
    (
        out=$(flashwt_json new "conc-$i" --dir "$FIXTURE_DIR/conc-$i" 2>"$FIXTURE_DIR/conc-$i.err")
        echo "$out" > "$FIXTURE_DIR/conc-$i.json"
    ) &
    pids+=($!)
done

failed_workers=0
for pid in "${pids[@]}"; do
    if ! wait "$pid"; then
        failed_workers=$((failed_workers + 1))
    fi
done

[ "$failed_workers" -eq 0 ] || test_fail "concurrency_5x" "$failed_workers concurrent workers failed"

for i in 1 2 3 4 5; do
    assert_file_exists "$FIXTURE_DIR/conc-$i.json" "conc-$i output missing"
    conc_json=$(cat "$FIXTURE_DIR/conc-$i.json")
    assert_json_ok "$conc_json" "conc-$i reported non-ok status"
    assert_file_exists "$FIXTURE_DIR/conc-$i/node_modules/.bin" "conc-$i files missing"
done

flashwt_json clean --all --force >/dev/null

test_pass "concurrency_5x" "{\"parallel_workers\": 5, \"failed\": 0}"

test_start "bit_rot_scrub" "Inject byte tampering into a CAS blob; verify scrub detects and purges corruption"

FLASHWT_SNAPSHOTS=0 flashwt_json new donor-scrub --dir "$FIXTURE_DIR/donor-scrub" >/dev/null

read -r target_blob < <(find "$FIXTURE_STORE/objects" -type f 2>/dev/null)
[ -n "$target_blob" ] || test_fail "bit_rot_scrub" "no blobs found in store"

chmod u+w "$target_blob"
printf '\xFF\xFE\xFD' | dd of="$target_blob" bs=1 seek=0 count=3 conv=notrunc 2>/dev/null

dry_out=$(flashwt_json scrub --dry-run)
assert_json_ok "$dry_out" "flashwt scrub --dry-run failed"
corrupt_count=$(python3 -c "
import json, sys
data = json.loads(sys.argv[1])['data']
print(len(data.get('corrupt', [])))
" "$dry_out")

[ "$corrupt_count" -gt 0 ] || test_fail "bit_rot_scrub" "scrub --dry-run failed to detect corrupted blob"

repair_out=$(flashwt_json scrub)
assert_json_ok "$repair_out" "flashwt scrub repair failed"
deleted_count=$(python3 -c "
import json, sys
data = json.loads(sys.argv[1])['data']
print(data.get('deleted', 0))
" "$repair_out")

[ "$deleted_count" -gt 0 ] || test_fail "bit_rot_scrub" "scrub repair failed to delete corrupted blob"

flashwt_json clean donor-scrub --dir "$FIXTURE_DIR/donor-scrub" --force >/dev/null

test_pass "bit_rot_scrub" "{\"detected_corrupt\": $corrupt_count, \"purged_blobs\": $deleted_count}"

test_start "crypto_verify" "Verify FLASHWT_VERIFY=1 detects tampered blobs and bypasses corrupted cache"

FLASHWT_SNAPSHOTS=0 flashwt_json new donor-crypto --dir "$FIXTURE_DIR/donor-crypto" >/dev/null

read -r target_blob < <(find "$FIXTURE_STORE/objects" -type f 2>/dev/null)
chmod u+w "$target_blob"
printf '\xDE\xAD\xBE\xEF' | dd of="$target_blob" bs=1 seek=0 count=4 conv=notrunc 2>/dev/null

set +e
verify_out=$(FLASHWT_SNAPSHOTS=0 FLASHWT_VERIFY=1 flashwt_json new crypto-worktree --dir "$FIXTURE_DIR/crypto-worktree" 2>&1)
exit_code=$?
set -e

if [ "$exit_code" -eq 0 ]; then
    assert_json_ok "$verify_out" "FLASHWT_VERIFY=1 create produced invalid JSON"
    assert_triple_axis "$FIXTURE_ORIGIN/node_modules" "$FIXTURE_DIR/crypto-worktree/node_modules" "crypto verify tree mismatch"
    test_pass "crypto_verify" "{\"action\": \"re-ingested_and_verified\"}"
else
    echo "$verify_out" | grep -qE "(corrupt|hash|mismatch|error)" || {
        test_fail "crypto_verify" "expected corruption detection message"
    }
    test_pass "crypto_verify" "{\"action\": \"rejected_corrupted_blob\"}"
fi

flashwt_json clean donor-crypto --dir "$FIXTURE_DIR/donor-crypto" --force >/dev/null 2>&1 || true
flashwt_json clean crypto-worktree --dir "$FIXTURE_DIR/crypto-worktree" --force >/dev/null 2>&1 || true
flashwt_json scrub >/dev/null 2>&1 || true

test_start "sigkill_recovery" "Simulate SIGKILL during hydration; verify store self-heals and locks do not leak"

(
    flashwt_json new abort-worktree --dir "$FIXTURE_DIR/abort-worktree" >/dev/null 2>&1
) &
kill_pid=$!

sleep 0.015
set +e
kill -9 "$kill_pid" 2>/dev/null
wait "$kill_pid" 2>/dev/null
set -e

git -C "$FIXTURE_ORIGIN" worktree prune 2>/dev/null || true
git -C "$FIXTURE_ORIGIN" branch -D abort-worktree 2>/dev/null || true
rm -rf "$FIXTURE_DIR/abort-worktree" 2>/dev/null || true

t0=$(now)
rec_out=$(flashwt_json new recovered-worktree --dir "$FIXTURE_DIR/recovered-worktree")
t1=$(now)
rec_ms=$(elapsed_ms "$t0" "$t1")

assert_json_ok "$rec_out" "Recovery worktree creation failed after crash"
assert_file_exists "$FIXTURE_DIR/recovered-worktree/node_modules/.bin" "recovered files missing"
assert_triple_axis "$FIXTURE_ORIGIN/node_modules" "$FIXTURE_DIR/recovered-worktree/node_modules" "recovered tree parity check failed"

sweep_post=$(flashwt_json sweep --age 0s)
assert_json_ok "$sweep_post" "post-crash sweep failed"

flashwt_json clean recovered-worktree --dir "$FIXTURE_DIR/recovered-worktree" --force >/dev/null

test_pass "sigkill_recovery" "{\"recovery_ms\": $rec_ms}"

teardown_isolated_fixture "$FIXTURE_DIR"

suite_finish

