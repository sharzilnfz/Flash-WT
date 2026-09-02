#!/usr/bin/env bash
# Tickets 08 + 01 + T06: public benchmark suite.
#
# One command reproduces the macOS-versus-Linux scenario numbers on
# whatever machine it runs on: plain `git worktree add` plus a fresh
# dependency install (simulated by writing the same fixture tree file
# by file, which is what an install does at the filesystem level),
# a direct recursive CoW clone of the heavy tree (the strongest
# simple alternative to any store), and `wt create`, against identical
# generated fixtures. Prints markdown results tables suitable for
# pasting into the README and launch post, plus physical disk usage of
# each hydrated tree, plus per-stage wt create timings when the CLI's
# WT_TIMING instrumentation is present.
#
# Two fixture shapes run by default:
#
#   scenarios A-C  published shape: thousands of tiny unique files
#                  across hundreds of package directories
#                  (`--files`, default 4000)
#   scenario D     realistic node_modules-like shape (T06): ~40k files
#                  across ~800 packages, ~96% duplicate content across
#                  packages, mixed executable bits, nested and empty
#                  directories, .bin-style symlinks
#
# Usage:
#   ./benchmarks/run.sh [--files N] [--runs N] [--quick] [--verify]
#                       [--scenario LIST]
#
#   --files N       A-C fixture file count (default 4000)
#   --runs N        timed runs per scenario (default 3)
#   --quick         tiny fixtures, single run; smoke mode for CI
#   --verify        deep-verify every hydrated tree after its run:
#                   byte-compare regular files, compare symlink targets,
#                   compare directory/file modes
#   --scenario LIST comma-separated subset of a,b,c,d (default all;
#                   e.g. --scenario d runs only the large-fixture suite)
#
# Set WT_BIN=/path/to/wt to reuse a prebuilt binary instead of running
# cargo. Everything runs in a throwaway temp directory; nothing on the
# machine is touched.
#
# STAGE-TIMING CONTRACT: when the CLI is built with WT_TIMING support,
# `WT_TIMING=1 wt create` prints exactly one line per stage, on stdout
# or stderr, in this exact format:
#
#   wt-stage <name>=<milliseconds>
#
# with name ∈ { ingest, references, materialize, total }. This script
# captures those lines out of each wt create log and reports the
# per-stage medians alongside the wall-clock table. When the
# instrumentation is not merged yet (no such lines in any log), every
# stage column prints "-" instead of failing: the suite measures what
# the binary can tell it today.

set -euo pipefail

BENCH_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$BENCH_DIR/.." && pwd)"
# shellcheck disable=SC1091  # sourced at runtime, same directory
. "$BENCH_DIR/fixture.sh"
# shellcheck disable=SC1091
. "$BENCH_DIR/eval_metrics.sh"
# shellcheck disable=SC1091
. "$BENCH_DIR/eval_storage.sh"

FILES_DEFAULT=4000
RUNS_DEFAULT=3
D_FILES_DEFAULT=40000
D_FILES_QUICK=2000

files=$FILES_DEFAULT
runs=$RUNS_DEFAULT
d_files=$D_FILES_DEFAULT
quick=0
verify=0
scenarios="a,b,c,d"

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
        --scenario)
            [ $# -ge 2 ] || die "--scenario needs a value"
            scenarios=$2
            shift 2
            ;;
        *)
            die "unknown argument $1"
            ;;
    esac
done

if [ "$quick" -eq 1 ]; then
    [ "$files" -eq "$FILES_DEFAULT" ] && files=200
    [ "$d_files" -eq "$D_FILES_DEFAULT" ] && d_files=$D_FILES_QUICK
    [ "$runs" -eq "$RUNS_DEFAULT" ] && runs=1
fi

[ "$files" -ge 1 ] || die "--files must be >= 1"
[ "$d_files" -ge 1 ] || die "--d-files resolved to $d_files"
[ "$runs" -ge 1 ] || die "--runs must be >= 1"

do_small=0
do_large=0
oldIFS=$IFS
IFS=,
for s in $scenarios; do
    case "$s" in
        a | b | c) do_small=1 ;;
        d) do_large=1 ;;
        *) die "unknown scenario '$s' (want a, b, c, or d)" ;;
    esac
done
IFS=$oldIFS

BIN=${WT_BIN:-}
if [ -z "$BIN" ]; then
    echo "building release binary..."
    cargo build --release --quiet \
        --manifest-path "$REPO_ROOT/Cargo.toml" -p wt-cli
    BIN="$REPO_ROOT/target/release/wt"
fi
[ -x "$BIN" ] || die "binary not runnable at $BIN"


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
WORK="$(mktemp -d "${TMPDIR:-/tmp}/wt-bench.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

# ---------------------------------------------------------------------------
# Verification: deep-verify regular files, directory/file modes, and symlinks.
# ---------------------------------------------------------------------------
verify_tree() { # src dest
    local src=$1 dest=$2
    if [ "$verify" -eq 0 ]; then
        local a b
        a=$(count_files "$src")
        b=$(count_files "$dest")
        [ "$a" -eq "$b" ] ||
            die "file count mismatch under $dest: $a vs $b"
        return
    fi

    diff -rq "$src" "$dest" >/dev/null || die "tree under $dest differs from source $src"
    diff <(list_modes "$src") <(list_modes "$dest") >/dev/null || die "mode mismatch under $dest vs source $src"
    diff <(list_symlinks "$src") <(list_symlinks "$dest") >/dev/null || die "symlink mismatch under $dest vs source $src"
}

drop_worktree() {
    git -C "$SRC" worktree remove --force "$1" >/dev/null 2>&1 ||
        rm -rf "$1"
}

# ---------------------------------------------------------------------------
# Scenario runners. Each sets run_time, run_app, and run_alloc as
# globals instead of echoing a stats line: a runner invoked through
# command substitution could not abort the suite on failure (die would
# kill only the subshell and read would carry on with partial input).
# ---------------------------------------------------------------------------

baseline_run() { # name
    name=$1
    dest="$WORK/$name"
    t0=$(now)
    git -C "$SRC" worktree add --detach -q "$dest" HEAD
    "$gen_fn" "$dest/node_modules" "$suite_files"
    t1=$(now)
    verify_tree "$SRC/node_modules" "$dest/node_modules"
    read -r run_app run_allocated <<<"$(disk_usage "$dest/node_modules")"
    drop_worktree "$dest"
    run_time=$(elapsed "$t0" "$t1")
}

# Scenario B: wt create, hydration through the store. Each call gets
# a unique branch name; the store path decides cold versus warm.
# Runs always carry WT_TIMING=1 so merged instrumentation shows up
# here without edits (see the stage-timing contract in the header).
wt_run() { # name store phase
    name=$1
    store=$2
    phase=$3
    dest="$WORK/wt-$name"
    t0=$(now)
    (
        cd "$SRC" &&
            WT_STORE="$store" WT_TIMING=1 "$BIN" create "$name" \
                --dir "$dest" >"$WORK/$name.log" 2>&1
    ) || {
        cat "$WORK/$name.log" >&2
        die "wt create $name failed"
    }
    t1=$(now)
    record_stages "$phase" "$WORK/$name.log"
    verify_tree "$SRC/node_modules" "$dest/node_modules"
    read -r run_app run_allocated <<<"$(disk_usage "$dest/node_modules")"
    drop_worktree "$dest"
    run_time=$(elapsed "$t0" "$t1")
}

# Scenario C (ticket 01): direct recursive CoW clone of the heavy
# tree, straight from source into the destination — the simplest
# alternative any store-based hydration has to beat. Timed, verified,
# and torn down exactly like the other scenarios. The destination is
# a plain directory, not a git worktree, so it goes away with rm -rf.
cow_run() { # name
    name=$1
    dest="$WORK/cow-$name"
    mkdir -p "$dest"
    t0=$(now)
    "${COW_CP[@]}" "$SRC/node_modules" "$dest/node_modules" ||
        die "direct-CoW clone failed into $dest"
    t1=$(now)
    verify_tree "$SRC/node_modules" "$dest/node_modules"
    read -r run_app run_allocated <<<"$(disk_usage "$dest/node_modules")"
    rm -rf "$dest"
    run_time=$(elapsed "$t0" "$t1")
}

# ---------------------------------------------------------------------------
# Stage-timing capture. Implements the parsing side of the contract
# documented in the header: scan a wt create log using parse_stage_log
# and bucket the milliseconds by phase (cold/warm).
# ---------------------------------------------------------------------------
stage_cold_ingest=""
stage_cold_references=""
stage_cold_materialize=""
stage_cold_total=""
stage_warm_ingest=""
stage_warm_references=""
stage_warm_materialize=""
stage_warm_total=""

record_stages() { # phase logfile
    local phase=$1
    local k v
    while IFS='=' read -r k v; do
        case "$phase:$k" in
            cold:ingest) stage_cold_ingest="$stage_cold_ingest $v" ;;
            cold:references) stage_cold_references="$stage_cold_references $v" ;;
            cold:materialize) stage_cold_materialize="$stage_cold_materialize $v" ;;
            cold:total) stage_cold_total="$stage_cold_total $v" ;;
            warm:ingest) stage_warm_ingest="$stage_warm_ingest $v" ;;
            warm:references) stage_warm_references="$stage_warm_references $v" ;;
            warm:materialize) stage_warm_materialize="$stage_warm_materialize $v" ;;
            warm:total) stage_warm_total="$stage_warm_total $v" ;;
        esac
    done < <(parse_stage_log "$2")
}

stage_row() { # phase label -> one markdown stage-median row
    case $1 in
        cold)
            echo "| $2 | cold | $(median_or_dash "$stage_cold_ingest") | $(median_or_dash "$stage_cold_references") | $(median_or_dash "$stage_cold_materialize") | $(median_or_dash "$stage_cold_total") |"
            ;;
        warm)
            echo "| $2 | warm | $(median_or_dash "$stage_warm_ingest") | $(median_or_dash "$stage_warm_references") | $(median_or_dash "$stage_warm_materialize") | $(median_or_dash "$stage_warm_total") |"
            ;;
    esac
}

# ---------------------------------------------------------------------------
# One fixture suite: set up an origin repo + heavy tree, run every
# selected scenario against it, print its results section.
# ---------------------------------------------------------------------------

platform="$(uname -s) $(uname -r) $(uname -m)"

run_suite() { # label generator_function file_count
    suite_label=$1
    gen_fn=$2
    suite_files=$3

    case $gen_fn in
        generate_tree) per_pkg=$FILES_PER_PKG ;;
        generate_tree_d) per_pkg=$FILES_PER_PKG_D ;;
        *) die "unknown generator $gen_fn" ;;
    esac
    suite_pkgs=$((suite_files / per_pkg))

    stage_cold_ingest=""
    stage_cold_references=""
    stage_cold_materialize=""
    stage_cold_total=""
    stage_warm_ingest=""
    stage_warm_references=""
    stage_warm_materialize=""
    stage_warm_total=""

    SRC="$WORK/origin-$suite_label"
    echo
    echo "== suite '$suite_label': setting up fixture repository..."
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

    "$gen_fn" "$SRC/node_modules" "$suite_files"
    count=$(count_files "$SRC/node_modules")
    [ "$count" -eq "$suite_files" ] ||
        die "fixture generator produced $count files, expected $suite_files"

    echo "== suite '$suite_label': benchmarking baseline x$runs, direct cow x$runs, wt cold x$runs, wt warm x$runs..."

    base_times=""
    base_app=0
    base_alloc=0
    ri=1
    while [ "$ri" -le "$runs" ]; do
        baseline_run "base-$ri"
        base_times="$base_times $run_time"
        base_app=$run_app
        base_alloc=$run_allocated
        echo "  baseline run $ri: ${run_time}s"
        ri=$((ri + 1))
    done

    cow_times=""
    cow_app=0
    cow_alloc=0
    ri=1
    while [ "$ri" -le "$runs" ]; do
        cow_run "cow-$ri"
        cow_times="$cow_times $run_time"
        cow_app=$run_app
        cow_alloc=$run_allocated
        echo "  direct cow run $ri: ${run_time}s"
        ri=$((ri + 1))
    done

    cold_times=""
    wt_app=0
    wt_alloc=0
    ri=1
    while [ "$ri" -le "$runs" ]; do
        wt_run "cold-$ri" "$WORK/store-cold-$ri" cold
        cold_times="$cold_times $run_time"
        wt_app=$run_app
        wt_alloc=$run_allocated
        echo "  wt cold run $ri: ${run_time}s"
        ri=$((ri + 1))
    done

    warm_store="$WORK/store-warm-$suite_label"
    wt_run "warm-store-0" "$warm_store" warm # populate, untimed
    warm_times=""
    ri=1
    while [ "$ri" -le "$runs" ]; do
        wt_run "warm-$ri" "$warm_store" warm
        warm_times="$warm_times $run_time"
        wt_app=$run_app
        wt_alloc=$run_allocated
        echo "  wt warm run $ri: ${run_time}s"
        ri=$((ri + 1))
    done

    # Cold baseline = first run (nothing cached); warm = median of the
    # rest. Same split for every scenario.
    # shellcheck disable=SC2086  # deliberate word split over the times
    set -- $base_times
    base_cold=$1
    shift
    base_warm="-"
    if [ "$#" -gt 0 ]; then
        base_warm=$(median "$@")
    fi

    # shellcheck disable=SC2086  # deliberate word split over the times
    set -- $cow_times
    cow_cold=$1
    shift
    cow_warm="-"
    if [ "$#" -gt 0 ]; then
        cow_warm=$(median "$@")
    fi

    wt_cold=$(median_or_dash "$cold_times")
    wt_warm=$(median_or_dash "$warm_times")

    speedup="-"
    if [ "$base_warm" != "-" ] && awk -v w="$wt_warm" 'BEGIN { exit !(w > 0) }'; then
        speedup=$(awk -v b="$base_warm" -v w="$wt_warm" 'BEGIN { printf "%.1f", b / w }')
    fi

    echo
    echo "## Benchmark results — fixture '$suite_label'"
    echo
    echo "- Platform: $platform"
    echo "- Fixture: $suite_files small files across $suite_pkgs package directories ($gen_fn)"
    echo "- Direct-CoW mechanism: ${COW_CP[*]}"
    echo "- Runs per scenario: $runs (cold = first run / empty store, warm = median of the rest)"
    if [ "$verify" -eq 1 ]; then
        echo "- Every run deep-verified: file bytes, symlink targets, directory/file modes"
    else
        echo "- Every run file-count verified against the source fixture"
    fi
    echo
    # Apparent = sum of st_size over regular files. Allocated = sum of
    # st_blocks * 512. On APFS a cloned file reports full blocks while
    # still sharing them with their source, so allocated is an UPPER
    # BOUND on physical usage, not the private footprint; naive du has
    # the same bias (see disk_usage above). The signal is in comparing
    # allocated against apparent within a row, and rows against each
    # other.
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
    echo "### wt create stage timings (ms, median of timed runs)"
    echo
    echo "Contract: \`WT_TIMING=1 wt create\` emits \`wt-stage <name>=<milliseconds>\`"
    echo "per stage; \"-\" means the binary printed no such lines."
    echo
    echo "| Fixture | Phase | Ingest (ms) | References (ms) | Materialize (ms) | Total (ms) |"
    echo "|---|---|---|---|---|---|"
    stage_row cold "$suite_label"
    stage_row warm "$suite_label"
    echo
    if [ "$speedup" != "-" ]; then
        echo "Warm speedup (vs fresh dependency install): ${speedup}x"
        echo
    fi
    echo "Raw times (s):"
    echo "- baseline:$base_times"
    echo "- direct cow:$cow_times"
    echo "- wt cold:$cold_times"
    echo "- wt warm:$warm_times"

    rm -rf "$SRC"
}

echo "Benchmarking wt on platform: $platform"
echo "Scenarios selected: $scenarios"

if [ "$do_small" -eq 1 ]; then
    run_suite "small" generate_tree "$files"
fi
if [ "$do_large" -eq 1 ]; then
    run_suite "large" generate_tree_d "$d_files"
fi
