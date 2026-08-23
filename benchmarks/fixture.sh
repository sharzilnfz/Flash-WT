#!/usr/bin/env bash
# Fixture generators for the benchmark suite (tickets 08 + T06).
#
# Sourced by run.sh; never executed directly.
# shellcheck shell=bash
#
#   generate_tree    The published benchmarks' shape: thousands of tiny
#                    unique files across hundreds of package-style
#                    directories (scenarios A-C).
#   generate_tree_d  A realistic node_modules-like shape (scenario D):
#                    ~40k files across ~800 packages where almost all
#                    content is duplicated across packages, with mixed
#                    executable bits, nested directories, empty
#                    directories, and .bin-style symlinks.
#
# Deterministic content, no network, so the same numbers are
# reproducible on any macOS or Linux machine.

FILES_PER_PKG=8

# Scenario D shape: every package carries exactly 50 regular files, so
# `--d-files 40000` lands on exactly 800 packages.
FILES_PER_PKG_D=50

# Write `total` small files into `$1` (callers pass the heavy
# directory itself, e.g. `<dest>/node_modules`), laid out as
# `pkg-NNNNN/{lib,dist}/file`. Every file is created fresh with real
# bytes, which is exactly what a dependency install does at the
# filesystem level — that is what makes it a fair baseline.
generate_tree() {
    root="$1"
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

# Write `total` files into `$1` (the heavy directory itself, e.g.
# `<repo>/node_modules`) as a realistic dependency
# tree (scenario D):
#
#   pkg-NNNNN/
#     lib/mod-{0..4}.js                  duplicated across all packages
#     lib/internal/helper-{0..9}.js      duplicated, nested two deep
#     lib/internal/deep/nested/core.js   duplicated, nested three deep
#     dist/chunk-{00..29}.js             duplicated
#     dist/license.txt                   duplicated boilerplate
#     bin/cli.js                         duplicated, mode +x
#     package.json                       unique per package
#     README.md                          unique per package
#     cache/  logs/                      empty directories
#   .bin/cli-pkg-NNNNN -> ../pkg-NNNNN/bin/cli.js
#
# That is 48 of 50 files per package byte-identical across packages —
# a 96% duplicate-content ratio, which is what makes a content-addressed
# store pay off and what a real install looks like after dedup. Every
# third package also gets an executable chunk file so the exec bit is
# mixed through the tree rather than uniform.
#
# All duplicated content is written by a single awk process (one fork
# total) because regenerating ~40k files per timed baseline run from
# one-shell-loop-per-file would dominate scenario D's baseline timing
# for reasons that have nothing to do with what an installer spends
# its time on. Unique files stay in plain shell.
generate_tree_d() {
    root="$1"
    total=$2
    pkgs=$((total / FILES_PER_PKG_D))

    mkdir -p "$root"

    # Directory skeleton first: awk's print-to-file cannot create
    # intermediate directories.
    i=0
    while [ "$i" -lt "$pkgs" ]; do
        pkg="$root/pkg-$(printf '%05d' "$i")"
        mkdir -p "$pkg/lib/internal/deep/nested" "$pkg/dist" "$pkg/bin"
        i=$((i + 1))
    done

    awk -v root="$root" -v pkgs="$pkgs" 'BEGIN {
        license = sprintf("MIT License\n\nCopyright (c) 2026 bench\n\nPermission is hereby granted, free of charge, to any person obtaining a copy\nof this software to deal in the Software without restriction.\n")
        for (i = 0; i < pkgs; i++) {
            pkg = sprintf("%s/pkg-%05d", root, i)
            for (j = 0; j < 5; j++) {
                p = pkg "/lib/mod-" j ".js"
                printf("// shared module %d\nmodule.exports = %d;\n", j, j) > p
                close(p)
            }
            for (k = 0; k < 10; k++) {
                p = pkg "/lib/internal/helper-" k ".js"
                body = sprintf("// internal helper %d: shared verbatim across every package\n", k)
                body = body sprintf("exports.handle = (req) => req.ok ? { status: 200, kind: %d } : null;\n", k)
                printf("%s", body) > p
                close(p)
            }
            p = pkg "/lib/internal/deep/nested/core.js"
            printf("/* nested three levels deep, identical everywhere */\nmodule.exports = Object.freeze({ core: true });\n") > p
            close(p)
            for (c = 0; c < 30; c++) {
                p = pkg "/dist/chunk-" sprintf("%02d", c) ".js"
                body = sprintf("// webpack-style chunk %02d, duplicated across all packages\n", c)
                for (l = 0; l < 6; l++)
                    body = body sprintf("define([\"./mod\"], function(m) { return m.f(%d, %d); });\n", c, l)
                printf("%s", body) > p
                close(p)
            }
            printf("%s", license) > (pkg "/dist/license.txt")
            close(pkg "/dist/license.txt")
            p = pkg "/bin/cli.js"
            printf("#!/usr/bin/env node\nrequire(\"../lib/mod-0.js\");\nprocess.exit(process.argv.length > 2 ? 0 : 1);\n") > p
            close(p)
        }
    }'

    # Per-package unique files, empty dirs, mixed exec bits, symlinks.
    # Small counts each; plain shell stays readable here.
    mkdir -p "$root/.bin"
    ln -sf "../pkg-00000/bin/cli.js" "$root/.bin/wt-d"
    i=0
    while [ "$i" -lt "$pkgs" ]; do
        name=$(printf '%05d' "$i")
        pkg="$root/pkg-$name"
        mkdir -p "$pkg/cache" "$pkg/logs"
        printf '{"name":"pkg-%s","version":"1.0.%d","main":"lib/mod-0.js","bin":{"cli":"bin/cli.js"}}\n' \
            "$name" $((i % 7)) >"$pkg/package.json"
        printf '# pkg-%s\n\nScenario D filler readme.\n' "$name" >"$pkg/README.md"
        chmod +x "$pkg/bin/cli.js"
        if [ $((i % 3)) -eq 0 ]; then
            chmod +x "$pkg/dist/chunk-07.js"
        fi
        ln -sf "../pkg-$name/bin/cli.js" "$root/.bin/cli-pkg-$name"
        i=$((i + 1))
    done

    # Remainder files so the count is exact whatever `total` is.
    r=$((pkgs * FILES_PER_PKG_D))
    if [ "$r" -lt "$total" ]; then
        extra="$root/pkg-extra/lib"
        mkdir -p "$extra"
        while [ "$r" -lt "$total" ]; do
            printf '// leftover %d\n' "$r" >"$extra/left-$r.js"
            r=$((r + 1))
        done
    fi
}

# Recursively count regular files under $1. Symlinks are not regular
# files and are not counted; the externally observable size of a
# fixture or hydrated tree is its regular-file population.
count_files() {
    find "$1" -type f | wc -l | tr -d ' '
}

# Emit "<mode> <relpath>" for every regular file and directory under
# $1, sorted. Symlinks are excluded: stat would see through them, and
# their fidelity is measured by list_symlinks instead. Mode format
# differs between stat flavors (BSD %p includes type bits, GNU %a does
# not); both sides of a comparison are measured on the same platform,
# so like-for-like comparison never sees the difference.
list_modes() {
    case "$(uname -s)" in
        Darwin) stat_args=(-f '%p %N') ;;
        *) stat_args=(-c '%a %n') ;;
    esac
    find "$1" \( -type f -o -type d \) -exec stat "${stat_args[@]}" {} + |
        awk -v pfx="$1/" -v tree="$1" '
            {
                i = index($0, pfx)
                if (i > 0) print substr($0, 1, i - 1) substr($0, i + length(pfx))
                else if (index($0, tree) > 0) print substr($0, 1, index($0, tree) - 1) "."
                else print
            }' | LC_ALL=C sort
}

# Emit "<target> <relpath>" for every symlink under $1, sorted.
# Fixture paths contain no whitespace, so line-oriented output is safe.
list_symlinks() {
    find "$1" -type l | LC_ALL=C sort | while IFS= read -r p; do
        printf '%s %s\n' "$(readlink "$p")" "${p#"$1"/}"
    done
}
