#!/usr/bin/env bash

set -euo pipefail

BENCH_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$BENCH_DIR/.." && pwd)"
. "$BENCH_DIR/fixture.sh"
. "$BENCH_DIR/eval_metrics.sh"

die() {
    echo "benchmarks: $*" >&2
    exit 1
}

FILES_DEFAULT=40000
SAMPLES_DEFAULT=3

files=${FLASHWT_BENCH_FILES:-$FILES_DEFAULT}
samples=${FLASHWT_BENCH_SAMPLES:-$SAMPLES_DEFAULT}

case $files in
    '' | *[!0-9]*) die "FLASHWT_BENCH_FILES must be a positive integer" ;;
esac
[ "$files" -ge 1 ] || die "FLASHWT_BENCH_FILES must be >= 1"
case $samples in
    '' | *[!0-9]*) die "FLASHWT_BENCH_SAMPLES must be a positive integer" ;;
esac
[ "$samples" -ge 1 ] || die "FLASHWT_BENCH_SAMPLES must be >= 1"

if [ "$(uname -s)" != Darwin ]; then
    die "this harness measures macOS/APFS-only features; run it on a Mac"
fi

BIN=${FLASHWT_BIN:-}
if [ -z "$BIN" ]; then
    if [ -x "$REPO_ROOT/target/release/flashwt" ]; then
        BIN="$REPO_ROOT/target/release/flashwt"
    else
        BIN="$REPO_ROOT/target/release/flashwt"
    fi
fi
if [ ! -x "$BIN" ]; then
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

WORK="$(mktemp -d "${TMPDIR:-/tmp}/flashwt-v2-bench.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

drop_worktree() {
    git -C "$SRC" worktree remove --force "$1" >/dev/null 2>&1 ||
        rm -rf "$1"
}

verify_tree() { # donor dest label
    local raw
    raw=$(diff -r "$1" "$2") || true
    [ -z "$raw" ] ||
        die "hydrated tree under $2 does not match donor $1 ($3):
$(printf '%s\n' "$raw" | first_lines 10)"
}

st_mode="-"
st_blt="-"
st_v2c="-"
st_v2l="-"

parse_stages() { # logfile
    st_mode="-"
    st_blt="-"
    st_v2c="-"
    st_v2l="-"
    local k v
    while IFS='=' read -r k v; do
        case "$k" in
            snapshot-mode) st_mode=$v ;;
            snapshot-build-link-train) st_blt=$v ;;
            snapshot-v2-cloned) st_v2c=$v ;;
            snapshot-v2-linked) st_v2l=$v ;;
        esac
    done < <(parse_stage_log "$1")
}

run_create() { # name store env-pairs...
    local name=$1 store=$2
    shift 2
    dest="$WORK/flashwt-$name"
    local t0 t1
    t0=$(now)
    (
        cd "$SRC" &&
            env FLASHWT_STORE="$store" FLASHWT_TIMING=1 "$@" \
                "$BIN" create "$name" --dir "$dest" >"$WORK/$name.log" 2>&1
    ) || {
        cat "$WORK/$name.log" >&2
        die "flashwt create $name failed"
    }
    t1=$(now)
    parse_stages "$WORK/$name.log"
    verify_tree "$SRC/node_modules" "$dest/node_modules" "$name"
    drop_worktree "$dest"
    run_wall=$(elapsed "$t0" "$t1")
}

row() { # cell sample
    printf '| %s | %s | %s | %s | %s | %s | %s |\n' \
        "$1" "$2" "$run_wall" "$st_mode" "$st_blt" "$st_v2c" "$st_v2l" \
        >>"$WORK/rows.md"
}

bump_packages() { # sample-index
    local k p
    for k in 0 1 2; do
        p="$SRC/node_modules/pkg-$(printf '%05d' "$k")"
        [ -f "$p/package.json" ] || die "fixture package missing: $p"
        printf '{"name":"pkg-%05d","version":"2.0.%s","bump":"sample-%s"}\n' \
            "$k" "$1" "$1" >"$p/package.json"
    done
}

platform="$(uname -s) $(uname -r) $(uname -m)"
pkgs=$((files / FILES_PER_PKG_D))

SRC="$WORK/origin"
echo "== setting up fixture repository ($files files, ~$pkgs packages)..."
mkdir "$SRC"
git init -q "$SRC"
git -C "$SRC" config user.email bench@example.com
git -C "$SRC" config user.name Bench
printf 'node_modules/\n' >"$SRC/.gitignore"
mkdir "$SRC/src"
printf 'export const m = 1;\n' >"$SRC/src/mod-1.js"
printf 'node_modules/\n' >"$SRC/.flashwtinclude"
git -C "$SRC" add .
git -C "$SRC" commit -qm init

generate_tree_d "$SRC/node_modules" "$files"
count=$(count_files "$SRC/node_modules")
[ "$count" -eq "$files" ] ||
    die "fixture generator produced $count files, expected $files"

touch "$WORK/rows.md"

echo "== cell warm-hit: populate + x$samples timed creates..."
run_create "warm-seed" "$WORK/store-warm" FLASHWT_SNAPSHOTS=1
warm_walls=""
i=1
while [ "$i" -le "$samples" ]; do
    run_create "warm-$i" "$WORK/store-warm" FLASHWT_SNAPSHOTS=1
    row "warm hit (unchanged tree)" "$i"
    warm_walls="$warm_walls $run_wall"
    echo "  warm hit $i: ${run_wall}s (mode=$st_mode)"
    i=$((i + 1))
done

echo "== cells bump-v1/bump-v2: seed stores..."
run_create "bump-v1-seed" "$WORK/store-bump-v1" FLASHWT_SNAPSHOTS=1
run_create "bump-v2-seed" "$WORK/store-bump-v2" \
    FLASHWT_SNAPSHOTS=1 FLASHWT_SNAPSHOTS_V2=1

v1_walls=""
v2_walls=""
i=1
while [ "$i" -le "$samples" ]; do
    bump_packages "$i"
    run_create "bump-v1-$i" "$WORK/store-bump-v1" FLASHWT_SNAPSHOTS=1
    row "post-bump, v1 full rebuild" "$i"
    v1_walls="$v1_walls $run_wall"
    echo "  bump v1 $i: ${run_wall}s (mode=$st_mode)"

    run_create "bump-v2-$i" "$WORK/store-bump-v2" \
        FLASHWT_SNAPSHOTS=1 FLASHWT_SNAPSHOTS_V2=1
    row "post-bump, v2 incremental" "$i"
    v2_walls="$v2_walls $run_wall"
    echo "  bump v2 $i: ${run_wall}s (mode=$st_mode, linked=$st_v2l)"
    i=$((i + 1))
done

echo "== cell poisoning: seeding clean snapshots, dropping .DS_Store..."
run_create "poison-v1-seed" "$WORK/store-poison-v1" FLASHWT_SNAPSHOTS=1
run_create "poison-v2-seed" "$WORK/store-poison-v2" \
    FLASHWT_SNAPSHOTS=1 FLASHWT_SNAPSHOTS_V2=1
printf 'junk\n' >"$SRC/node_modules/.DS_Store"

run_create "poison-v1" "$WORK/store-poison-v1" FLASHWT_SNAPSHOTS=1
row "poison (.DS_Store), v1 full rebuild" 1
poison_v1_wall=$run_wall
echo "  poison v1: ${run_wall}s (mode=$st_mode)"

run_create "poison-v2" "$WORK/store-poison-v2" \
    FLASHWT_SNAPSHOTS=1 FLASHWT_SNAPSHOTS_V2=1
row "poison (.DS_Store), v2 incremental" 1
poison_v2_wall=$run_wall
echo "  poison v2: ${run_wall}s (mode=$st_mode, linked=$st_v2l)"

echo
echo "## v2 benchmark results"
echo
echo "- Platform: $platform"
echo "- Fixture: Scenario D, $files files across $pkgs packages (generate_tree_d)"
echo "- Bump shape: package.json rewritten in pkg-00000/00001/00002, unique bytes per sample"
echo "- Samples: $samples per cell (poisoning: 1 per gate, inherently single-shot)"
echo "- Correctness: every hydrated tree diff -r'd against the donor; mismatch aborts"
echo
echo "| Cell | Sample | Wall (s) | Snapshot mode | Build link-train (ms) | v2 cloned units | v2 linked files |"
echo "|---|---|---|---|---|---|---|"
cat "$WORK/rows.md"
echo
echo "Wall-time medians:"
echo "- warm hit: $(cell_median "$warm_walls")s"
echo "- post-bump v1: $(cell_median "$v1_walls")s"
echo "- post-bump v2: $(cell_median "$v2_walls")s"
echo "- poisoning v1: ${poison_v1_wall}s / v2: ${poison_v2_wall}s"

