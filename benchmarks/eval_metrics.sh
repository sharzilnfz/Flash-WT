#!/usr/bin/env bash

set -euo pipefail

now() {
    perl -MTime::HiRes=time -e 'printf "%.6f\n", time'
}

elapsed() { # start end -> seconds, 3 decimals
    awk -v a="$1" -v b="$2" 'BEGIN { printf "%.3f", b - a }'
}

elapsed_ms() { # start end -> ms integer
    awk -v a="$1" -v b="$2" 'BEGIN { printf "%.0f", (b - a) * 1000 }'
}

median() { # numbers on argv -> median, 3 decimals
    printf '%s\n' "$@" | sort -g | awk '
        { v[NR] = $1 }
        END {
            if (NR % 2) { printf "%.3f", v[(NR + 1) / 2] }
            else { printf "%.3f", (v[NR / 2] + v[NR / 2 + 1]) / 2 }
        }'
}

median_or_dash() { # possibly-empty number string -> median or "-"
    set -- $1
    if [ "$#" -eq 0 ]; then
        echo "-"
    else
        median "$@"
    fi
}

cell_median() {
    set -- $1
    median "$@"
}

first_lines() { # n
    awk -v n="$1" 'NR <= n { buf = buf (NR > 1 ? "\n" : "") $0 } END { if (NR) printf "%s\n", buf }'
}

sha256_hash() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

stats_to_json() {
    perl -e '
        use strict;
        use warnings;
        use List::Util qw(min max sum);

        my @vals;
        if (@ARGV) {
            @vals = map { 0 + $_ } @ARGV;
        } else {
            while (<STDIN>) {
                chomp;
                push @vals, 0 + $_ if length($_) && $_ =~ /^-?[0-9]+(\.[0-9]+)?$/;
            }
        }

        if (!@vals) {
            print "null";
            exit 0;
        }

        @vals = sort { $a <=> $b } @vals;
        my $n = scalar(@vals);
        my $min = $vals[0];
        my $max = $vals[-1];
        my $sum = sum(@vals);
        my $mean = $sum / $n;

        my $median;
        if ($n % 2 == 1) {
            $median = $vals[int($n / 2)];
        } else {
            $median = ($vals[$n / 2 - 1] + $vals[$n / 2]) / 2.0;
        }

        my $p95_idx = int(0.95 * $n);
        $p95_idx = $n - 1 if $p95_idx >= $n;
        my $p95 = $vals[$p95_idx];

        my $sq_sum = 0;
        for my $v (@vals) {
            $sq_sum += ($v - $mean) ** 2;
        }
        my $stdev = $n > 1 ? sqrt($sq_sum / ($n - 1)) : 0.0;

        my $q1 = $vals[int(0.25 * $n)];
        my $q3 = $vals[int(0.75 * $n)];
        $q3 = $vals[-1] if int(0.75 * $n) >= $n;
        my $iqr = $q3 - $q1;

        printf "{\"count\":%d,\"min\":%.3f,\"max\":%.3f,\"mean\":%.3f,\"median\":%.3f,\"p95\":%.3f,\"stdev\":%.3f,\"iqr\":%.3f}",
            $n, $min, $max, $mean, $median, $p95, $stdev, $iqr;
    ' "$@"
}

parse_stage_log() {
    local logfile=$1
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

system_telemetry_json() {
    local os kernel arch cpus
    os=$(uname -s)
    kernel=$(uname -r)
    arch=$(uname -m)

    if [ "$os" = "Darwin" ]; then
        cpus=$(sysctl -n hw.ncpu 2>/dev/null || echo 1)
    else
        cpus=$(nproc 2>/dev/null || echo 1)
    fi

    printf '{"os":"%s","kernel":"%s","arch":"%s","cpus":%d}' \
        "$os" "$kernel" "$arch" "$cpus"
}

build_scenario_json() {
    local name=$1 phase=$2 files=$3 pkgs=$4 wall_stats=$5 stages=$6 fidelity=$7 disk=$8
    printf '{"scenario":"%s","phase":"%s","files":%d,"packages":%d,"wall_clock_ms":%s,"stages":%s,"fidelity":%s,"disk":%s}' \
        "$name" "$phase" "$files" "$pkgs" "$wall_stats" "$stages" "$fidelity" "$disk"
}

