#!/usr/bin/env bash

set -euo pipefail

volume_free_bytes() {
    local target_dir=$1
    if [ ! -d "$target_dir" ]; then
        target_dir=$(dirname "$target_dir")
    fi

    df -k "$target_dir" | awk 'NR > 1 { if ($4 ~ /^[0-9]+$/) { printf "%.0f\n", $4 * 1024; exit } else if ($5 ~ /^[0-9]+$/) { printf "%.0f\n", $5 * 1024; exit } }'
}

tree_disk_usage() {
    local dir=$1
    [ -d "$dir" ] || { echo "0 0"; return; }

    local stat_args
    case "$(uname -s)" in
        Darwin) stat_args=(-f '%z %b') ;;
        *) stat_args=(-c '%s %b') ;;
    esac

    find "$dir" -type f -exec stat "${stat_args[@]}" {} + 2>/dev/null | awk '
        { app += $1; alloc += $2 * 512 }
        END { printf "%.0f %.0f\n", (app ? app : 0), (alloc ? alloc : 0) }'
}

disk_usage() { # tree -> "<apparent_bytes> <allocated_bytes>" on stdout
    tree_disk_usage "$@"
}

storage_to_json() {
    local app=0 alloc=0 vol_consumed=0 dedup_ratio=1.0
    local raw="$*"
    if [ -n "$raw" ]; then
        set -- $raw
        app=${1:-0}
        alloc=${2:-0}
        vol_consumed=${3:-0}
        dedup_ratio=${4:-1.0}
    fi

    printf '{"apparent_bytes":%.0f,"allocated_bytes":%.0f,"volume_consumed_bytes":%.0f,"dedup_ratio":%.2f}' \
        "$app" "$alloc" "$vol_consumed" "$dedup_ratio"
}

