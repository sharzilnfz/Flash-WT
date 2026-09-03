#!/usr/bin/env bash

if [ -n "${BASH_VERSION:-}" ]; then
    set -euo pipefail
else
    set -eu
fi

HARNESS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$HARNESS_DIR/../.." && pwd)"
VERIFY_DIR="$REPO_ROOT/scripts/verify"
SUITES_DIR="$VERIFY_DIR/suites"
ARTIFACTS_DIR="${ARTIFACTS_DIR:-$REPO_ROOT/artifacts/verify-flashwt}"

mkdir -p "$ARTIFACTS_DIR"

resolve_flashwt_bin() {
    if [ -n "${FLASHWT_BIN:-}" ] && [ -x "$FLASHWT_BIN" ]; then
        FLASHWT_BIN="$(cd "$(dirname "$FLASHWT_BIN")" && pwd)/$(basename "$FLASHWT_BIN")"
        export FLASHWT_BIN
        return 0
    fi

    local candidates=(
        "$REPO_ROOT/target/release/flashwt"
        "$REPO_ROOT/target/release/flashwt"
        "$REPO_ROOT/target/debug/flashwt"
        "$REPO_ROOT/target/debug/flashwt"
    )

    for bin in "${candidates[@]}"; do
        if [ -x "$bin" ]; then
            FLASHWT_BIN="$(cd "$(dirname "$bin")" && pwd)/$(basename "$bin")"
            export FLASHWT_BIN
            return 0
        fi
    done

    if command -v flashwt >/dev/null 2>&1; then
        FLASHWT_BIN="$(command -v flashwt)"
        export FLASHWT_BIN
        return 0
    elif command -v flashwt >/dev/null 2>&1; then
        FLASHWT_BIN="$(command -v flashwt)"
        export FLASHWT_BIN
        return 0
    fi

    echo "harness: building release binary..." >&2
    cargo build --release --manifest-path "$REPO_ROOT/Cargo.toml" -p flashwt-cli >&2
    if [ -x "$REPO_ROOT/target/release/flashwt" ]; then
        FLASHWT_BIN="$REPO_ROOT/target/release/flashwt"
    else
        FLASHWT_BIN="$REPO_ROOT/target/release/flashwt"
    fi
    export FLASHWT_BIN
    return 0
}

resolve_flashwt_bin


flashwt() {
    FLASHWT_STORE="${FLASHWT_STORE:-${FIXTURE_STORE:-}}" \
    FLASHWT_TIMING="${FLASHWT_TIMING:-}" \
    FLASHWT_SNAPSHOTS="${FLASHWT_SNAPSHOTS:-}" \
    FLASHWT_SNAPSHOTS_V2="${FLASHWT_SNAPSHOTS_V2:-}" \
    FLASHWT_VERIFY="${FLASHWT_VERIFY:-}" \
    "$FLASHWT_BIN" "$@"
}

flashwt_json() {
    flashwt --json "$@"
}

FLASHWT_ENV_ROOT=""
FLASHWT_ORIGIN=""
FLASHWT_STORE=""
FLASHWT_WORKTREES=""
FLASHWT_TMP=""
ACTIVE_FIXTURES=()

_harness_cleanup_trap() {
    local sig="${1:-EXIT}"
    local rc=$?
    trap "" EXIT INT TERM HUP
    teardown_env
    if [ "$sig" != "EXIT" ]; then
        kill -s "$sig" "$$" 2>/dev/null || exit 1
    else
        exit "$rc"
    fi
}

setup_isolated_env() {
    local name="${1:-test}"
    local fixture_root="${FLASHWT_FIXTURE_ROOT:-${TMPDIR:-/tmp}/flashflashwt-verify}"
    mkdir -p "$fixture_root"

    local env_dir
    env_dir="$(mktemp -d "$fixture_root/flashwt-test-${name}-XXXXXX")"

    FLASHWT_ENV_ROOT="$env_dir"
    FLASHWT_ORIGIN="$env_dir/origin"
    FLASHWT_STORE="$env_dir/store"
    FLASHWT_WORKTREES="$env_dir/worktrees"
    FLASHWT_TMP="$env_dir/tmp"

    mkdir -p "$FLASHWT_ORIGIN" "$FLASHWT_STORE" "$FLASHWT_WORKTREES" "$FLASHWT_TMP"

    export FLASHWT_ENV_ROOT FLASHWT_ORIGIN FLASHWT_STORE FLASHWT_WORKTREES FLASHWT_TMP

    FIXTURE_DIR="$env_dir"
    FIXTURE_ORIGIN="$FLASHWT_ORIGIN"
    FIXTURE_STORE="$FLASHWT_STORE"
    export FIXTURE_DIR FIXTURE_ORIGIN FIXTURE_STORE

    git -C "$FLASHWT_ORIGIN" init --quiet
    git -C "$FLASHWT_ORIGIN" config user.email "verify@example.com"
    git -C "$FLASHWT_ORIGIN" config user.name "Flash-WT Verify"
    git -C "$FLASHWT_ORIGIN" config commit.gpgSign false

    printf "# Fixture: %s\n" "$name" > "$FLASHWT_ORIGIN/README.md"
    printf ".DS_Store\n" > "$FLASHWT_ORIGIN/.gitignore"
    git -C "$FLASHWT_ORIGIN" add .
    git -C "$FLASHWT_ORIGIN" commit --quiet -m "Initial commit for $name fixture"

    ACTIVE_FIXTURES+=("$env_dir")

    trap "_harness_cleanup_trap EXIT" EXIT
    trap "_harness_cleanup_trap INT" INT
    trap "_harness_cleanup_trap TERM" TERM
    trap "_harness_cleanup_trap HUP" HUP

    return 0
}

setup_isolated_fixture() {
    setup_isolated_env "$@"
}

teardown_env() {
    local target="${1:-${FLASHWT_ENV_ROOT:-}}"
    if [ -z "$target" ] && [ ${#ACTIVE_FIXTURES[@]} -eq 0 ]; then
        return 0
    fi

    if [ "${FLASHWT_KEEP_FIXTURE:-0}" = "1" ] || [ "${FLASHWT_PRESERVE:-0}" = "1" ]; then
        echo "[harness] Preserving fixture directories for debugging" >&2
        return 0
    fi

    local targets_to_clean=()
    if [ -n "$target" ] && [ -d "$target" ]; then
        targets_to_clean+=("$target")
    fi
    for f in "${ACTIVE_FIXTURES[@]}"; do
        if [ -d "$f" ] && [ "$f" != "$target" ]; then
            targets_to_clean+=("$f")
        fi
    done

    for env_root in "${targets_to_clean[@]}"; do
        case "$(uname -s)" in
            Darwin)
                mount | awk -v root="$env_root" '
                    $0 ~ root {
                        for (i = 1; i <= NF; i++) {
                            if ($i == "on") {
                                print $(i + 1);
                                break;
                            }
                        }
                    }' | sort -r | while IFS= read -r mnt; do
                    if [ -n "$mnt" ] && [ -d "$mnt" ]; then
                        diskutil unmount force "$mnt" >/dev/null 2>&1 || umount -f "$mnt" >/dev/null 2>&1 || true
                    fi
                done
                ;;
            Linux)
                if command -v findmnt >/dev/null 2>&1; then
                    findmnt -l -n -o TARGET 2>/dev/null | grep "^$env_root" | sort -r | while IFS= read -r mnt; do
                        if [ -n "$mnt" ]; then
                            umount -l "$mnt" >/dev/null 2>&1 || umount -f "$mnt" >/dev/null 2>&1 || true
                        fi
                    done
                fi
                ;;
        esac

        if [ -d "$env_root/origin/.git" ]; then
            git -C "$env_root/origin" worktree prune >/dev/null 2>&1 || true
        fi

        rm -rf "$env_root" 2>/dev/null || true
    done

    ACTIVE_FIXTURES=()
    FLASHWT_ENV_ROOT=""
    FIXTURE_DIR=""
    return 0
}

teardown_isolated_fixture() {
    teardown_env "$@"
}

sha256_hash() {
    local file_path="$1"
    if command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$file_path" | awk '{print $1}'
    elif command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$file_path" | awk '{print $1}'
    else
        python3 -c 'import hashlib, sys; print(hashlib.sha256(open(sys.argv[1], "rb").read()).hexdigest())' "$file_path"
    fi
}

volume_free_bytes() {
    local target_dir="${1:-.}"
    if [ ! -e "$target_dir" ]; then
        target_dir="$(dirname "$target_dir")"
    fi
    if [ ! -e "$target_dir" ]; then
        target_dir="."
    fi

    df -k "$target_dir" 2>/dev/null | awk 'NR > 1 {
        if ($4 ~ /^[0-9]+$/) {
            printf "%.0f\n", $4 * 1024;
            exit;
        } else if ($5 ~ /^[0-9]+$/) {
            printf "%.0f\n", $5 * 1024;
            exit;
        }
    }'
}

tree_disk_usage() {
    local dir="$1"
    [ -d "$dir" ] || { echo "0 0"; return 0; }

    local stat_args
    case "$(uname -s)" in
        Darwin) stat_args=(-f '%z %b') ;;
        *) stat_args=(-c '%s %b') ;;
    esac

    find "$dir" -type f -exec stat "${stat_args[@]}" {} + 2>/dev/null | awk '
        { app += $1; alloc += $2 * 512 }
        END { printf "%.0f %.0f\n", (app ? app : 0), (alloc ? alloc : 0) }'
}

storage_to_json() {
    local app=0 alloc=0 vol_consumed=0 dedup_ratio=1.0

    if [ $# -eq 1 ] && [ -d "$1" ]; then
        read -r app alloc <<< "$(tree_disk_usage "$1")"
        if awk -v a="$alloc" 'BEGIN { exit !(a > 0) }'; then
            dedup_ratio=$(awk -v ap="$app" -v al="$alloc" 'BEGIN { printf "%.2f", ap / al }')
        fi
    elif [ $# -gt 0 ]; then
        app=${1:-0}
        alloc=${2:-0}
        vol_consumed=${3:-0}
        dedup_ratio=${4:-1.0}
    fi

    printf '{"apparent_bytes":%.0f,"allocated_bytes":%.0f,"volume_consumed_bytes":%.0f,"dedup_ratio":%.2f}\n' \
        "$app" "$alloc" "$vol_consumed" "$dedup_ratio"
}

json_val() {
    local json_input="$1"
    local key="$2"

    if [ -f "$json_input" ]; then
        json_input="$(cat "$json_input")"
    fi

    if command -v python3 >/dev/null 2>&1; then
        python3 -c '
import sys, json

raw = sys.argv[1]
key = sys.argv[2]
try:
    data = json.loads(raw)
    k = key.replace("[", ".").replace("]", "").replace("'\''", "").replace("\"", "")
    if k.startswith("."):
        k = k[1:]
    for part in k.split("."):
        if not part:
            continue
        if isinstance(data, dict):
            data = data.get(part)
        elif isinstance(data, list) and part.isdigit():
            idx = int(part)
            data = data[idx] if 0 <= idx < len(data) else None
        else:
            data = None
            break
    if data is None:
        sys.exit(1)
    if isinstance(data, (dict, list)):
        print(json.dumps(data))
    elif isinstance(data, bool):
        print(str(data))
    else:
        print(data)
except Exception:
    sys.exit(1)
' "$json_input" "$key" 2>/dev/null || true
    else
        printf "%s" "$json_input" | awk -v k="$key" '
        BEGIN { RS="[,{}\\[\\]]"; FS=":" }
        {
            gsub(/^[ \t\n\r"'"'"']+|[ \t\n\r"'"'"']+$/, "", $1);
            if ($1 == k) {
                sub(/^[ \t\r\n]+/, "", $2);
                gsub(/^"|"$/, "", $2);
                print $2;
                exit;
            }
        }'
    fi
}

assert_status_ok() {
    local envelope="$1"
    local msg="${2:-Envelope status must be ok}"
    local raw=""
    if [ -f "$envelope" ]; then
        raw="$(cat "$envelope")"
    else
        raw="$envelope"
    fi

    local status
    status="$(json_val "$raw" "status")"
    if [ "$status" != "ok" ]; then
        echo "ASSERTION FAILED: $msg (expected status 'ok', got '$status')" >&2
        local diag
        diag="$(json_val "$raw" "diagnostics")"
        if [ -n "$diag" ] && [ "$diag" != "[]" ]; then
            echo "  diagnostics: $diag" >&2
        fi
        echo "  raw envelope: $raw" >&2
        return 1
    fi
    return 0
}

assert_json_ok() {
    assert_status_ok "$@"
}

assert_eq() {
    local actual="$1"
    local expected="$2"
    local msg="${3:-assertion failed: expected '$expected' but got '$actual'}"

    if [ "$actual" != "$expected" ]; then
        echo "ASSERTION FAILED: $msg" >&2
        echo "  expected: '$expected'" >&2
        echo "  actual:   '$actual'" >&2
        return 1
    fi
    return 0
}

assert_json_val() {
    local json_input="$1"
    local key="$2"
    local expected="$3"
    local msg="${4:-JSON value mismatch}"

    local actual
    actual="$(json_val "$json_input" "$key")"
    if [ "$actual" != "$expected" ]; then
        echo "ASSERTION FAILED: $msg (key: $key: expected '$expected', got '$actual')" >&2
        return 1
    fi
    return 0
}

assert_file_exists() {
    local path="$1"
    local msg="${2:-File must exist: $path}"
    if [ ! -e "$path" ]; then
        echo "ASSERTION FAILED: $msg" >&2
        return 1
    fi
    return 0
}

assert_file_not_exists() {
    local path="$1"
    local msg="${2:-File must not exist: $path}"
    if [ -e "$path" ]; then
        echo "ASSERTION FAILED: $msg" >&2
        return 1
    fi
    return 0
}

assert_file_content() {
    local path="$1"
    local expected="$2"
    local msg="${3:-File content mismatch in $path}"
    if [ ! -f "$path" ]; then
        echo "ASSERTION FAILED: $msg (file not found)" >&2
        return 1
    fi
    local actual
    actual="$(cat "$path")"
    assert_eq "$actual" "$expected" "$msg"
}

assert_symlink() {
    local link_path="$1"
    local expected_target="$2"
    local msg="${3:-Symlink invalid: $link_path}"

    if [ ! -L "$link_path" ]; then
        echo "ASSERTION FAILED: $msg ($link_path is not a symlink)" >&2
        return 1
    fi

    local actual_target
    actual_target=$(readlink "$link_path")
    assert_eq "$actual_target" "$expected_target" "$msg"
}

assert_mode() {
    local file_path="$1"
    local expected_mode="$2"
    local msg="${3:-Mode mismatch on $file_path}"

    local actual_mode
    case "$(uname -s)" in
        Darwin) actual_mode=$(stat -f '%Lp' "$file_path" 2>/dev/null) ;;
        *) actual_mode=$(stat -c '%a' "$file_path" 2>/dev/null) ;;
    esac

    expected_mode="${expected_mode#0}"
    actual_mode="${actual_mode#0}"
    assert_eq "$actual_mode" "$expected_mode" "$msg"
}

assert_triple_axis() {
    local src="$1"
    local dst="$2"
    local msg="${3:-Triple-axis parity verification}"

    [ -d "$src" ] || { echo "assert_triple_axis: src $src is not a directory" >&2; return 1; }
    [ -d "$dst" ] || { echo "assert_triple_axis: dst $dst is not a directory" >&2; return 1; }

    python3 -c '
import os, sys, stat, hashlib

src_root = os.path.abspath(sys.argv[1])
dst_root = os.path.abspath(sys.argv[2])

def get_tree_info(root):
    tree = {}
    for dirpath, dirnames, filenames in os.walk(root, followlinks=False):
        rel_dir = os.path.relpath(dirpath, root)
        if rel_dir == ".":
            rel_dir = ""
        if rel_dir:
            tree[rel_dir] = {"type": "dir"}
        for fn in filenames:
            full = os.path.join(dirpath, fn)
            rel = os.path.join(rel_dir, fn) if rel_dir else fn
            lstat = os.lstat(full)
            mode = stat.S_IMODE(lstat.st_mode)
            if stat.S_ISLNK(lstat.st_mode):
                tree[rel] = {"type": "link", "target": os.readlink(full), "mode": mode}
            elif stat.S_ISREG(lstat.st_mode):
                h = hashlib.sha256()
                with open(full, "rb") as f:
                    while chunk := f.read(65536):
                        h.update(chunk)
                tree[rel] = {"type": "file", "sha256": h.hexdigest(), "mode": mode, "size": lstat.st_size}
            else:
                tree[rel] = {"type": "other"}
    return tree

src_tree = get_tree_info(src_root)
dst_tree = get_tree_info(dst_root)

src_keys = set(src_tree.keys())
dst_keys = set(dst_tree.keys())

missing = src_keys - dst_keys
if missing:
    sys.stderr.write(f"Triple-axis error: missing {len(missing)} items, e.g. {list(missing)[:5]}\n")
    sys.exit(1)

extra = dst_keys - src_keys
if extra:
    sys.stderr.write(f"Triple-axis error: extra {len(extra)} items, e.g. {list(extra)[:5]}\n")
    sys.exit(1)

for rel, s_meta in src_tree.items():
    d_meta = dst_tree[rel]
    st = s_meta["type"]
    dt = d_meta["type"]
    if st != dt:
        sys.stderr.write(f"Type mismatch on {rel}: {st} vs {dt}\n")
        sys.exit(1)
    if st == "file":
        if s_meta["sha256"] != d_meta["sha256"]:
            sys.stderr.write(f"SHA256 mismatch on {rel}\n")
            sys.exit(1)
        if s_meta["mode"] != d_meta["mode"]:
            sm = oct(s_meta["mode"])
            dm = oct(d_meta["mode"])
            sys.stderr.write(f"Permission mismatch on {rel}: {sm} vs {dm}\n")
            sys.exit(1)
    elif st == "link":
        if s_meta["target"] != d_meta["target"]:
            stgt = s_meta["target"]
            dtgt = d_meta["target"]
            sys.stderr.write(f"Symlink mismatch on {rel}: {stgt} vs {dtgt}\n")
            sys.exit(1)
sys.exit(0)
' "$src" "$dst" || {
        echo "ASSERTION FAILED: $msg (source: $src, dest: $dst)" >&2
        return 1
    }
    return 0
}

now() {
    perl -MTime::HiRes=time -e 'printf "%.6f\n", time' 2>/dev/null || python3 -c 'import time; print(f"{time.time():.6f}")'
}

elapsed() {
    awk -v a="$1" -v b="$2" 'BEGIN { printf "%.3f", b - a }'
}

elapsed_ms() {
    awk -v a="$1" -v b="$2" 'BEGIN { printf "%.0f", (b - a) * 1000 }'
}

parse_stage_log() {
    local logfile="$1"
    [ -f "$logfile" ] || return 0
    awk '
        /^flashwt-stage / {
            sub(/^flashwt-stage /, "", $0)
            split($0, kv, "=")
            if (length(kv[1]) > 0 && length(kv[2]) > 0) {
                print kv[1] "=" kv[2]
            }
        }
    ' "$logfile"
}

SUITE_ID=""
SUITE_TITLE=""
SUITE_START_TIME=0
TEST_COUNT=0
PASS_COUNT=0
FAIL_COUNT=0
SKIP_COUNT=0
TEST_RESULTS_JSON="[]"

suite_init() {
    SUITE_ID="$1"
    SUITE_TITLE="$2"
    SUITE_START_TIME=$(now)
    TEST_COUNT=0
    PASS_COUNT=0
    FAIL_COUNT=0
    SKIP_COUNT=0
    TEST_RESULTS_JSON="[]"

    echo ""
    echo "======================================================================"
    echo " Suite: [${SUITE_ID}] ${SUITE_TITLE}"
    echo "======================================================================"
}

test_start() {
    local test_id="$1"
    local desc="$2"
    echo "▶ RUN: [${test_id}] ${desc}"
}

test_pass() {
    local test_id="$1"
    local arg2="${2:-}"
    local arg3="${3:-}"
    local desc=""
    local extra_json="{}"

    if [ -n "$arg3" ]; then
        desc="$arg2"
        extra_json="$arg3"
    elif [[ "$arg2" =~ ^\{.*\}$ ]]; then
        desc=""
        extra_json="$arg2"
    else
        desc="$arg2"
        extra_json="{}"
    fi

    PASS_COUNT=$((PASS_COUNT + 1))
    TEST_COUNT=$((TEST_COUNT + 1))

    if [ -n "$desc" ]; then
        echo "  ✓ PASS: [${test_id}] ${desc}"
    elif [ "$extra_json" != "{}" ]; then
        echo "  ✓ PASS: [${test_id}] ${extra_json}"
    else
        echo "  ✓ PASS: [${test_id}]"
    fi

    TEST_RESULTS_JSON=$(python3 -c '
import json, sys
arr = json.loads(sys.argv[1])
try:
    extra = json.loads(sys.argv[2])
except Exception:
    extra = {}
arr.append({"id": sys.argv[3], "description": sys.argv[4], "status": "pass", "metrics": extra})
print(json.dumps(arr))
' "$TEST_RESULTS_JSON" "$extra_json" "$test_id" "$desc")
}

test_fail() {
    local test_id="$1"
    local desc="$2"
    local reason="${3:-unknown failure}"

    FAIL_COUNT=$((FAIL_COUNT + 1))
    TEST_COUNT=$((TEST_COUNT + 1))

    echo "  ✗ FAIL: [${test_id}] ${desc} — ${reason}"

    TEST_RESULTS_JSON=$(python3 -c '
import json, sys
arr = json.loads(sys.argv[1])
arr.append({"id": sys.argv[2], "description": sys.argv[3], "status": "fail", "error": sys.argv[4]})
print(json.dumps(arr))
' "$TEST_RESULTS_JSON" "$test_id" "$desc" "$reason")
}

test_skip() {
    local test_id="$1"
    local desc="$2"
    local reason="${3:-skipped}"

    SKIP_COUNT=$((SKIP_COUNT + 1))
    TEST_COUNT=$((TEST_COUNT + 1))

    echo "  ⊘ SKIP: [${test_id}] ${desc} — ${reason}"

    TEST_RESULTS_JSON=$(python3 -c '
import json, sys
arr = json.loads(sys.argv[1])
arr.append({"id": sys.argv[2], "description": sys.argv[3], "status": "skip", "reason": sys.argv[4]})
print(json.dumps(arr))
' "$TEST_RESULTS_JSON" "$test_id" "$desc" "$reason")
}

suite_finish() {
    local end_time
    end_time=$(now)
    local suite_duration
    suite_duration=$(elapsed "$SUITE_START_TIME" "$end_time")

    local status="PASS"
    if [ "$FAIL_COUNT" -gt 0 ]; then
        status="FAIL"
    fi

    echo ""
    echo "--- Suite [${SUITE_ID}] Summary ---"
    echo "Total: $TEST_COUNT | Passed: $PASS_COUNT | Failed: $FAIL_COUNT | Skipped: $SKIP_COUNT | Duration: ${suite_duration}s"

    local suite_out_file="$ARTIFACTS_DIR/suite_${SUITE_ID}.json"
    python3 -c '
import json, sys
result = {
    "suite_id": sys.argv[1],
    "suite_title": sys.argv[2],
    "status": sys.argv[3],
    "total": int(sys.argv[4]),
    "passed": int(sys.argv[5]),
    "failed": int(sys.argv[6]),
    "skipped": int(sys.argv[7]),
    "duration_seconds": float(sys.argv[8]),
    "tests": json.loads(sys.argv[9])
}
with open(sys.argv[10], "w") as f:
    json.dump(result, f, indent=2)
' "$SUITE_ID" "$SUITE_TITLE" "$status" "$TEST_COUNT" "$PASS_COUNT" "$FAIL_COUNT" "$SKIP_COUNT" "$suite_duration" "$TEST_RESULTS_JSON" "$suite_out_file"

    if [ "$FAIL_COUNT" -gt 0 ]; then
        return 1
    fi
    return 0
}

