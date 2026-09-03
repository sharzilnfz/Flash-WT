#!/usr/bin/env bash
# Reproducible harness for the v2 incremental-rebuild numbers
# (commit 9f1f0e6, "v2: diff-based incremental snapshot rebuilds behind
# WT_SNAPSHOTS_V2=1"). This is the committed version of the ad-hoc
# script used to measure those numbers; one command regenerates the
# table on any macOS/APFS machine.
#
# Builds a scratch repo with a Scenario-D fixture (benchmarks/fixture.sh,
# generate_tree_d), then times four cells of `wt create` against it:
#
#   warm hit     store already holds a published snapshot of an
#                UNCHANGED tree; WT_SNAPSHOTS=1 only. The number every
#                rebuild cell should stay well above.
#   bump, v1     3 package files rewritten uniquely per sample, then
#                create with WT_SNAPSHOTS=1 but NOT WT_SNAPSHOTS_V2:
#                the pre-v2 full rebuild (snapshot-mode=build).
#   bump, v2     same bump shape, separate store, both gates on:
#                the incremental rebuild (snapshot-mode=v2).
#   poisoning    a single junk file (.DS_Store) at the heavy-directory
#                root after a clean published snapshot, v1 vs v2.
#                Inherently single-shot per gate: once the poisoned
#                rebuild lands, later creates are hits and say nothing
#                about poisoning.
#
# Every timed create runs with WT_TIMING=1; the `wt-stage` lines are
# parsed out of its log and reported as wall time plus snapshot-mode,
# snapshot-build-link-train, snapshot-v2-cloned, and snapshot-v2-linked.
#
# CORRECTNESS GATE: after EVERY create (timed or not) the hydrated tree
# is diff -r'd against the donor tree and any mismatch aborts the run.
# A benchmark that hydrates wrong is worthless, so this is fatal, not a
# reported column.
#
# Usage:
#   ./benchmarks/v2-bench.sh
#
# Environment:
#   WT_BENCH_FILES    Scenario-D fixture size (default 40000 -> 800
#                     packages). Keep divisible by 50.
#   WT_BENCH_SAMPLES  timed samples per cell (default 3). Poisoning is
#                     always a single sample per gate (see above).
#   WT_BIN            reuse a prebuilt binary instead of building.
#
# macOS-only: the snapshot fast path itself is macOS/APFS-only, so
# running this anywhere else would measure fallback hydration and print
# meaningless mode columns; the script refuses instead.

set -euo pipefail

BENCH_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$BENCH_DIR/.." && pwd)"
# shellcheck disable=SC1091  # sourced at runtime, same directory
. "$BENCH_DIR/fixture.sh"
# shellcheck disable=SC1091
. "$BENCH_DIR/eval_metrics.sh"

die() {
    echo "benchmarks: $*" >&2
    exit 1
}

FILES_DEFAULT=40000
SAMPLES_DEFAULT=3

files=${WT_BENCH_FILES:-$FILES_DEFAULT}
samples=${WT_BENCH_SAMPLES:-$SAMPLES_DEFAULT}

case $files in
    '' | *[!0-9]*) die "WT_BENCH_FILES must be a positive integer" ;;
esac
[ "$files" -ge 1 ] || die "WT_BENCH_FILES must be >= 1"
case $samples in
    '' | *[!0-9]*) die "WT_BENCH_SAMPLES must be a positive integer" ;;
esac
[ "$samples" -ge 1 ] || die "WT_BENCH_SAMPLES must be >= 1"

if [ "$(uname -s)" != Darwin ]; then
    die "this harness measures macOS/APFS-only features; run it on a Mac"
fi

BIN=${WT_BIN:-}
if [ -z "$BIN" ]; then
    if [ -x "$REPO_ROOT/target/release/flashwt" ]; then
        BIN="$REPO_ROOT/target/release/flashwt"
    else
        BIN="$REPO_ROOT/target/release/wt"
    fi
fi
if [ ! -x "$BIN" ]; then
    echo "building release binary..."
    cargo build --release --quiet \
        --manifest-path "$REPO_ROOT/Cargo.toml" -p wt-cli
    if [ -x "$REPO_ROOT/target/release/flashwt" ]; then
        BIN="$REPO_ROOT/target/release/flashwt"
    else
        BIN="$REPO_ROOT/target/release/wt"
    fi
fi
[ -x "$BIN" ] || die "binary not runnable at $BIN"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/wt-v2-bench.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

drop_worktree() {
    git -C "$SRC" worktree remove --force "$1" >/dev/null 2>&1 ||
        rm -rf "$1"
}

# ---------------------------------------------------------------------------
# Correctness gate. Runs after every create; fixtures are generated fresh
# in this process, so a plain recursive diff is the whole contract.
# ---------------------------------------------------------------------------
verify_tree() { # donor dest label
    local raw
    raw=$(diff -r "$1" "$2") || true
    [ -z "$raw" ] ||
        die "hydrated tree under $2 does not match donor $1 ($3):
$(printf '%s\n' "$raw" | first_lines 10)"
}

# ---------------------------------------------------------------------------
# Stage-timing capture: scan a wt create log for lines shaped exactly
# `wt-stage <name>=<value>`, keep what this harness reports. Globals set
# per call; "-" means the binary printed no such line.
# ---------------------------------------------------------------------------
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

# One wt create: timed, logged, stage-parsed, verified against the
# donor. Sets run_wall (seconds) and the st_* globals.
run_create() { # name store env-pairs...
    local name=$1 store=$2
    shift 2
    dest="$WORK/wt-$name"
    local t0 t1
    t0=$(now)
    (
        cd "$SRC" &&
            env WT_STORE="$store" WT_TIMING=1 "$@" \
                "$BIN" create "$name" --dir "$dest" >"$WORK/$name.log" 2>&1
    ) || {
        cat "$WORK/$name.log" >&2
        die "wt create $name failed"
    }
    t1=$(now)
    parse_stages "$WORK/$name.log"
    verify_tree "$SRC/node_modules" "$dest/node_modules" "$name"
    drop_worktree "$dest"
    run_wall=$(elapsed "$t0" "$t1")
}

# One markdown results row, appended to the table body.
row() { # cell sample
    printf '| %s | %s | %s | %s | %s | %s | %s |\n' \
        "$1" "$2" "$run_wall" "$st_mode" "$st_blt" "$st_v2c" "$st_v2l" \
        >>"$WORK/rows.md"
}

# Rewrite package.json in three fixed packages with sample-unique
# bytes, so every sample's rebuild sees real content changes in the
# manifest units that matter.
bump_packages() { # sample-index
    local k p
    for k in 0 1 2; do
        p="$SRC/node_modules/pkg-$(printf '%05d' "$k")"
        [ -f "$p/package.json" ] || die "fixture package missing: $p"
        printf '{"name":"pkg-%05d","version":"2.0.%s","bump":"sample-%s"}\n' \
            "$k" "$1" "$1" >"$p/package.json"
    done
}

# ---------------------------------------------------------------------------
# Fixture repository with the Scenario-D heavy tree.
# ---------------------------------------------------------------------------
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
printf 'node_modules/\n' >"$SRC/.wtinclude"
git -C "$SRC" add .
git -C "$SRC" commit -qm init

generate_tree_d "$SRC/node_modules" "$files"
count=$(count_files "$SRC/node_modules")
[ "$count" -eq "$files" ] ||
    die "fixture generator produced $count files, expected $files"

touch "$WORK/rows.md"

# ---------------------------------------------------------------------------
# Cell 1: warm snapshot hit on an unchanged tree.
# ---------------------------------------------------------------------------
echo "== cell warm-hit: populate + x$samples timed creates..."
run_create "warm-seed" "$WORK/store-warm" WT_SNAPSHOTS=1
warm_walls=""
i=1
while [ "$i" -le "$samples" ]; do
    run_create "warm-$i" "$WORK/store-warm" WT_SNAPSHOTS=1
    row "warm hit (unchanged tree)" "$i"
    warm_walls="$warm_walls $run_wall"
    echo "  warm hit $i: ${run_wall}s (mode=$st_mode)"
    i=$((i + 1))
done

# ---------------------------------------------------------------------------
# Cells 2+3: post-bump rebuild, v1 full vs v2 incremental. Both stores
# are seeded from the same donor and see the exact same bump sequence,
# which is what makes the two columns comparable.
# ---------------------------------------------------------------------------
echo "== cells bump-v1/bump-v2: seed stores..."
run_create "bump-v1-seed" "$WORK/store-bump-v1" WT_SNAPSHOTS=1
run_create "bump-v2-seed" "$WORK/store-bump-v2" \
    WT_SNAPSHOTS=1 WT_SNAPSHOTS_V2=1

v1_walls=""
v2_walls=""
i=1
while [ "$i" -le "$samples" ]; do
    bump_packages "$i"
    run_create "bump-v1-$i" "$WORK/store-bump-v1" WT_SNAPSHOTS=1
    row "post-bump, v1 full rebuild" "$i"
    v1_walls="$v1_walls $run_wall"
    echo "  bump v1 $i: ${run_wall}s (mode=$st_mode)"

    run_create "bump-v2-$i" "$WORK/store-bump-v2" \
        WT_SNAPSHOTS=1 WT_SNAPSHOTS_V2=1
    row "post-bump, v2 incremental" "$i"
    v2_walls="$v2_walls $run_wall"
    echo "  bump v2 $i: ${run_wall}s (mode=$st_mode, linked=$st_v2l)"
    i=$((i + 1))
done

# ---------------------------------------------------------------------------
# Cell 4: single junk file (.DS_Store at the heavy-dir root) after a
# clean published snapshot. Single-shot per gate: the poisoned rebuild
# IS the measurement.
# ---------------------------------------------------------------------------
echo "== cell poisoning: seeding clean snapshots, dropping .DS_Store..."
run_create "poison-v1-seed" "$WORK/store-poison-v1" WT_SNAPSHOTS=1
run_create "poison-v2-seed" "$WORK/store-poison-v2" \
    WT_SNAPSHOTS=1 WT_SNAPSHOTS_V2=1
printf 'junk\n' >"$SRC/node_modules/.DS_Store"

run_create "poison-v1" "$WORK/store-poison-v1" WT_SNAPSHOTS=1
row "poison (.DS_Store), v1 full rebuild" 1
poison_v1_wall=$run_wall
echo "  poison v1: ${run_wall}s (mode=$st_mode)"

run_create "poison-v2" "$WORK/store-poison-v2" \
    WT_SNAPSHOTS=1 WT_SNAPSHOTS_V2=1
row "poison (.DS_Store), v2 incremental" 1
poison_v2_wall=$run_wall
echo "  poison v2: ${run_wall}s (mode=$st_mode, linked=$st_v2l)"

# ---------------------------------------------------------------------------
# Results.
# ---------------------------------------------------------------------------
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
