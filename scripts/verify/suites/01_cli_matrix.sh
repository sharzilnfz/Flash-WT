#!/usr/bin/env bash

set -euo pipefail

SUITE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VERIFY_DIR="$(cd "$SUITE_DIR/.." && pwd)"

. "$VERIFY_DIR/harness.sh"
. "$VERIFY_DIR/generators.sh"

suite_init "01_cli_matrix" "Exhaustive 11-Subcommand CLI Matrix"

setup_isolated_fixture "cli-matrix"

mkdir -p "$FIXTURE_ORIGIN/node_modules/pkg-demo/lib"
printf '// demo module\nmodule.exports = 42;\n' > "$FIXTURE_ORIGIN/node_modules/pkg-demo/lib/index.js"
printf '{"name":"pkg-demo","version":"1.0.0"}\n' > "$FIXTURE_ORIGIN/node_modules/pkg-demo/package.json"
mkdir -p "$FIXTURE_ORIGIN/node_modules/.bin"
printf '#!/bin/sh\necho "demo-bin running"\n' > "$FIXTURE_ORIGIN/node_modules/pkg-demo/bin.js"
chmod +x "$FIXTURE_ORIGIN/node_modules/pkg-demo/bin.js"
ln -sf "../pkg-demo/bin.js" "$FIXTURE_ORIGIN/node_modules/.bin/demo-bin"

test_start "init_basic" "Test flashwt init generates starter .flashwtinclude"
cd "$FIXTURE_ORIGIN"
rm -f .flashwtinclude
out=$(flashwt_json init)
assert_json_ok "$out" "flashwt init failed"
assert_file_exists "$FIXTURE_ORIGIN/.flashwtinclude" ".flashwtinclude was not created"
grep -q "node_modules/" "$FIXTURE_ORIGIN/.flashwtinclude" || {
    test_fail "init_basic" ".flashwtinclude missing node_modules pattern"
}
test_pass "init_basic" "{\"manifest_path\": \"$FIXTURE_ORIGIN/.flashwtinclude\"}"

test_start "init_force" "Test flashwt init refuses overwrite without --force, and succeeds with --force"
set +e
err_out=$(flashwt_json init 2>&1)
exit_code=$?
set -e
if [ "$exit_code" -eq 0 ] && echo "$err_out" | grep -qv "error"; then
    test_fail "init_force" "flashwt init should fail or notify when .flashwtinclude already exists without --force"
else
    force_out=$(flashwt_json init --force)
    assert_json_ok "$force_out" "flashwt init --force failed"
    test_pass "init_force" "{}"
fi

test_start "init_dir" "Test flashwt init --dir <subdir>"
mkdir -p "$FIXTURE_ORIGIN/nested/subproject"
sub_out=$(flashwt_json init --dir "$FIXTURE_ORIGIN/nested/subproject")
assert_json_ok "$sub_out" "flashwt init --dir failed"
assert_file_exists "$FIXTURE_ORIGIN/nested/subproject/.flashwtinclude" "nested .flashwtinclude missing"
test_pass "init_dir" "{}"

cd "$FIXTURE_ORIGIN"
git add .flashwtinclude
git commit -qm "add .flashwtinclude"

test_start "create_worktree" "Test flashwt create creates worktree with hydrated heavy files"
flashwt1_dir="$FIXTURE_DIR/flashwt-create-test"
create_out=$(flashwt_json create feat-create --dir "$flashwt1_dir")
assert_json_ok "$create_out" "flashwt create failed"
assert_file_exists "$flashwt1_dir/node_modules/pkg-demo/lib/index.js" "hydrated file missing"
assert_symlink "$flashwt1_dir/node_modules/.bin/demo-bin" "../pkg-demo/bin.js" "symlink missing or bad target"
    if ! git -C "$FIXTURE_ORIGIN" worktree list | grep -q "feat-create"; then
        test_fail "create_worktree" "worktree not registered in git"
    else
        test_pass "create_worktree" "{\"worktree_path\":\"$flashwt1_dir\"}"
    fi

test_start "new_worktree_options" "Test flashwt new alias with --base and --manifest"
flashwt2_dir="$FIXTURE_DIR/flashwt-new-test"
custom_manifest="$FIXTURE_ORIGIN/custom.manifest"
printf 'node_modules/\n' > "$custom_manifest"
new_out=$(flashwt_json new feat-new --base master --manifest "$custom_manifest" --dir "$flashwt2_dir")
assert_json_ok "$new_out" "flashwt new alias failed"
assert_file_exists "$flashwt2_dir/node_modules/pkg-demo/package.json" "new worktree heavy file missing"
test_pass "new_worktree_options" "{\"worktree_path\":\"$flashwt2_dir\"}"

test_start "hydrate_in_place" "Test flashwt hydrate in-place into existing directory without branch"
hydrate_dest="$FIXTURE_DIR/hydrated-dir"
mkdir -p "$hydrate_dest"
hydrate_out=$(flashwt_json hydrate "$hydrate_dest" --source "$FIXTURE_ORIGIN" --manifest "$FIXTURE_ORIGIN/.flashwtinclude")
assert_json_ok "$hydrate_out" "flashwt hydrate failed"
assert_file_exists "$hydrate_dest/node_modules/pkg-demo/lib/index.js" "in-place hydrated file missing"
test_pass "hydrate_in_place" "{\"destination\":\"$hydrate_dest\"}"

test_start "list_and_ls" "Test flashwt list and flashwt ls report JSON with worktrees, branches, savings"
list_out=$(flashwt_json list)
assert_json_ok "$list_out" "flashwt list failed"
python3 -c "
import json, sys
data = json.loads(sys.argv[1])['data']
assert 'worktrees' in data, 'worktrees array missing'
assert len(data['worktrees']) >= 2, f'expected >= 2 worktrees, got {len(data[\"worktrees\"])}'
assert 'total_disk_saved' in data, 'total_disk_saved missing'
" "$list_out"

ls_out=$(flashwt_json ls)
assert_json_ok "$ls_out" "flashwt ls alias failed"
test_pass "list_and_ls" "{}"

test_start "scratch_run" "Test flashwt scratch with --run auto-executes and tears down"
scratch_out=$(flashwt_json scratch --run "cat node_modules/pkg-demo/lib/index.js")
assert_json_ok "$scratch_out" "flashwt scratch --run failed"
python3 -c "
import json, sys
data = json.loads(sys.argv[1])['data']
assert data.get('executed', False) or data.get('cleaned_up', False), 'execution or cleanup flag missing'
" "$scratch_out"
test_pass "scratch_run" "{}"

test_start "isolate_lease" "Test flashwt isolate with --ttl creates persistent lease"
iso_dir="$FIXTURE_DIR/iso-sandbox"
iso_out=$(flashwt_json isolate iso-sandbox --ttl 30m --dir "$iso_dir")
assert_json_ok "$iso_out" "flashwt isolate failed"
assert_file_exists "$iso_dir/node_modules/pkg-demo/lib/index.js" "isolate worktree file missing"
test_pass "isolate_lease" "{\"iso_dir\":\"$iso_dir\"}"

test_start "remove_single" "Test flashwt remove cleans single worktree and releases references"
rem_out=$(flashwt_json remove feat-create --dir "$flashwt1_dir")
assert_json_ok "$rem_out" "flashwt remove failed"
assert_file_not_exists "$flashwt1_dir" "worktree directory still exists after remove"
test_pass "remove_single" "{}"

test_start "clean_force" "Test flashwt clean with --force cleans worktree"
clean_out=$(flashwt_json clean feat-new --dir "$flashwt2_dir" --force)
assert_json_ok "$clean_out" "flashwt clean --force failed"
assert_file_not_exists "$flashwt2_dir" "worktree directory still exists after clean"
test_pass "clean_force" "{}"

test_start "clean_all" "Test flashwt clean --all batch purges all secondary worktrees"
worktree_a="$FIXTURE_DIR/batch-a"
worktree_b="$FIXTURE_DIR/batch-b"
flashwt_json new batch-a --dir "$worktree_a" >/dev/null
flashwt_json new batch-b --dir "$worktree_b" >/dev/null
assert_file_exists "$worktree_a" "batch-a missing"
assert_file_exists "$worktree_b" "batch-b missing"

clean_all_out=$(flashwt_json clean --all --force)
assert_json_ok "$clean_all_out" "flashwt clean --all failed"
assert_file_not_exists "$worktree_a" "batch-a still exists after clean --all"
assert_file_not_exists "$worktree_b" "batch-b still exists after clean --all"
test_pass "clean_all" "{}"

flashwt_json remove iso-sandbox --dir "$iso_dir" >/dev/null 2>&1 || true

test_start "sweep_gc" "Test flashwt sweep --age 0s collects unreferenced store blobs"
sweep_out=$(flashwt_json sweep --age 0s)
assert_json_ok "$sweep_out" "flashwt sweep failed"
python3 -c "
import json, sys
data = json.loads(sys.argv[1])['data']
assert 'reclaimed' in data or 'leases_reclaimed' in data, 'sweep metrics missing'
" "$sweep_out"
test_pass "sweep_gc" "{}"

test_start "scrub_dry_run_and_repair" "Test flashwt scrub --dry-run vs repair"
scrub_dry=$(flashwt_json scrub --dry-run)
assert_json_ok "$scrub_dry" "flashwt scrub --dry-run failed"
assert_json_val "$scrub_dry" "data['dry_run']" "True"

scrub_full=$(flashwt_json scrub)
assert_json_ok "$scrub_full" "flashwt scrub failed"
assert_json_val "$scrub_full" "data['dry_run']" "False"
test_pass "scrub_dry_run_and_repair" "{}"

test_start "store_migrate" "Test flashwt store migrate --activate-mark-sweep"
migrate_out=$(flashwt_json store migrate --activate-mark-sweep)
assert_json_ok "$migrate_out" "flashwt store migrate failed"
assert_json_val "$migrate_out" "data['gc_mode']" "mark-sweep"

post_sweep=$(flashwt_json sweep --age 0s)
assert_json_ok "$post_sweep" "post-migration sweep failed"
assert_json_val "$post_sweep" "data['mode']" "mark-sweep"
test_pass "store_migrate" "{}"

test_start "demo_command" "Test flashwt demo runs self-benchmark and returns status: ok"
if [ "${QUICK:-0}" -eq 1 ]; then
    test_skip "demo_command" "skipped in quick mode (10,000 file fixture)"
else
    demo_tmp=$(mktemp -d "${TMPDIR:-/tmp}/flashwt-demo-test.XXXXXX")
    demo_out=$(FLASHWT_STORE="$demo_tmp/store" flashwt_json demo)
    rm -rf "$demo_tmp"
    assert_json_ok "$demo_out" "flashwt demo failed"
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

test_start "completions_all_shells" "Test flashwt completions for bash, zsh, fish, elvish, powershell"
for sh_type in bash zsh fish elvish powershell; do
    comp_out=$(flashwt completions "$sh_type")
    [ -n "$comp_out" ] || test_fail "completions_all_shells" "empty completions for $sh_type"
    echo "$comp_out" | grep -qE "(flashwt|subcommand|clean|hydrate|init)" || {
        test_fail "completions_all_shells" "completions for $sh_type missing expected flashwt tokens"
    }
done
test_pass "completions_all_shells" "{\"shells\": [\"bash\", \"zsh\", \"fish\", \"elvish\", \"powershell\"]}"

teardown_isolated_fixture "$FIXTURE_DIR"

suite_finish

