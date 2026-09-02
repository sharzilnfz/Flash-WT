#!/usr/bin/env bash
# scripts/verify/generators.sh — Deterministic multi-ecosystem fixture tree generators.
#
# Generates realistic directory trees for:
#  - Node.js node_modules (~40k files, 96% dedup, symlinks, executables)
#  - Rust target/debug tree (rlibs, rmetas, incremental caches, binaries)
#  - Python .venv (site-packages, __pycache__, bin symlinks, entrypoints)
#  - Multi-ecosystem Monorepo (.wtinclude with web, rust, and python)
#
# Optimized for speed with single-pass awk where possible.

set -euo pipefail

FILES_PER_PKG_D=50

# Generate realistic Node.js node_modules tree (Scenario D)
# $1: destination heavy directory (e.g. /path/to/node_modules)
# $2: total files (divisible by 50, default 40000 -> 800 packages)
generate_tree_d() {
    local root="$1"
    local total="${2:-40000}"
    local pkgs=$((total / FILES_PER_PKG_D))
    [ "$pkgs" -ge 1 ] || pkgs=1

    mkdir -p "$root/.bin"

    # 1. Directory skeleton first
    awk -v root="$root" -v pkgs="$pkgs" 'BEGIN {
        for (i = 0; i < pkgs; i++) {
            p = sprintf("%s/pkg-%05d", root, i)
            print p "/lib/internal/deep/nested"
            print p "/dist"
            print p "/bin"
            print p "/cache"
            print p "/logs"
        }
    }' | xargs -n 100 mkdir -p

    # 2. Awk batch generator for duplicated content across packages
    awk -v root="$root" -v pkgs="$pkgs" 'BEGIN {
        for (i = 0; i < pkgs; i++) {
            p = sprintf("%s/pkg-%05d", root, i)

            # lib/mod-{0..4}.js
            for (m = 0; m < 5; m++) {
                fn = sprintf("%s/lib/mod-%d.js", p, m)
                print "// shared module code\nmodule.exports = { id: " m " };" > fn
                close(fn)
            }

            # lib/internal/helper-{0..9}.js
            for (h = 0; h < 10; h++) {
                fn = sprintf("%s/lib/internal/helper-%d.js", p, h)
                print "// shared internal helper\nfunction help() { return true; }" > fn
                close(fn)
            }

            # lib/internal/deep/nested/core.js
            fn = sprintf("%s/lib/internal/deep/nested/core.js", p)
            print "// deeply nested core module\nmodule.exports = { core: 1 };" > fn
            close(fn)

            # dist/chunk-{00..29}.js
            for (c = 0; c < 30; c++) {
                fn = sprintf("%s/dist/chunk-%02d.js", p, c)
                print "/* bundle chunk */ var x = " c ";" > fn
                close(fn)
            }

            # dist/license.txt
            fn = sprintf("%s/dist/license.txt", p)
            print "MIT License\nCopyright (c) 2026 Test" > fn
            close(fn)

            # bin/cli.js
            fn = sprintf("%s/bin/cli.js", p)
            print "#!/usr/bin/env node\nconsole.log(\"cli running\");" > fn
            close(fn)

            # package.json (unique per package)
            fn = sprintf("%s/package.json", p)
            printf "{\"name\":\"pkg-%05d\",\"version\":\"1.0.0\",\"main\":\"lib/mod-0.js\"}\n", i > fn
            close(fn)

            # README.md (unique per package)
            fn = sprintf("%s/README.md", p)
            printf "# pkg-%05d\nPackage documentation.\n", i > fn
            close(fn)
        }
    }'

    # 3. Permissions and .bin symlinks
    local i=0
    while [ "$i" -lt "$pkgs" ]; do
        local p
        p=$(printf "pkg-%05d" "$i")
        chmod +x "$root/$p/bin/cli.js"
        
        # Every 3rd package gets an executable chunk
        if [ $((i % 3)) -eq 0 ]; then
            chmod +x "$root/$p/dist/chunk-00.js"
        fi

        # Symlink in .bin
        ln -sf "../$p/bin/cli.js" "$root/.bin/cli-$p"
        i=$((i + 1))
    done
}

# Generate synthetic Rust target/debug tree
# $1: destination target directory (e.g. /path/to/target)
# $2: total files (approx, default 2000)
generate_tree_rust_target() {
    local root="$1"
    local total_files="${2:-2000}"
    local crates=$((total_files / 20))
    [ "$crates" -ge 1 ] || crates=1

    mkdir -p "$root/debug/deps" "$root/debug/incremental" "$root/debug/.fingerprint" "$root/debug/build"

    local i=0
    while [ "$i" -lt "$crates" ]; do
        local cname
        cname=$(printf "crate_%04d" "$i")
        local hash
        hash=$(printf "a%07xbf42" "$i")

        # Fingerprint entries
        mkdir -p "$root/debug/.fingerprint/$cname-$hash"
        printf '{"rustc_fingerprint":%d,"features":"","target":0}\n' "$i" \
            >"$root/debug/.fingerprint/$cname-$hash/lib-$cname.json"
        printf '%s\n' "$hash" >"$root/debug/.fingerprint/$cname-$hash/dep-lib-$cname"

        # Dep info and rmeta
        printf '%s/debug/deps/lib%s-%s.rlib: src/lib.rs\n' "$root" "$cname" "$hash" \
            >"$root/debug/deps/lib$cname-$hash.d"
        printf '\x00\x00\x00\x00rustc-rmeta-%s-%d\n' "$hash" "$i" \
            >"$root/debug/deps/lib$cname-$hash.rmeta"

        # Simulated rlib binary archive
        printf '!<arch>\n/               0           0     0     644     16        `\nsynthetic-rlib\n' \
            >"$root/debug/deps/lib$cname-$hash.rlib"

        # Incremental session caches
        mkdir -p "$root/debug/incremental/$cname-$hash/s-1234567-890abc"
        printf 'query-cache-%s\n' "$hash" \
            >"$root/debug/incremental/$cname-$hash/s-1234567-890abc/query-cache.bin"
        printf 'work-products-%s\n' "$hash" \
            >"$root/debug/incremental/$cname-$hash/s-1234567-890abc/work-products.bin"

        i=$((i + 1))
    done

    # Ensure executable target bin
    printf '#!/bin/sh\necho "synthetic rust binary"\n' >"$root/debug/app"
    chmod +x "$root/debug/app"
}

# Generate synthetic Python .venv tree
# $1: destination venv directory (e.g. /path/to/.venv)
# $2: total files (approx, default 2000)
generate_tree_python_venv() {
    local root="$1"
    local total_files="${2:-2000}"
    local pkgs=$((total_files / 15))
    [ "$pkgs" -ge 1 ] || pkgs=1

    local site="$root/lib/python3.11/site-packages"
    mkdir -p "$root/bin" "$site"

    # Virtualenv binary symlinks & activation scripts
    printf '#!/bin/sh\nexec true "$@"\n' >"$root/bin/python3"
    chmod +x "$root/bin/python3"
    ln -sf "python3" "$root/bin/python"
    printf '# activate script\nexport VIRTUAL_ENV="%s"\n' "$root" >"$root/bin/activate"

    local i=0
    while [ "$i" -lt "$pkgs" ]; do
        local pname
        pname=$(printf "package_%04d" "$i")
        local pdir="$site/$pname"
        mkdir -p "$pdir/__pycache__" "$site/$pname-1.0.0.dist-info"

        # Package modules & bytecode
        printf '"""Module %s"""\n__version__ = "1.0.0"\n' "$pname" >"$pdir/__init__.py"
        printf 'def main():\n    return %d\n' "$i" >"$pdir/core.py"
        printf '\x63\x00\x00\x00pyc-bytecode-%s\n' "$pname" >"$pdir/__pycache__/core.cpython-311.pyc"
        printf '\x63\x00\x00\x00pyc-bytecode-init-%s\n' "$pname" >"$pdir/__pycache__/__init__.cpython-311.pyc"

        # Dist-info metadata & RECORD
        printf 'Metadata-Version: 2.1\nName: %s\nVersion: 1.0.0\n' "$pname" >"$site/$pname-1.0.0.dist-info/METADATA"
        printf 'Installer: pip\n' >"$site/$pname-1.0.0.dist-info/INSTALLER"
        printf '%s/__init__.py,,\n%s/core.py,,\n' "$pname" "$pname" >"$site/$pname-1.0.0.dist-info/RECORD"

        # Console script wrapper in bin/
        printf '#!/usr/bin/env python3\nimport %s\n%s.core.main()\n' "$pname" "$pname" >"$root/bin/cli-$pname"
        chmod +x "$root/bin/cli-$pname"

        i=$((i + 1))
    done
}

# Generate a multi-ecosystem monorepo tree
# $1: repository root
generate_tree_monorepo() {
    local root="$1"

    mkdir -p "$root/apps/web/src" "$root/crates/core/src" "$root/services/api"

    # 1. Web app
    printf 'export const name = "web";\n' > "$root/apps/web/src/index.ts"
    generate_tree_d "$root/apps/web/node_modules" 200

    # 2. Rust core
    printf 'pub fn run() -> i32 { 42 }\n' > "$root/crates/core/src/lib.rs"
    generate_tree_rust_target "$root/crates/core/target" 200

    # 3. Python service
    printf 'def handle(): return "ok"\n' > "$root/services/api/app.py"
    generate_tree_python_venv "$root/services/api/.venv" 150

    # 4. Monorepo .wtinclude manifest
    cat << 'EOF' > "$root/.wtinclude"
apps/web/node_modules/
crates/core/target/
services/api/.venv/
EOF
}
