#!/usr/bin/env bash
# scripts/verify/suites/03_real_repos.sh — Real-World Ecosystem Repository Verification.
#
# Proves hydration correctness and parity across 4 real ecosystems:
#  1. Real Node.js repo with heavy node_modules (symlinks, .bin, deep packages)
#  2. Real Rust workspace with target/debug (rlibs, rmetas, incremental, binaries)
#  3. Real Python project with .venv (site-packages, __pycache__, python symlinks)
#  4. Multi-ecosystem monorepo (.wtinclude with web, rust, and python)
#
# Validates triple-axis fidelity (SHA256, POSIX modes, symlink targets) and zero corruption.

set -euo pipefail

SUITE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VERIFY_DIR="$(cd "$SUITE_DIR/.." && pwd)"

# shellcheck disable=SC1091
. "$VERIFY_DIR/harness.sh"
# shellcheck disable=SC1091
. "$VERIFY_DIR/generators.sh"
# shellcheck disable=SC1091
. "$VERIFY_DIR/fetch_repos.sh"

suite_init "03_real_repos" "Real-World Ecosystem Integration & Fidelity"

QUICK_MODE="${QUICK:-0}"
FILE_COUNT=2000
[ "$QUICK_MODE" -eq 1 ] && FILE_COUNT=800

BASE_TMP="$(mktemp -d "${TMPDIR:-/tmp}/wt-real-repos.XXXXXX")"
export WT_STORE="$BASE_TMP/store"
mkdir -p "$WT_STORE"

# -----------------------------------------------------------------------------
# 1. Node.js Ecosystem
# -----------------------------------------------------------------------------
test_start "real_node_repo" "Hydrate real Node.js repo with deeply nested node_modules and .bin symlinks"
node_origin="$BASE_TMP/node-origin"
node_wt="$BASE_TMP/node-wt"

prepare_node_repo "$node_origin" "$FILE_COUNT"
cd "$node_origin"

t0=$(now)
out_node=$(wt_json new test-node --dir "$node_wt")
t1=$(now)
node_ms=$(elapsed_ms "$t0" "$t1")

assert_json_ok "$out_node" "Node.js worktree creation failed"
assert_file_exists "$node_wt/node_modules" "node_modules missing in worktree"
assert_triple_axis "$node_origin/node_modules" "$node_wt/node_modules" "Node.js triple-axis check failed"

# Test binary symlink inside .bin works
read -r first_bin < <(find "$node_wt/node_modules/.bin" -type l 2>/dev/null)
[ -n "$first_bin" ] && [ -x "$first_bin" ] || test_fail "real_node_repo" ".bin symlink not executable"

wt_json clean test-node --dir "$node_wt" --force >/dev/null
test_pass "real_node_repo" "{\"duration_ms\": $node_ms, \"files\": $FILE_COUNT}"

# -----------------------------------------------------------------------------
# 2. Rust Workspace Ecosystem
# -----------------------------------------------------------------------------
test_start "real_rust_repo" "Hydrate real Rust workspace with target/debug, rlibs, and build outputs"
rust_origin="$BASE_TMP/rust-origin"
rust_wt="$BASE_TMP/rust-wt"

prepare_rust_repo "$rust_origin" "$FILE_COUNT"
cd "$rust_origin"

t0=$(now)
out_rust=$(wt_json new test-rust --dir "$rust_wt")
t1=$(now)
rust_ms=$(elapsed_ms "$t0" "$t1")

assert_json_ok "$out_rust" "Rust worktree creation failed"
assert_file_exists "$rust_wt/target/debug" "target/debug missing in worktree"
assert_triple_axis "$rust_origin/target/debug/deps" "$rust_wt/target/debug/deps" "Rust deps triple-axis check failed"
assert_triple_axis "$rust_origin/target/debug/.fingerprint" "$rust_wt/target/debug/.fingerprint" "Rust fingerprint triple-axis check failed"
assert_file_not_exists "$rust_wt/target/debug/incremental" "Rust volatile incremental cache must be excluded"

# Test simulated binary execution
if [ -x "$rust_wt/target/debug/app" ]; then
    app_output=$("$rust_wt/target/debug/app")
    echo "$app_output" | grep -q "synthetic rust binary" || test_fail "real_rust_repo" "Rust binary execution output incorrect"
fi

wt_json clean test-rust --dir "$rust_wt" --force >/dev/null
test_pass "real_rust_repo" "{\"duration_ms\": $rust_ms, \"files\": $FILE_COUNT}"

# -----------------------------------------------------------------------------
# 3. Python Ecosystem
# -----------------------------------------------------------------------------
test_start "real_python_repo" "Hydrate real Python project with .venv, site-packages, and pycache"
py_origin="$BASE_TMP/py-origin"
py_wt="$BASE_TMP/py-wt"

prepare_python_repo "$py_origin" "$FILE_COUNT"
cd "$py_origin"

t0=$(now)
out_py=$(wt_json new test-python --dir "$py_wt")
t1=$(now)
py_ms=$(elapsed_ms "$t0" "$t1")

assert_json_ok "$out_py" "Python worktree creation failed"
assert_file_exists "$py_wt/.venv/lib" ".venv/lib missing in worktree"
assert_triple_axis "$py_origin/.venv" "$py_wt/.venv" "Python venv triple-axis check failed"

# Assert python symlink resolves to python3
assert_symlink "$py_wt/.venv/bin/python" "python3" "Python binary symlink invalid"

wt_json clean test-python --dir "$py_wt" --force >/dev/null
test_pass "real_python_repo" "{\"duration_ms\": $py_ms, \"files\": $FILE_COUNT}"

# -----------------------------------------------------------------------------
# 4. Multi-Ecosystem Monorepo
# -----------------------------------------------------------------------------
test_start "real_monorepo" "Hydrate multi-ecosystem monorepo (Node + Rust + Python) under single .wtinclude"
mono_origin="$BASE_TMP/mono-origin"
mono_wt="$BASE_TMP/mono-wt"

prepare_monorepo "$mono_origin"
cd "$mono_origin"

t0=$(now)
out_mono=$(wt_json new test-mono --dir "$mono_wt")
t1=$(now)
mono_ms=$(elapsed_ms "$t0" "$t1")

assert_json_ok "$out_mono" "Monorepo worktree creation failed"
assert_triple_axis "$mono_origin/apps/web/node_modules" "$mono_wt/apps/web/node_modules" "Monorepo web app triple-axis failed"
assert_triple_axis "$mono_origin/crates/core/target/debug/deps" "$mono_wt/crates/core/target/debug/deps" "Monorepo rust target triple-axis failed"
assert_triple_axis "$mono_origin/services/api/.venv" "$mono_wt/services/api/.venv" "Monorepo python venv triple-axis failed"

wt_json clean test-mono --dir "$mono_wt" --force >/dev/null
test_pass "real_monorepo" "{\"duration_ms\": $mono_ms}"

# Teardown
rm -rf "$BASE_TMP" 2>/dev/null || true

suite_finish
