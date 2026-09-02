#!/usr/bin/env bash
# scripts/verify/suites/05_chaos_resilience.sh — Chaos, Concurrency, and Fault-Injection Verification.
#
# Verifies:
#  1. Concurrency: 5 parallel `wt new` commands against a single store with lock contention.
#  2. Bit-rot injection: intentionally tamper with a CAS blob byte; verify `wt scrub --dry-run`
#     detects corruption and `wt scrub` repairs/purges it.
#  3. Cryptographic validation: verify `WT_VERIFY=1` detects tampered blobs and bypasses corrupt cache.
#  4. Interrupted hydration recovery: simulate SIGKILL mid-staging; verify store self-heals and locks do not leak.

set -euo pipefail

SUITE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VERIFY_DIR="$(cd "$SUITE_DIR/.." && pwd)"

# shellcheck disable=SC1091
. "$VERIFY_DIR/harness.sh"
# shellcheck disable=SC1091
. "$VERIFY_DIR/generators.sh"

suite_init "05_chaos_resilience" "Concurrency, Bit-Rot, Crypto Verify & Crash Resilience"

setup_isolated_fixture "chaos"

echo "Generating test tree for chaos verification..."
mkdir -p "$FIXTURE_ORIGIN/node_modules"
generate_tree_d "$FIXTURE_ORIGIN/node_modules" 2000

cat << 'EOF' > "$FIXTURE_ORIGIN/.wtinclude"
node_modules/
EOF

cd "$FIXTURE_ORIGIN"
git add .wtinclude
git commit -qm "manifest for chaos tests"

# -----------------------------------------------------------------------------
# 1. Concurrency (Lock Contention)
# -----------------------------------------------------------------------------
test_start "concurrency_5x" "Execute 5 parallel wt new commands concurrently against single store"

pids=()
for i in 1 2 3 4 5; do
    (
        out=$(wt_json new "conc-$i" --dir "$FIXTURE_DIR/conc-$i" 2>"$FIXTURE_DIR/conc-$i.err")
        echo "$out" > "$FIXTURE_DIR/conc-$i.json"
    ) &
    pids+=($!)
done

# Wait for all background workers
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

# Clean up concurrent worktrees
wt_json clean --all --force >/dev/null

test_pass "concurrency_5x" "{\"parallel_workers\": 5, \"failed\": 0}"

# -----------------------------------------------------------------------------
# 2. Bit-Rot Injection & Scrub Repair
# -----------------------------------------------------------------------------
test_start "bit_rot_scrub" "Inject byte tampering into a CAS blob; verify scrub detects and purges corruption"

# Hydrate a donor worktree using CAS blob store
WT_SNAPSHOTS=0 wt_json new donor-scrub --dir "$FIXTURE_DIR/donor-scrub" >/dev/null

# Select a blob to corrupt
read -r target_blob < <(find "$FIXTURE_STORE/objects" -type f 2>/dev/null)
[ -n "$target_blob" ] || test_fail "bit_rot_scrub" "no blobs found in store"

# Read original byte and invert it (CAS blobs are read-only by default)
chmod u+w "$target_blob"
printf '\xFF\xFE\xFD' | dd of="$target_blob" bs=1 seek=0 count=3 conv=notrunc 2>/dev/null

# 1. Run scrub --dry-run
dry_out=$(wt_json scrub --dry-run)
assert_json_ok "$dry_out" "wt scrub --dry-run failed"
corrupt_count=$(python3 -c "
import json, sys
data = json.loads(sys.argv[1])['data']
print(len(data.get('corrupt', [])))
" "$dry_out")

[ "$corrupt_count" -gt 0 ] || test_fail "bit_rot_scrub" "scrub --dry-run failed to detect corrupted blob"

# 2. Run active scrub to repair/purge
repair_out=$(wt_json scrub)
assert_json_ok "$repair_out" "wt scrub repair failed"
deleted_count=$(python3 -c "
import json, sys
data = json.loads(sys.argv[1])['data']
print(data.get('deleted', 0))
" "$repair_out")

[ "$deleted_count" -gt 0 ] || test_fail "bit_rot_scrub" "scrub repair failed to delete corrupted blob"

wt_json clean donor-scrub --dir "$FIXTURE_DIR/donor-scrub" --force >/dev/null

test_pass "bit_rot_scrub" "{\"detected_corrupt\": $corrupt_count, \"purged_blobs\": $deleted_count}"

# -----------------------------------------------------------------------------
# 3. Cryptographic Validation (WT_VERIFY=1)
# -----------------------------------------------------------------------------
test_start "crypto_verify" "Verify WT_VERIFY=1 detects tampered blobs and bypasses corrupted cache"

WT_SNAPSHOTS=0 wt_json new donor-crypto --dir "$FIXTURE_DIR/donor-crypto" >/dev/null

# Corrupt another blob in the store
read -r target_blob < <(find "$FIXTURE_STORE/objects" -type f 2>/dev/null)
chmod u+w "$target_blob"
printf '\xDE\xAD\xBE\xEF' | dd of="$target_blob" bs=1 seek=0 count=4 conv=notrunc 2>/dev/null

# Hydrate with WT_VERIFY=1
set +e
verify_out=$(WT_SNAPSHOTS=0 WT_VERIFY=1 wt_json new crypto-wt --dir "$FIXTURE_DIR/crypto-wt" 2>&1)
exit_code=$?
set -e

# WT_VERIFY=1 will either re-hash from origin and succeed with clean content, or fail explicitly on bad store blob
if [ "$exit_code" -eq 0 ]; then
    assert_json_ok "$verify_out" "WT_VERIFY=1 create produced invalid JSON"
    # Ensure hydrated file in worktree matches origin (not the corrupt blob!)
    assert_triple_axis "$FIXTURE_ORIGIN/node_modules" "$FIXTURE_DIR/crypto-wt/node_modules" "crypto verify tree mismatch"
    test_pass "crypto_verify" "{\"action\": \"re-ingested_and_verified\"}"
else
    # Explicit rejection of corrupted blob
    echo "$verify_out" | grep -qE "(corrupt|hash|mismatch|error)" || {
        test_fail "crypto_verify" "expected corruption detection message"
    }
    test_pass "crypto_verify" "{\"action\": \"rejected_corrupted_blob\"}"
fi

wt_json clean donor-crypto --dir "$FIXTURE_DIR/donor-crypto" --force >/dev/null 2>&1 || true
wt_json clean crypto-wt --dir "$FIXTURE_DIR/crypto-wt" --force >/dev/null 2>&1 || true
# Purge the tampered blob so store is clean for subsequent tests
wt_json scrub >/dev/null 2>&1 || true

# -----------------------------------------------------------------------------
# 4. Interrupted Hydration Recovery (SIGKILL Mid-Staging)
# -----------------------------------------------------------------------------
test_start "sigkill_recovery" "Simulate SIGKILL during hydration; verify store self-heals and locks do not leak"

# Launch wt new in background
(
    wt_json new abort-wt --dir "$FIXTURE_DIR/abort-wt" >/dev/null 2>&1
) &
kill_pid=$!

# Brief delay and SIGKILL
sleep 0.015
set +e
kill -9 "$kill_pid" 2>/dev/null
wait "$kill_pid" 2>/dev/null
set -e

# Prune git worktree remnants from killed process
git -C "$FIXTURE_ORIGIN" worktree prune 2>/dev/null || true
git -C "$FIXTURE_ORIGIN" branch -D abort-wt 2>/dev/null || true
rm -rf "$FIXTURE_DIR/abort-wt" 2>/dev/null || true

# Verify that lock is released and store is operational
t0=$(now)
rec_out=$(wt_json new recovered-wt --dir "$FIXTURE_DIR/recovered-wt")
t1=$(now)
rec_ms=$(elapsed_ms "$t0" "$t1")

assert_json_ok "$rec_out" "Recovery worktree creation failed after crash"
assert_file_exists "$FIXTURE_DIR/recovered-wt/node_modules/.bin" "recovered files missing"
assert_triple_axis "$FIXTURE_ORIGIN/node_modules" "$FIXTURE_DIR/recovered-wt/node_modules" "recovered tree parity check failed"

# Sweep to clean any dangling items
sweep_post=$(wt_json sweep --age 0s)
assert_json_ok "$sweep_post" "post-crash sweep failed"

wt_json clean recovered-wt --dir "$FIXTURE_DIR/recovered-wt" --force >/dev/null

test_pass "sigkill_recovery" "{\"recovery_ms\": $rec_ms}"

# Teardown
teardown_isolated_fixture "$FIXTURE_DIR"

suite_finish
