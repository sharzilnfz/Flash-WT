#!/usr/bin/env bash

FILES_PER_PKG=8

FILES_PER_PKG_D=50

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

    extra="$root/pkg-extra/lib"
    mkdir -p "$extra"
    r=$((pkgs * FILES_PER_PKG))
    while [ "$r" -lt "$total" ]; do
        printf '// leftover %d\n' "$r" >"$extra/left-$r.js"
        r=$((r + 1))
    done
}

generate_tree_d() {
    root="$1"
    total=$2
    pkgs=$((total / FILES_PER_PKG_D))

    mkdir -p "$root"

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

    mkdir -p "$root/.bin"
    ln -sf "../pkg-00000/bin/cli.js" "$root/.bin/flashwt-d"
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

count_files() {
    find "$1" -type f | wc -l | tr -d ' '
}

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

list_symlinks() {
    find "$1" -type l | LC_ALL=C sort | while IFS= read -r p; do
        printf '%s %s\n' "$(readlink "$p")" "${p#"$1"/}"
    done
}

