#!/usr/bin/env bash
# fixture_matrix.sh — Multi-ecosystem fixture generators (Rust target, Python venv, Fan-Out).
# Sourced by benchmark and eval runners; builds deterministic directory trees.

set -euo pipefail

# Generate a synthetic Rust target/debug tree into $1
# Simulates incremental build outputs, rlibs, rmetas, and fingerprint caches.
generate_tree_rust_target() {
    local root=$1
    local total_files=${2:-2000}
    local crates=$((total_files / 20))
    [ "$crates" -ge 1 ] || crates=1

    mkdir -p "$root/debug/deps" "$root/debug/incremental" "$root/debug/.fingerprint" "$root/debug/build"

    local i=0
    while [ "$i" -lt "$crates" ]; do
        local cname="crate_$(printf '%04d' "$i")"
        local hash="a$(printf '%07x' "$i")bf42"

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

# Generate a synthetic Python .venv tree into $1
# Simulates site-packages, .dist-info directories, __pycache__/*.pyc, and bin/ symlinks.
generate_tree_python_venv() {
    local root=$1
    local total_files=${2:-2000}
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
        local pname="package_$(printf '%04d' "$i")"
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
