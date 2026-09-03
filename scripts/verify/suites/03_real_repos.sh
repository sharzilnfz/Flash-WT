#!/usr/bin/env bash

set -euo pipefail

SUITE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VERIFY_DIR="$(cd "$SUITE_DIR/.." && pwd)"

. "$VERIFY_DIR/harness.sh"
. "$VERIFY_DIR/generators.sh"
. "$VERIFY_DIR/fetch_repos.sh"

suite_init "03_real_repos" "Real-World Ecosystem Integration & Fidelity"

QUICK_MODE="${QUICK:-0}"
FILE_COUNT=2000
[ "$QUICK_MODE" -eq 1 ] && FILE_COUNT=800

BASE_TMP="$(mktemp -d "${TMPDIR:-/tmp}/flashwt-real-repos.XXXXXX")"
export FLASHWT_STORE="$BASE_TMP/store"
mkdir -p "$FLASHWT_STORE"

test_start "real_node_repo" "Hydrate real Node.js repo with deeply nested node_modules and .bin symlinks"
node_origin="$BASE_TMP/node-origin"
node_flashwt="$BASE_TMP/node-worktree"

prepare_node_repo "$node_origin" "$FILE_COUNT"
cd "$node_origin"

t0=$(now)
out_node=$(flashwt_json new test-node --dir "$node_flashwt")
t1=$(now)
node_ms=$(elapsed_ms "$t0" "$t1")

assert_json_ok "$out_node" "Node.js worktree creation failed"
assert_file_exists "$node_flashwt/node_modules" "node_modules missing in worktree"
assert_triple_axis "$node_origin/node_modules" "$node_flashwt/node_modules" "Node.js triple-axis check failed"

read -r first_bin < <(find "$node_flashwt/node_modules/.bin" -type l 2>/dev/null)
[ -n "$first_bin" ] && [ -x "$first_bin" ] || test_fail "real_node_repo" ".bin symlink not executable"

flashwt_json clean test-node --dir "$node_flashwt" --force >/dev/null
test_pass "real_node_repo" "{\"duration_ms\": $node_ms, \"files\": $FILE_COUNT}"

test_start "real_rust_repo" "Hydrate real Rust workspace with target/debug, rlibs, and build outputs"
rust_origin="$BASE_TMP/rust-origin"
rust_flashwt="$BASE_TMP/rust-worktree"

prepare_rust_repo "$rust_origin" "$FILE_COUNT"
cd "$rust_origin"

t0=$(now)
out_rust=$(flashwt_json new test-rust --dir "$rust_flashwt")
t1=$(now)
rust_ms=$(elapsed_ms "$t0" "$t1")

assert_json_ok "$out_rust" "Rust worktree creation failed"
assert_file_exists "$rust_flashwt/target/debug" "target/debug missing in worktree"
assert_triple_axis "$rust_origin/target/debug/deps" "$rust_flashwt/target/debug/deps" "Rust deps triple-axis check failed"
assert_triple_axis "$rust_origin/target/debug/.fingerprint" "$rust_flashwt/target/debug/.fingerprint" "Rust fingerprint triple-axis check failed"
assert_file_not_exists "$rust_flashwt/target/debug/incremental" "Rust volatile incremental cache must be excluded"

if [ -x "$rust_flashwt/target/debug/app" ]; then
    app_output=$("$rust_flashwt/target/debug/app")
    echo "$app_output" | grep -q "synthetic rust binary" || test_fail "real_rust_repo" "Rust binary execution output incorrect"
fi

flashwt_json clean test-rust --dir "$rust_flashwt" --force >/dev/null
test_pass "real_rust_repo" "{\"duration_ms\": $rust_ms, \"files\": $FILE_COUNT}"

test_start "real_python_repo" "Hydrate real Python project with .venv, site-packages, and pycache"
py_origin="$BASE_TMP/py-origin"
py_flashwt="$BASE_TMP/py-worktree"

prepare_python_repo "$py_origin" "$FILE_COUNT"
cd "$py_origin"

t0=$(now)
out_py=$(flashwt_json new test-python --dir "$py_flashwt")
t1=$(now)
py_ms=$(elapsed_ms "$t0" "$t1")

assert_json_ok "$out_py" "Python worktree creation failed"
assert_file_exists "$py_flashwt/.venv/lib" ".venv/lib missing in worktree"
assert_triple_axis "$py_origin/.venv" "$py_flashwt/.venv" "Python venv triple-axis check failed"

assert_symlink "$py_flashwt/.venv/bin/python" "python3" "Python binary symlink invalid"

flashwt_json clean test-python --dir "$py_flashwt" --force >/dev/null
test_pass "real_python_repo" "{\"duration_ms\": $py_ms, \"files\": $FILE_COUNT}"

test_start "real_monorepo" "Hydrate multi-ecosystem monorepo (Node + Rust + Python) under single .flashwtinclude"
mono_origin="$BASE_TMP/mono-origin"
mono_flashwt="$BASE_TMP/mono-worktree"

prepare_monorepo "$mono_origin"
cd "$mono_origin"

t0=$(now)
out_mono=$(flashwt_json new test-mono --dir "$mono_flashwt")
t1=$(now)
mono_ms=$(elapsed_ms "$t0" "$t1")

assert_json_ok "$out_mono" "Monorepo worktree creation failed"
assert_triple_axis "$mono_origin/apps/web/node_modules" "$mono_flashwt/apps/web/node_modules" "Monorepo web app triple-axis failed"
assert_triple_axis "$mono_origin/crates/core/target/debug/deps" "$mono_flashwt/crates/core/target/debug/deps" "Monorepo rust target triple-axis failed"
assert_triple_axis "$mono_origin/services/api/.venv" "$mono_flashwt/services/api/.venv" "Monorepo python venv triple-axis failed"

flashwt_json clean test-mono --dir "$mono_flashwt" --force >/dev/null
test_pass "real_monorepo" "{\"duration_ms\": $mono_ms}"

rm -rf "$BASE_TMP" 2>/dev/null || true

suite_finish

