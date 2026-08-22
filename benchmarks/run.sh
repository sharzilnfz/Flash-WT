#!/usr/bin/env bash
# Tickets 08 + 01: public benchmark suite.
#
# One command reproduces the macOS-versus-Linux scenario numbers on
# whatever machine it runs on: plain `git worktree add` plus a fresh
# dependency install (simulated by writing the same fixture tree file
# by file, which is what an install does at the filesystem level),
# a direct recursive CoW clone of the heavy tree (the strongest
# simple alternative to any store), and `wt create`, against
# identical generated fixtures of thousands of small files. Prints a
# markdown results table suitable for pasting into the README and
# launch post, plus physical disk usage of each hydrated tree.
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

# Direct-CoW scenario mechanism (ticket 01). Exactly one command per
# platform, chosen up front; no fallback chains that could silently
# turn the scenario into something else.
#
#   macOS: `cp -c` clones every file through copyfile(3) with
#     COPYFILE_CLONE, which lands in fclonefileat(2) — clonefile(2)
#     semantics per file. Same physics wt's CoW materialization uses.
#   Linux: GNU cp's `--reflink=auto` issues FICLONE per file on
#     filesystems with shared-extent support (btrfs, xfs, f2fs...).
#     On a filesystem without CoW (e.g. ext4 on stock CI runners)
#     auto degrades to a plain byte copy. That degradation is
#     deliberate and visible rather than fatal: CI runs this suite on
#     ext4, and the disk-usage report below shows whether sharing
#     actually happened (allocated << apparent) or not (allocated ~=
#     apparent). We never mix in hardlinks.
case "$(uname -s)" in
    Darwin) COW_CP=(cp -Rc) ;;
    Linux) COW_CP=(cp -Ra --reflink=auto) ;;
    *) die "no direct-CoW clone mechanism known for $(uname -s)" ;;
esac

# Physical disk usage of a tree (ticket 01). Two raw numbers:
#
#   apparent  = sum of logical file bytes (st_size). What the files
#               claim to weigh.
#   allocated = sum of st_blocks * 512 over regular files. Blocks the
#               filesystem reports as backing those files.
#
# Known limitation, documented instead of papered over: on APFS a
# cloned file reports its FULL st_blocks while every block is still
# shared with its source, and st_blocks never shrinks back after the
# share ends. So `allocated` here is an upper bound on physical
# usage, NOT the tree's private footprint beyond shared blocks —
# naive `du` overcounts for exactly the same reason. Distinguishing
# shared from private storage needs volume-level free-space deltas,
# which no per-tree syscall exposes. Both numbers are reported raw;
# the signal lives in comparing them within and across scenarios.
disk_usage() { # tree -> "<apparent_bytes> <allocated_bytes>" on stdout
    local stat_args
    case "$(uname -s)" in
        Darwin) stat_args=(-f '%z %b') ;;
        *) stat_args=(-c '%s %b') ;;
    esac
    find "$1" -type f -exec stat "${stat_args[@]}" {} + | awk '
        { app += $1; alloc += $2 * 512 }
        END { printf "%.0f %.0f\n", app, alloc }'
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
# Each scenario function prints "<seconds> <apparent> <allocated>"
# so the disk-usage sample survives the command-substitution subshell.
baseline_run() { # name -> stats line
    name=$1
    dest="$WORK/$name"
    t0=$(now)
    git -C "$SRC" worktree add --detach -q "$dest" HEAD
    generate_tree "$dest/node_modules" "$files"
    t1=$(now)
    verify_tree "$SRC/node_modules" "$dest/node_modules"
    read -r usage_apparent usage_allocated <<<"$(disk_usage "$dest/node_modules")"
    drop_worktree "$dest"
    printf '%s %s %s\n' "$(elapsed "$t0" "$t1")" \
        "${usage_apparent:-0}" "${usage_allocated:-0}"
}

# Scenario B: wt create, hydration through the store. Each call gets
# a unique branch name; the store path decides cold versus warm.
wt_run() { # name store -> stats line
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
    read -r usage_apparent usage_allocated <<<"$(disk_usage "$dest/node_modules")"
    drop_worktree "$dest"
    printf '%s %s %s\n' "$(elapsed "$t0" "$t1")" \
        "${usage_apparent:-0}" "${usage_allocated:-0}"
}

# Scenario C (ticket 01): direct recursive CoW clone of the heavy
# tree, straight from source into the destination — the simplest
# alternative any store-based hydration has to beat. Timed, verified,
# and torn down exactly like the other scenarios. The destination is
# a plain directory, not a git worktree, so it goes away with rm -rf.
cow_run() { # name -> stats line
    name=$1
    dest="$WORK/cow-$name"
    mkdir -p "$dest"
    t0=$(now)
    "${COW_CP[@]}" "$SRC/node_modules" "$dest/node_modules" ||
        die "direct-CoW clone failed into $dest"
    t1=$(now)
    verify_tree "$SRC/node_modules" "$dest/node_modules"
    read -r usage_apparent usage_allocated <<<"$(disk_usage "$dest/node_modules")"
    rm -rf "$dest"
    printf '%s %s %s\n' "$(elapsed "$t0" "$t1")" \
        "${usage_apparent:-0}" "${usage_allocated:-0}"
}

echo "benchmarking: baseline x$runs, direct cow x$runs, wt cold x$runs, wt warm x$runs..."

base_times=""
base_app=0
base_alloc=0
i=1
while [ "$i" -le "$runs" ]; do
    read -r t base_app base_alloc <<<"$(baseline_run "base-$i")"
    base_times="$base_times $t"
    echo "  baseline run $i: ${t}s"
    i=$((i + 1))
done

cow_times=""
cow_app=0
cow_alloc=0
i=1
while [ "$i" -le "$runs" ]; do
    read -r t cow_app cow_alloc <<<"$(cow_run "cow-$i")"
    cow_times="$cow_times $t"
    echo "  direct cow run $i: ${t}s"
    i=$((i + 1))
done

cold_times=""
wt_app=0
wt_alloc=0
i=1
while [ "$i" -le "$runs" ]; do
    read -r t wt_app wt_alloc <<<"$(wt_run "cold-$i" "$WORK/store-cold-$i")"
    cold_times="$cold_times $t"
    echo "  wt cold run $i: ${t}s"
    i=$((i + 1))
done

warm_store="$WORK/store-warm"
wt_run "warm-store-0" "$warm_store" >/dev/null # populate, untimed
warm_times=""
i=1
while [ "$i" -le "$runs" ]; do
    read -r t wt_app wt_alloc <<<"$(wt_run "warm-$i" "$warm_store")"
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

# Same cold/warm split for the direct-CoW scenario: first run is the
# cold number, the rest collapse to a warm median.
set -- $cow_times
cow_cold=$1
shift
cow_warm="-"
if [ "$#" -gt 0 ]; then
    cow_warm=$(median "$@")
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
echo "- Direct-CoW mechanism: ${COW_CP[*]}"
echo "- Fixture: $files small files across $pkgs package directories"
echo "- Runs per scenario: $runs (cold = first run / empty store, warm = median of the rest)"
if [ "$verify" -eq 1 ]; then
    echo "- Every run byte-verified against the source fixture"
else
    echo "- Every run file-count verified against the source fixture"
fi
echo
# Apparent = sum of st_size over regular files. Allocated = sum of
# st_blocks * 512. On APFS a cloned file reports full st_blocks while
# sharing every block with its source, so allocated is an UPPER BOUND
# on physical usage, not the private footprint; naive du has the same
# bias (see disk_usage above). The signal is in comparing allocated
# against apparent within a row, and rows against each other.
echo "| Scenario | Cold (s) | Warm (s) | Apparent (bytes) | Allocated (bytes) |"
echo "|---|---|---|---|---|"
echo "| git worktree add + fresh dependency install | $base_cold | $base_warm | $base_app | $base_alloc |"
echo "| direct recursive CoW clone (${COW_CP[*]}) | $cow_cold | $cow_warm | $cow_app | $cow_alloc |"
echo "| wt create (store hydration) | $wt_cold | $wt_warm | $wt_app | $wt_alloc |"
echo
echo "Apparent size sums logical file bytes; allocated sums st_blocks x 512."
echo "Cloned files report full blocks while still sharing them with their"
echo "source, so allocated is an upper bound on physical usage — it does not"
echo "isolate private (unshared) storage."
echo
if [ "$speedup" != "-" ]; then
    echo "Warm speedup: ${speedup}x"
    echo
fi
echo "Raw times (s):"
echo "- baseline:$base_times"
echo "- direct cow:$cow_times"
echo "- wt cold:$cold_times"
echo "- wt warm:$warm_times"
