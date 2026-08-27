#!/usr/bin/env bash
# eval_metrics.sh — JSON metrics schema, stage log parser, and statistics accumulator.
# Sourced by eval harnesses or executed directly to parse and aggregate test logs.

set -euo pipefail

# Compute comprehensive statistical distributions (count, min, max, mean, median, p95, stdev, iqr).
# Input: numbers on stdin or argv. Output: JSON fragment string.
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

        # Median
        my $median;
        if ($n % 2 == 1) {
            $median = $vals[int($n / 2)];
        } else {
            $median = ($vals[$n / 2 - 1] + $vals[$n / 2]) / 2.0;
        }

        # Percentile 95 (nearest rank or linear interpolation)
        my $p95_idx = int(0.95 * $n);
        $p95_idx = $n - 1 if $p95_idx >= $n;
        my $p95 = $vals[$p95_idx];

        # Standard deviation
        my $sq_sum = 0;
        for my $v (@vals) {
            $sq_sum += ($v - $mean) ** 2;
        }
        my $stdev = $n > 1 ? sqrt($sq_sum / ($n - 1)) : 0.0;

        # Interquartile range (IQR = Q3 - Q1)
        my $q1 = $vals[int(0.25 * $n)];
        my $q3 = $vals[int(0.75 * $n)];
        $q3 = $vals[-1] if int(0.75 * $n) >= $n;
        my $iqr = $q3 - $q1;

        printf "{\"count\":%d,\"min\":%.3f,\"max\":%.3f,\"mean\":%.3f,\"median\":%.3f,\"p95\":%.3f,\"stdev\":%.3f,\"iqr\":%.3f}",
            $n, $min, $max, $mean, $median, $p95, $stdev, $iqr;
    ' "$@"
}

# Parse wt-stage lines from a log file.
# Emits key=value pairs for known stage names.
parse_stage_log() {
    local logfile=$1
    [ -f "$logfile" ] || return 0

    awk '
        /^wt-stage / {
            sub(/^wt-stage /, "", $0)
            split($0, kv, "=")
            if (length(kv[1]) > 0 && length(kv[2]) > 0) {
                print kv[1] "=" kv[2]
            }
        }
    ' "$logfile"
}

# Get host system telemetry as a JSON object.
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

# Format a complete scenario evaluation result into JSON.
# Args: scenario_name, scenario_phase, files, packages, wall_stats_json, stages_json, fidelity_json, disk_json
build_scenario_json() {
    local name=$1 phase=$2 files=$3 pkgs=$4 wall_stats=$5 stages=$6 fidelity=$7 disk=$8
    printf '{"scenario":"%s","phase":"%s","files":%d,"packages":%d,"wall_clock_ms":%s,"stages":%s,"fidelity":%s,"disk":%s}' \
        "$name" "$phase" "$files" "$pkgs" "$wall_stats" "$stages" "$fidelity" "$disk"
}
