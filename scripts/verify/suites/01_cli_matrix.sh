#!/usr/bin/env bash
# scripts/verify/suites/01_cli_matrix.sh — Exhaustive CLI Subcommand Matrix Verification.
#
# Covers all 11 subcommands:
#  1. wt init (create, --force, --dir)
#  2. wt new / wt create (worktree creation, --base, --manifest, --dir)
#  3. wt hydrate (in-place hydration without creating a branch)
#  4. wt list / wt ls (JSON output, branch tracking, disk savings)
#  5. wt scratch / wt isolate (ephemeral execution, --run, --ttl, lease registration)
#  6. wt clean / wt remove (single removal, clean --all batch purge, --force)
#  7. wt sweep (--age 0s mark-sweep GC, unreferenced blob reclamation)
#  8. wt scrub (--dry-run vs repair)
#  9. wt store migrate (--activate-mark-sweep, verify store status)
# 10. wt demo (zero exit code, JSON envelope, speedup, mutation isolation)
# 11. wt completions (bash, zsh, fish, elvish, powershell)

set -euo pipefail

SUITE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VERIFY_DIR="$(cd "$SUITE_DIR/.." && pwd)"

# shellcheck disable=SC1091
. "$VERIFY_DIR/harness.sh"
# shellcheck disable=SC1091
. "$VERIFY_DIR/generators.sh"

suite_init "01_cli_matrix" "Exhaustive 11-Subcommand CLI Matrix"

setup_isolated_fixture "cli-matrix"

# Populate sample heavy content in origin
mkdir -p "$FIXTURE_ORIGIN/node_modules/pkg-demo/lib"
printf '// demo module\nmodule.exports = 42;\n' > "$FIXTURE_ORIGIN/node_modules/pkg-demo/lib/index.js"
printf '{"name":"pkg-demo","version":"1.0.0"}\n' > "$FIXTURE_ORIGIN/node_modules/pkg-demo/package.json"
mkdir -p "$FIXTURE_ORIGIN/node_modules/.bin"
printf '#!/bin/sh\necho "demo-bin running"\n' > "$FIXTURE_ORIGIN/node_modules/pkg-demo/bin.js"
chmod +x "$FIXTURE_ORIGIN/node_modules/pkg-demo/bin.js"
ln -sf "../pkg-demo/bin.js" "$FIXTURE_ORIGIN/node_modules/.bin/demo-bin"

# -----------------------------------------------------------------------------
# 1. wt init
# -----------------------------------------------------------------------------
test_start "init_basic" "Test wt init generates starter .wtinclude"
cd "$FIXTURE_ORIGIN"
rm -f .wtinclude
out=$(wt_json init)
assert_json_ok "$out" "wt init failed"
assert_file_exists "$FIXTURE_ORIGIN/.wtinclude" ".wtinclude was not created"
grep -q "node_modules/" "$FIXTURE_ORIGIN/.wtinclude" || {
    test_fail "init_basic" ".wtinclude missing node_modules pattern"
}
test_pass "init_basic" "{\"manifest_path\": \"$FIXTURE_ORIGIN/.wtinclude\"}"

test_start "init_force" "Test wt init refuses overwrite without --force, and succeeds with --force"
set +e
err_out=$(wt_json init 2>&1)
exit_code=$?
set -e
if [ "$exit_code" -eq 0 ] && echo "$err_out" | grep -qv "error"; then
    test_fail "init_force" "wt init should fail or notify when .wtinclude already exists without --force"
else
    # Now with --force
    force_out=$(wt_json init --force)
    assert_json_ok "$force_out" "wt init --force failed"
    test_pass "init_force" "{}"
fi

test_start "init_dir" "Test wt init --dir <subdir>"
mkdir -p "$FIXTURE_ORIGIN/nested/subproject"
sub_out=$(wt_json init --dir "$FIXTURE_ORIGIN/nested/subproject")
assert_json_ok "$sub_out" "wt init --dir failed"
assert_file_exists "$FIXTURE_ORIGIN/nested/subproject/.wtinclude" "nested .wtinclude missing"
test_pass "init_dir" "{}"

# Commit the .wtinclude so git has it
cd "$FIXTURE_ORIGIN"
git add .wtinclude
git commit -qm "add .wtinclude"

# -----------------------------------------------------------------------------
# 2. wt create & wt new
# -----------------------------------------------------------------------------
test_start "create_worktree" "Test wt create creates worktree with hydrated heavy files"
wt1_dir="$FIXTURE_DIR/wt-create-test"
create_out=$(wt_json create feat-create --dir "$wt1_dir")
assert_json_ok "$create_out" "wt create failed"
assert_file_exists "$wt1_dir/node_modules/pkg-demo/lib/index.js" "hydrated file missing"
assert_symlink "$wt1_dir/node_modules/.bin/demo-bin" "../pkg-demo/bin.js" "symlink missing or bad target"
    if ! git -C "$FIXTURE_ORIGIN" worktree list | grep -q "feat-create"; then
        test_fail "create_worktree" "worktree not registered in git"
    else
        test_pass "create_worktree" "{\"worktree_path\":\"$wt1_dir\"}"
    fi

test_start "new_worktree_options" "Test wt new alias with --base and --manifest"
wt2_dir="$FIXTURE_DIR/wt-new-test"
custom_manifest="$FIXTURE_ORIGIN/custom.manifest"
printf 'node_modules/\n' > "$custom_manifest"
new_out=$(wt_json new feat-new --base master --manifest "$custom_manifest" --dir "$wt2_dir")
assert_json_ok "$new_out" "wt new alias failed"
assert_file_exists "$wt2_dir/node_modules/pkg-demo/package.json" "new worktree heavy file missing"
test_pass "new_worktree_options" "{\"worktree_path\":\"$wt2_dir\"}"

# -----------------------------------------------------------------------------
# 3. wt hydrate
# -----------------------------------------------------------------------------
test_start "hydrate_in_place" "Test wt hydrate in-place into existing directory without branch"
hydrate_dest="$FIXTURE_DIR/hydrated-dir"
mkdir -p "$hydrate_dest"
hydrate_out=$(wt_json hydrate "$hydrate_dest" --source "$FIXTURE_ORIGIN" --manifest "$FIXTURE_ORIGIN/.wtinclude")
assert_json_ok "$hydrate_out" "wt hydrate failed"
assert_file_exists "$hydrate_dest/node_modules/pkg-demo/lib/index.js" "in-place hydrated file missing"
test_pass "hydrate_in_place" "{\"destination\":\"$hydrate_dest\"}"

# -----------------------------------------------------------------------------
# 4. wt list & wt ls
# -----------------------------------------------------------------------------
test_start "list_and_ls" "Test wt list and wt ls report JSON with worktrees, branches, savings"
list_out=$(wt_json list)
assert_json_ok "$list_out" "wt list failed"
python3 -c "
import json, sys
data = json.loads(sys.argv[1])['data']
assert 'worktrees' in data, 'worktrees array missing'
assert len(data['worktrees']) >= 2, f'expected >= 2 worktrees, got {len(data[\"worktrees\"])}'
assert 'total_disk_saved' in data, 'total_disk_saved missing'
" "$list_out"

ls_out=$(wt_json ls)
assert_json_ok "$ls_out" "wt ls alias failed"
test_pass "list_and_ls" "{}"

# -----------------------------------------------------------------------------
# 5. wt scratch & wt isolate
# -----------------------------------------------------------------------------
test_start "scratch_run" "Test wt scratch with --run auto-executes and tears down"
scratch_out=$(wt_json scratch --run "cat node_modules/pkg-demo/lib/index.js")
assert_json_ok "$scratch_out" "wt scratch --run failed"
python3 -c "
import json, sys
data = json.loads(sys.argv[1])['data']
assert data.get('executed', False) or data.get('cleaned_up', False), 'execution or cleanup flag missing'
" "$scratch_out"
test_pass "scratch_run" "{}"

test_start "isolate_lease" "Test wt isolate with --ttl creates persistent lease"
iso_dir="$FIXTURE_DIR/iso-sandbox"
iso_out=$(wt_json isolate iso-sandbox --ttl 30m --dir "$iso_dir")
assert_json_ok "$iso_out" "wt isolate failed"
assert_file_exists "$iso_dir/node_modules/pkg-demo/lib/index.js" "isolate worktree file missing"
test_pass "isolate_lease" "{\"iso_dir\":\"$iso_dir\"}"

# -----------------------------------------------------------------------------
# 6. wt clean & wt remove
# -----------------------------------------------------------------------------
test_start "remove_single" "Test wt remove cleans single worktree and releases references"
rem_out=$(wt_json remove feat-create --dir "$wt1_dir")
assert_json_ok "$rem_out" "wt remove failed"
assert_file_not_exists "$wt1_dir" "worktree directory still exists after remove"
test_pass "remove_single" "{}"

test_start "clean_force" "Test wt clean with --force cleans worktree"
clean_out=$(wt_json clean feat-new --dir "$wt2_dir" --force)
assert_json_ok "$clean_out" "wt clean --force failed"
assert_file_not_exists "$wt2_dir" "worktree directory still exists after clean"
test_pass "clean_force" "{}"

test_start "clean_all" "Test wt clean --all batch purges all secondary worktrees"
# Create two dummy worktrees
wt_a="$FIXTURE_DIR/batch-a"
wt_b="$FIXTURE_DIR/batch-b"
wt_json new batch-a --dir "$wt_a" >/dev/null
wt_json new batch-b --dir "$wt_b" >/dev/null
assert_file_exists "$wt_a" "batch-a missing"
assert_file_exists "$wt_b" "batch-b missing"

clean_all_out=$(wt_json clean --all --force)
assert_json_ok "$clean_all_out" "wt clean --all failed"
assert_file_not_exists "$wt_a" "batch-a still exists after clean --all"
assert_file_not_exists "$wt_b" "batch-b still exists after clean --all"
test_pass "clean_all" "{}"

# Clean up isolate lease as well
wt_json remove iso-sandbox --dir "$iso_dir" >/dev/null 2>&1 || true

# -----------------------------------------------------------------------------
# 7. wt sweep
# -----------------------------------------------------------------------------
test_start "sweep_gc" "Test wt sweep --age 0s collects unreferenced store blobs"
sweep_out=$(wt_json sweep --age 0s)
assert_json_ok "$sweep_out" "wt sweep failed"
python3 -c "
import json, sys
data = json.loads(sys.argv[1])['data']
assert 'reclaimed' in data or 'leases_reclaimed' in data, 'sweep metrics missing'
" "$sweep_out"
test_pass "sweep_gc" "{}"

# -----------------------------------------------------------------------------
# 8. wt scrub
# -----------------------------------------------------------------------------
test_start "scrub_dry_run_and_repair" "Test wt scrub --dry-run vs repair"
scrub_dry=$(wt_json scrub --dry-run)
assert_json_ok "$scrub_dry" "wt scrub --dry-run failed"
assert_json_val "$scrub_dry" "data['dry_run']" "True"

scrub_full=$(wt_json scrub)
assert_json_ok "$scrub_full" "wt scrub failed"
assert_json_val "$scrub_full" "data['dry_run']" "False"
test_pass "scrub_dry_run_and_repair" "{}"

# -----------------------------------------------------------------------------
# 9. wt store migrate
# -----------------------------------------------------------------------------
test_start "store_migrate" "Test wt store migrate --activate-mark-sweep"
migrate_out=$(wt_json store migrate --activate-mark-sweep)
assert_json_ok "$migrate_out" "wt store migrate failed"
assert_json_val "$migrate_out" "data['gc_mode']" "mark-sweep"

# Verify subsequent sweep reflects mark-sweep mode
post_sweep=$(wt_json sweep --age 0s)
assert_json_ok "$post_sweep" "post-migration sweep failed"
assert_json_val "$post_sweep" "data['mode']" "mark-sweep"
test_pass "store_migrate" "{}"

# -----------------------------------------------------------------------------
# 10. wt demo
test_start "demo_command" "Test wt demo runs self-benchmark and returns status: ok"
if [ "${QUICK:-0}" -eq 1 ]; then
    test_skip "demo_command" "skipped in quick mode (10,000 file fixture)"
else
    demo_tmp=$(mktemp -d "${TMPDIR:-/tmp}/wt-demo-test.XXXXXX")
    demo_out=$(WT_STORE="$demo_tmp/store" wt_json demo)
    rm -rf "$demo_tmp"
    assert_json_ok "$demo_out" "wt demo failed"
    python3 -c "
import json, sys
data = json.loads(sys.argv[1])['data']
assert data['isolation_verified'] is True, 'isolation_verified must be true'
assert data['cleaned_up'] is True, 'cleaned_up must be true'
assert data['files_count'] > 0, 'files_count must be > 0'
assert data['speedup_ratio'] > 0, 'speedup_ratio must be > 0'
" "$demo_out"
    test_pass "demo_command" "{}"
fi

# -----------------------------------------------------------------------------
# 11. wt completions
# -----------------------------------------------------------------------------
test_start "completions_all_shells" "Test wt completions for bash, zsh, fish, elvish, powershell"
for sh_type in bash zsh fish elvish powershell; do
    comp_out=$(wt completions "$sh_type")
    [ -n "$comp_out" ] || test_fail "completions_all_shells" "empty completions for $sh_type"
    echo "$comp_out" | grep -qE "(wt|subcommand|clean|hydrate|init)" || {
        test_fail "completions_all_shells" "completions for $sh_type missing expected wt tokens"
    }
done
test_pass "completions_all_shells" "{\"shells\": [\"bash\", \"zsh\", \"fish\", \"elvish\", \"powershell\"]}"

# Teardown fixture
teardown_isolated_fixture "$FIXTURE_DIR"

suite_finish
