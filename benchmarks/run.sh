#!/usr/bin/env bash
# Ticket 08: public benchmark suite.
#
# One command reproduces the macOS-versus-Linux scenario numbers on
# whatever machine it runs on: plain `git worktree add` plus a fresh
# dependency install (simulated by writing the same fixture tree file
# by file, which is what an install does at the filesystem level)
# versus `wt create`, against identical generated fixtures of
# thousands of small files. Prints a markdown results table suitable
# for pasting into the README and launch post.
#
# Usage:
#   ./benchmarks/run.sh [--files N] [--runs N] [--quick] [--verify]
#
#   --files N   fixture file count (default 4000, matching the shape
#               of a real node_modules install)
#   --runs N    timed runs per scenario (default 3)
#   --quick     tiny fixture, single run; smoke mode for CI
#   --verify    byte-compare every hydrated tree after its run
#
# Set WT_BIN=/path/to/wt to reuse a prebuilt binary instead of running
# cargo. Everything runs in a throwaway temp directory; nothing on the
# machine is touched.

set -euo pipefail

BENCH_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$BENCH_DIR/.." && pwd)"
# shellcheck source=fixture.sh
. "$BENCH_DIR/fixture.sh"

FILES_DEFAULT=4000
RUNS_DEFAULT=3
files=$FILES_DEFAULT
runs=$RUNS_DEFAULT
quick=0
verify=0

die() {
    echo "benchmarks: $*" >&2
    exit 1
}

while [ $# -gt 0 ]; do
    case "$1" in
        --files)
            [ $# -ge 2 ] || die "--files needs a value"
            files=$2
            shift 2
            ;;
        --runs)
            [ $# -ge 2 ] || die "--runs needs a value"
            runs=$2
            shift 2
            ;;
        --quick)
            quick=1
            shift
            ;;
        --verify)
            verify=1
            shift
            ;;
        *)
            die "unknown argument $1"
            ;;
    esac
done

if [ "$quick" -eq 1 ]; then
    [ "$files" -eq "$FILES_DEFAULT" ] && files=200
    [ "$runs" -eq "$RUNS_DEFAULT" ] && runs=1
fi

[ "$files" -ge 1 ] || die "--files must be >= 1"
[ "$runs" -ge 1 ] || die "--runs must be >= 1"

BIN=${WT_BIN:-}
if [ -z "$BIN" ]; then
    echo "building release binary..."
    cargo build --release --quiet \
        --manifest-path "$REPO_ROOT/Cargo.toml" -p wt-cli
    BIN="$REPO_ROOT/target/release/wt"
fi
[ -x "$BIN" ] || die "binary not runnable at $BIN"

# Millisecond-resolution clocks without a compile step: perl ships
# with both macOS and Linux.
now() {
    perl -MTime::HiRes=time -e 'printf "%.6f\n", time'
}
elapsed() { # start end -> seconds, 3 decimals
    awk -v a="$1" -v b="$2" 'BEGIN { printf "%.3f", b - a }'
}

median() { # numbers on argv -> median, 3 decimals
    printf '%s\n' "$@" | sort -g | awk '
        { v[NR] = $1 }
        END {
            if (NR % 2) { printf "%.3f", v[(NR + 1) / 2] }
            else { printf "%.3f", (v[NR / 2] + v[NR / 2 + 1]) / 2 }
        }'
}

WORK="$(mktemp -d "${TMPDIR:-/tmp}/wt-bench.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
SRC="$WORK/origin"

echo "setting up fixture repository..."
mkdir "$SRC"
git init -q "$SRC"
git -C "$SRC" config user.email bench@example.com
git -C "$SRC" config user.name Bench
printf 'node_modules/\n' >"$SRC/.gitignore"
mkdir "$SRC/src"
i=1
while [ "$i" -le 3 ]; do
    printf 'export const m%d = %d;\n' "$i" "$i" >"$SRC/src/mod-$i.js"
    i=$((i + 1))
done
printf 'node_modules/\n' >"$SRC/.wtinclude"
git -C "$SRC" add .
git -C "$SRC" commit -qm init

generate_tree "$SRC/node_modules" "$files"
count=$(count_files "$SRC/node_modules")
[ "$count" -eq "$files" ] ||
    die "fixture generator produced $count files, expected $files"
pkgs=$((files / FILES_PER_PKG))

# After each timed run the hydrated or installed tree must match the
# source fixture — a fast benchmark that copies wrong is worthless,
# and unattended CI has no human to eyeball it.
verify_tree() {
    if [ "$verify" -eq 1 ]; then
        diff -rq "$1" "$2" >/dev/null ||
            die "tree under $2 differs from source $1"
    else
        a=$(count_files "$1")
        b=$(count_files "$2")
        [ "$a" -eq "$b" ] ||
            die "file count mismatch under $2: $a vs $b"
    fi
}

drop_worktree() {
    git -C "$SRC" worktree remove --force "$1" >/dev/null 2>&1 ||
        rm -rf "$1"
}

# Scenario A: plain git worktree add plus fresh dependency install.
baseline_run() { # name -> seconds
    name=$1
    dest="$WORK/$name"
    t0=$(now)
    git -C "$SRC" worktree add --detach -q "$dest" HEAD
    generate_tree "$dest/node_modules" "$files"
    t1=$(now)
    verify_tree "$SRC/node_modules" "$dest/node_modules"
    drop_worktree "$dest"
    elapsed "$t0" "$t1"
}

# Scenario B: wt create, hydration through the store. Each call gets
# a unique branch name; the store path decides cold versus warm.
wt_run() { # name store -> seconds
    name=$1
    store=$2
    dest="$WORK/wt-$name"
    t0=$(now)
    (
        cd "$SRC" &&
            WT_STORE="$store" "$BIN" create "$name" --dir "$dest" \
                >"$WORK/$name.log" 2>&1
    ) || {
        cat "$WORK/$name.log" >&2
        die "wt create $name failed"
    }
    t1=$(now)
    verify_tree "$SRC/node_modules" "$dest/node_modules"
    drop_worktree "$dest"
    elapsed "$t0" "$t1"
}

echo "benchmarking: baseline x$runs, wt cold x$runs, wt warm x$runs..."

base_times=""
i=1
while [ "$i" -le "$runs" ]; do
    t=$(baseline_run "base-$i")
    base_times="$base_times $t"
    echo "  baseline run $i: ${t}s"
    i=$((i + 1))
done

cold_times=""
i=1
while [ "$i" -le "$runs" ]; do
    t=$(wt_run "cold-$i" "$WORK/store-cold-$i")
    cold_times="$cold_times $t"
    echo "  wt cold run $i: ${t}s"
    i=$((i + 1))
done

warm_store="$WORK/store-warm"
wt_run "warm-store-0" "$warm_store" >/dev/null # populate, untimed
warm_times=""
i=1
while [ "$i" -le "$runs" ]; do
    t=$(wt_run "warm-$i" "$warm_store")
    warm_times="$warm_times $t"
    echo "  wt warm run $i: ${t}s"
    i=$((i + 1))
done

# Cold baseline = first run (nothing cached); warm = median of the rest.
set -- $base_times
base_cold=$1
shift
base_warm="-"
if [ "$#" -gt 0 ]; then
    base_warm=$(median "$@")
fi
wt_cold=$(median $cold_times)
wt_warm=$(median $warm_times)

speedup="-"
if [ "$base_warm" != "-" ] && awk -v b="$base_warm" -v w="$wt_warm" 'BEGIN { exit !(w > 0) }'; then
    speedup=$(awk -v b="$base_warm" -v w="$wt_warm" 'BEGIN { printf "%.1f", b / w }')
fi

platform="$(uname -s) $(uname -r) $(uname -m)"
echo
echo "## Benchmark results"
echo
echo "- Platform: $platform"
echo "- Fixture: $files small files across $pkgs package directories"
echo "- Runs per scenario: $runs (cold = first run / empty store, warm = median of the rest)"
if [ "$verify" -eq 1 ]; then
    echo "- Every run byte-verified against the source fixture"
else
    echo "- Every run file-count verified against the source fixture"
fi
echo
echo "| Scenario | Cold (s) | Warm (s) |"
echo "|---|---|---|"
echo "| git worktree add + fresh dependency install | $base_cold | $base_warm |"
echo "| wt create (store hydration) | $wt_cold | $wt_warm |"
echo
if [ "$speedup" != "-" ]; then
    echo "Warm speedup: ${speedup}x"
    echo
fi
echo "Raw times (s):"
echo "- baseline:$base_times"
echo "- wt cold:$cold_times"
echo "- wt warm:$warm_times"
