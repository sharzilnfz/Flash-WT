#!/usr/bin/env bash
# chaos.sh — Automated fault-injection and crash resilience test runner.
# Sourced or executed directly to verify store self-healing after SIGKILL.

set -euo pipefail

BENCH_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$BENCH_DIR/.." && pwd)"

# shellcheck disable=SC1091
. "$BENCH_DIR/fixture.sh"
# shellcheck disable=SC1091
. "$BENCH_DIR/fixture_matrix.sh"
# shellcheck disable=SC1091
. "$BENCH_DIR/eval_metrics.sh"

run_chaos_test() {
    local default_bin="$REPO_ROOT/target/release/flashwt"
    if [ ! -x "$default_bin" ] && [ -x "$REPO_ROOT/target/release/wt" ]; then
        default_bin="$REPO_ROOT/target/release/wt"
    fi
    local raw_bin=${1:-"$default_bin"}
    local kills=${2:-3}

    [ -x "$raw_bin" ] || {
        echo "chaos: binary $raw_bin not found or executable" >&2
        return 1
    }

    local bin
    bin="$(cd "$(dirname "$raw_bin")" && pwd)/$(basename "$raw_bin")"

    local work
    work="$(mktemp -d "${TMPDIR:-/tmp}/wt-chaos.XXXXXX")"

    local store="$work/store"
    local src="$work/origin"
    mkdir -p "$src" "$store"

    git init -q "$src"
    git -C "$src" config user.email chaos@example.com
    git -C "$src" config user.name Chaos
    printf 'node_modules/\n' >"$src/.gitignore"
    printf 'node_modules/\n' >"$src/.wtinclude"
    printf 'console.log("chaos");\n' >"$src/index.js"
    git -C "$src" add .
    git -C "$src" commit -qm init

    generate_tree_d "$src/node_modules" 2000

    echo "== running chaos fault-injection with $kills SIGKILL interruptions..."

    local k=1
    while [ "$k" -le "$kills" ]; do
        local dest="$work/wt-chaos-$k"
        local delay=$(awk -v k="$k" 'BEGIN { printf "%.3f", 0.010 * k }')

        # Launch creation in background
        (
            cd "$src" &&
                WT_STORE="$store" "$bin" create "chaos-$k" --dir "$dest" >"$work/chaos-$k.log" 2>&1
        ) &
        local pid=$!

        # Sleep brief fraction then SIGKILL
        sleep "$delay"
        kill -9 "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true

        echo "  [kill $k] terminated PID $pid after ${delay}s"

        # Verify no store blob corruption: every blob in store must hash to its prefix + filename
        if [ -d "$store/objects" ]; then
            while read -r obj; do
                local prefix fname expected_hash actual_hash
                prefix=$(basename "$(dirname "$obj")")
                fname=$(basename "$obj")
                expected_hash="${prefix}${fname}"
                actual_hash=$(sha256_hash "$obj")
                if [ "$expected_hash" != "$actual_hash" ]; then
                    echo "chaos: corrupt blob found in store: $obj ($expected_hash vs $actual_hash)" >&2
                    rm -rf "$work"
                    return 1
                fi
            done < <(find "$store/objects" -type f)
        fi

        # Attempt recovery: retry create on the same store
        local rec_dest="$work/wt-recovery-$k"
        (
            cd "$src" &&
                WT_STORE="$store" "$bin" create "rec-$k" --dir "$rec_dest" >"$work/rec-$k.log" 2>&1
        ) || {
            echo "chaos: recovery create failed after kill $k" >&2
            cat "$work/rec-$k.log" >&2
            rm -rf "$work"
            return 1
        }

        # Deep verify the recovered tree
        diff -rq "$src/node_modules" "$rec_dest/node_modules" >/dev/null || {
            echo "chaos: recovered tree does not match source after kill $k" >&2
            rm -rf "$work"
            return 1
        }

        echo "  [kill $k] store integrity intact, recovery create succeeded and deep-verified"
        git -C "$src" worktree remove --force "$rec_dest" >/dev/null 2>&1 || rm -rf "$rec_dest"

        k=$((k + 1))
    done

    rm -rf "$work"
    echo "== chaos resilience test passed: 0 corruptions across $kills crashes"
    return 0
}

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    run_chaos_test "${1:-}" "${2:-3}"
fi
