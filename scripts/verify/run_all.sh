#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
SUITES_DIR="$SCRIPT_DIR/suites"
ARTIFACTS_DIR="${ARTIFACTS_DIR:-$REPO_ROOT/artifacts/verify-flashwt}"

mkdir -p "$ARTIFACTS_DIR"

QUICK=0
RUN_ALL=1
TARGET_SUITE=""
RUNS=1
GITHUB=0

usage() {
    cat << EOF
Flash-WT Master Verification Runner

Usage: $0 [OPTIONS]

Options:
  --quick          Run swift test passes with lightweight fixtures
  --all            Run all 5 verification suites (default)
  --suite <name>   Run a single suite by number or name (e.g. 01, 02, cli, apfs, real, storage, chaos)
  --runs <n>       Benchmark repetition count (default: 1)
  --github         Enable GitHub Actions formatting / step summary
  -h, --help       Show help
EOF
    exit 0
}

while [ $# -gt 0 ]; do
    case "$1" in
        --quick)
            QUICK=1
            shift
            ;;
        --all)
            RUN_ALL=1
            shift
            ;;
        --suite)
            [ $# -ge 2 ] || { echo "Error: --suite requires an argument" >&2; exit 1; }
            TARGET_SUITE="$2"
            RUN_ALL=0
            shift 2
            ;;
        --runs)
            [ $# -ge 2 ] || { echo "Error: --runs requires an integer argument" >&2; exit 1; }
            RUNS="$2"
            shift 2
            ;;
        --github)
            GITHUB=1
            shift
            ;;
        -h|--help)
            usage
            ;;
        *)
            echo "Unknown option: $1" >&2
            usage
            ;;
    esac
done

export QUICK GITHUB

FLASHWT_BIN="${FLASHWT_BIN:-$REPO_ROOT/target/release/flashwt}"
if [ ! -x "$FLASHWT_BIN" ]; then
    echo "run_all: Building release binary..."
    cargo build --release --manifest-path "$REPO_ROOT/Cargo.toml" -p flashwt-cli
    FLASHWT_BIN="$REPO_ROOT/target/release/flashwt"
fi
export FLASHWT_BIN

OS_NAME="$(uname -s)"
OS_KERNEL="$(uname -r)"
ARCH_NAME="$(uname -m)"
CPU_COUNT="$(sysctl -n hw.ncpu 2>/dev/null || nproc 2>/dev/null || echo 1)"
GIT_COMMIT="$(git -C "$REPO_ROOT" rev-parse --short HEAD 2>/dev/null || echo "unknown")"
TIMESTAMP="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

echo "======================================================================"
echo " Flash-WT Master Verification Rig"
echo " Host: $OS_NAME $ARCH_NAME ($CPU_COUNT cores) | Git: $GIT_COMMIT"
echo " Mode: $([ "$QUICK" -eq 1 ] && echo "QUICK" || echo "FULL") | Runs: $RUNS"
echo "======================================================================"

declare -a SUITES_TO_RUN=()

if [ -n "$TARGET_SUITE" ]; then
    case "$TARGET_SUITE" in
        1|01|cli|matrix) SUITES_TO_RUN+=("$SUITES_DIR/01_cli_matrix.sh") ;;
        2|02|apfs|flash) SUITES_TO_RUN+=("$SUITES_DIR/02_flash_apfs.sh") ;;
        3|03|real|repos) SUITES_TO_RUN+=("$SUITES_DIR/03_real_repos.sh") ;;
        4|04|storage|iso|isolation) SUITES_TO_RUN+=("$SUITES_DIR/04_isolation_storage.sh") ;;
        5|05|chaos|resilience) SUITES_TO_RUN+=("$SUITES_DIR/05_chaos_resilience.sh") ;;
        *)
            if [ -f "$SUITES_DIR/$TARGET_SUITE" ]; then
                SUITES_TO_RUN+=("$SUITES_DIR/$TARGET_SUITE")
            elif [ -f "$SUITES_DIR/${TARGET_SUITE}.sh" ]; then
                SUITES_TO_RUN+=("$SUITES_DIR/${TARGET_SUITE}.sh")
            else
                echo "Error: Suite '$TARGET_SUITE' not found" >&2
                exit 1
            fi
            ;;
    esac
else
    SUITES_TO_RUN=(
        "$SUITES_DIR/01_cli_matrix.sh"
        "$SUITES_DIR/02_flash_apfs.sh"
        "$SUITES_DIR/03_real_repos.sh"
        "$SUITES_DIR/04_isolation_storage.sh"
        "$SUITES_DIR/05_chaos_resilience.sh"
    )
fi

RUNNER_START_TIME=$(perl -MTime::HiRes=time -e 'printf "%.6f\n", time')
TOTAL_SUITES_PASSED=0
TOTAL_SUITES_FAILED=0

rm -f "$ARTIFACTS_DIR"/suite_*.json

for suite in "${SUITES_TO_RUN[@]}"; do
    sname="$(basename "$suite")"
    echo ""
    echo ">>> Running $sname..."
    if bash "$suite"; then
        TOTAL_SUITES_PASSED=$((TOTAL_SUITES_PASSED + 1))
    else
        TOTAL_SUITES_FAILED=$((TOTAL_SUITES_FAILED + 1))
        echo ">>> $sname EXITED WITH NON-ZERO STATUS" >&2
    fi
done

RUNNER_END_TIME=$(perl -MTime::HiRes=time -e 'printf "%.6f\n", time')
TOTAL_ELAPSED=$(awk -v a="$RUNNER_START_TIME" -v b="$RUNNER_END_TIME" 'BEGIN { printf "%.3f", b - a }')

RAW_DATA_FILE="$ARTIFACTS_DIR/raw_data.json"
REPORT_MD_FILE="$ARTIFACTS_DIR/REPORT.md"

python3 -c "
import json, glob, os, sys

artifacts_dir = sys.argv[1]
raw_file = sys.argv[2]

suite_files = sorted(glob.glob(os.path.join(artifacts_dir, 'suite_*.json')))
suites = []
total_tests = 0
passed_tests = 0
failed_tests = 0
skipped_tests = 0

for sf in suite_files:
    try:
        with open(sf) as f:
            sdata = json.load(f)
            suites.append(sdata)
            total_tests += sdata.get('total', 0)
            passed_tests += sdata.get('passed', 0)
            failed_tests += sdata.get('failed', 0)
            skipped_tests += sdata.get('skipped', 0)
    except Exception as e:
        sys.stderr.write(f'Error reading {sf}: {e}\n')

overall_status = 'PASS' if int(sys.argv[3]) == 0 and failed_tests == 0 else 'FAIL'

payload = {
    'telemetry': {
        'os': sys.argv[4],
        'kernel': sys.argv[5],
        'arch': sys.argv[6],
        'cpus': int(sys.argv[7]),
        'commit': sys.argv[8],
        'timestamp': sys.argv[9],
        'quick_mode': bool(int(sys.argv[10])),
        'total_duration_seconds': float(sys.argv[11])
    },
    'summary': {
        'overall_status': overall_status,
        'suites_total': len(suites),
        'suites_passed': int(sys.argv[12]),
        'suites_failed': int(sys.argv[3]),
        'tests_total': total_tests,
        'tests_passed': passed_tests,
        'tests_failed': failed_tests,
        'tests_skipped': skipped_tests
    },
    'suites': suites
}

with open(raw_file, 'w') as f:
    json.dump(payload, f, indent=2)

print(f'Successfully compiled raw telemetry into {raw_file}')
" "$ARTIFACTS_DIR" "$RAW_DATA_FILE" "$TOTAL_SUITES_FAILED" "$OS_NAME" "$OS_KERNEL" "$ARCH_NAME" "$CPU_COUNT" "$GIT_COMMIT" "$TIMESTAMP" "$QUICK" "$TOTAL_ELAPSED" "$TOTAL_SUITES_PASSED"

echo "Invoking report generator..."
"$SCRIPT_DIR/generate_report.sh" "$RAW_DATA_FILE" "$REPORT_MD_FILE"

if [ "$GITHUB" -eq 1 ] && [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
    cat "$REPORT_MD_FILE" >> "$GITHUB_STEP_SUMMARY"
fi

echo ""
echo "======================================================================"
echo " Verification Complete: $TOTAL_SUITES_PASSED Passed, $TOTAL_SUITES_FAILED Failed (${TOTAL_ELAPSED}s)"
echo " Report generated at: $REPORT_MD_FILE"
echo "======================================================================"

if [ "$TOTAL_SUITES_FAILED" -gt 0 ]; then
    exit 1
fi
exit 0

