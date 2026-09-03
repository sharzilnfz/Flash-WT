#!/usr/bin/env bash

set -euo pipefail

BENCH_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$BENCH_DIR/.." && pwd)"
. "$BENCH_DIR/fixture.sh"
. "$BENCH_DIR/eval_metrics.sh"
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

BIN=${FLASHWT_BIN:-}
if [ -z "$BIN" ]; then
    echo "building release binary..."
    cargo build --release --quiet \
        --manifest-path "$REPO_ROOT/Cargo.toml" -p flashwt-cli
    if [ -x "$REPO_ROOT/target/release/flashwt" ]; then
        BIN="$REPO_ROOT/target/release/flashwt"
    else
        BIN="$REPO_ROOT/target/release/flashwt"
    fi
fi
[ -x "$BIN" ] || die "binary not runnable at $BIN"

case "$(uname -s)" in
    Darwin) COW_CP=(cp -Rc) ;;
    Linux) COW_CP=(cp -Ra --reflink=auto) ;;
    *) die "no direct-CoW clone mechanism known for $(uname -s)" ;;
esac

WORK="$(mktemp -d "${TMPDIR:-/tmp}/flashwt-bench.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

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

flashwt_run() { # name store phase
    name=$1
    store=$2
    phase=$3
    dest="$WORK/flashwt-$name"
    t0=$(now)
    (
        cd "$SRC" &&
            FLASHWT_STORE="$store" FLASHWT_TIMING=1 "$BIN" create "$name" \
                --dir "$dest" >"$WORK/$name.log" 2>&1
    ) || {
        cat "$WORK/$name.log" >&2
        die "flashwt create $name failed"
    }
    t1=$(now)
    record_stages "$phase" "$WORK/$name.log"
    verify_tree "$SRC/node_modules" "$dest/node_modules"
    read -r run_app run_allocated <<<"$(disk_usage "$dest/node_modules")"
    drop_worktree "$dest"
    run_time=$(elapsed "$t0" "$t1")
}

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
    printf 'node_modules/\n' >"$SRC/.flashwtinclude"
    git -C "$SRC" add .
    git -C "$SRC" commit -qm init

    "$gen_fn" "$SRC/node_modules" "$suite_files"
    count=$(count_files "$SRC/node_modules")
    [ "$count" -eq "$suite_files" ] ||
        die "fixture generator produced $count files, expected $suite_files"

    echo "== suite '$suite_label': benchmarking baseline x$runs, direct cow x$runs, flashwt cold x$runs, flashwt warm x$runs..."

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
    flashwt_app=0
    flashwt_alloc=0
    ri=1
    while [ "$ri" -le "$runs" ]; do
        flashwt_run "cold-$ri" "$WORK/store-cold-$ri" cold
        cold_times="$cold_times $run_time"
        flashwt_app=$run_app
        flashwt_alloc=$run_allocated
        echo "  flashwt cold run $ri: ${run_time}s"
        ri=$((ri + 1))
    done

    warm_store="$WORK/store-warm-$suite_label"
    flashwt_run "warm-store-0" "$warm_store" warm # populate, untimed
    warm_times=""
    ri=1
    while [ "$ri" -le "$runs" ]; do
        flashwt_run "warm-$ri" "$warm_store" warm
        warm_times="$warm_times $run_time"
        flashwt_app=$run_app
        flashwt_alloc=$run_allocated
        echo "  flashwt warm run $ri: ${run_time}s"
        ri=$((ri + 1))
    done

    set -- $base_times
    base_cold=$1
    shift
    base_warm="-"
    if [ "$#" -gt 0 ]; then
        base_warm=$(median "$@")
    fi

    set -- $cow_times
    cow_cold=$1
    shift
    cow_warm="-"
    if [ "$#" -gt 0 ]; then
        cow_warm=$(median "$@")
    fi

    flashwt_cold=$(median_or_dash "$cold_times")
    flashwt_warm=$(median_or_dash "$warm_times")

    speedup="-"
    if [ "$base_warm" != "-" ] && awk -v w="$flashwt_warm" 'BEGIN { exit !(w > 0) }'; then
        speedup=$(awk -v b="$base_warm" -v w="$flashwt_warm" 'BEGIN { printf "%.1f", b / w }')
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
    echo "| Scenario | Cold (s) | Warm (s) | Apparent (bytes) | Allocated (bytes) |"
    echo "|---|---|---|---|---|"
    echo "| git worktree add + fresh dependency install | $base_cold | $base_warm | $base_app | $base_alloc |"
    echo "| direct recursive CoW clone (${COW_CP[*]}) | $cow_cold | $cow_warm | $cow_app | $cow_alloc |"
    echo "| flashwt create (store hydration) | $flashwt_cold | $flashwt_warm | $flashwt_app | $flashwt_alloc |"
    echo
    echo "Apparent size sums logical file bytes; allocated sums st_blocks x 512."
    echo "Cloned files report full blocks while still sharing them with their"
    echo "source, so allocated is an upper bound on physical usage — it does not"
    echo "isolate private (unshared) storage."
    echo
    echo "### flashwt create stage timings (ms, median of timed runs)"
    echo
    echo "Contract: \`FLASHWT_TIMING=1 flashwt create\` emits \`flashwt-stage <name>=<milliseconds>\`"
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
    echo "- flashwt cold:$cold_times"
    echo "- flashwt warm:$warm_times"

    rm -rf "$SRC"
}

echo "Benchmarking flashwt on platform: $platform"
echo "Scenarios selected: $scenarios"

if [ "$do_small" -eq 1 ]; then
    run_suite "small" generate_tree "$files"
fi
if [ "$do_large" -eq 1 ]; then
    run_suite "large" generate_tree_d "$d_files"
fi

