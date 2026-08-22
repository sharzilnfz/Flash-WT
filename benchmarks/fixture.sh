# Fixture generator for the benchmark suite (ticket 08).
#
# Sourced by run.sh. Builds the published benchmarks' tree shape:
# thousands of tiny files spread across hundreds of package-style
# directories under a `node_modules` root — the small-file churn the
# tool exists to end. Deterministic content, no network, so the same
# numbers are reproducible on any macOS or Linux machine.

FILES_PER_PKG=8

# Write `total` small files into `$1/node_modules`, laid out as
# `pkg-NNNNN/{lib,dist}/file`. Every file is created fresh with real
# bytes, which is exactly what a dependency install does at the
# filesystem level — that is what makes it a fair baseline.
generate_tree() {
    root="$1/node_modules"
    total=$2
    pkgs=$((total / FILES_PER_PKG))

    mkdir -p "$root"
    i=0
    while [ "$i" -lt "$pkgs" ]; do
        pkg="$root/pkg-$(printf '%05d' "$i")"
        mkdir -p "$pkg/lib" "$pkg/dist"
        j=0
        while [ "$j" -lt 6 ]; do
            printf '// fake module %d.%d\nmodule.exports = %d;\n' \
                "$i" "$j" "$j" >"$pkg/lib/mod-$j.js"
            j=$((j + 1))
        done
        j=0
        while [ "$j" -lt 2 ]; do
            printf '{"name":"pkg-%05d","main":"lib/mod-0.js","version":"1.0.%d"}\n' \
                "$i" "$j" >"$pkg/dist/meta-$j.json"
            j=$((j + 1))
        done
        i=$((i + 1))
    done

    # Remainder files so the count is exact whatever `total` is.
    extra="$root/pkg-extra/lib"
    mkdir -p "$extra"
    r=$((pkgs * FILES_PER_PKG))
    while [ "$r" -lt "$total" ]; do
        printf '// leftover %d\n' "$r" >"$extra/left-$r.js"
        r=$((r + 1))
    done
}

# Recursively count regular files under $1. The externally observable
# size of a fixture or hydrated tree.
count_files() {
    find "$1" -type f | wc -l | tr -d ' '
}
