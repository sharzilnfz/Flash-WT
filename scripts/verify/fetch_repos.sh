#!/usr/bin/env bash
# scripts/verify/fetch_repos.sh — Real-world repository integration and setup manager.
#
# Prepares realistic git repositories for real-world integration verification:
#  1. Node.js repo with deeply nested node_modules, .bin symlinks, package.json
#  2. Rust workspace with Cargo.toml, target/debug/ deps, rlibs, binaries
#  3. Python project with pyproject.toml, .venv/, site-packages, pycache, symlinks
#  4. Multi-ecosystem monorepo with web, rust, and python under a unified .wtinclude
#
# Can be run standalone or sourced by test suites.

set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
. "$DIR/generators.sh"

init_git_repo() {
    local dir="$1"
    local name="$2"

    mkdir -p "$dir"
    git -C "$dir" init -q
    git -C "$dir" config user.email "verify@example.com"
    git -C "$dir" config user.name "Verify Bot"
    git -C "$dir" config commit.gpgsign false
}

prepare_node_repo() {
    local target_dir="$1"
    local file_count="${2:-2000}"

    init_git_repo "$target_dir" "real-node"

    cat << 'EOF' > "$target_dir/package.json"
{
  "name": "real-node-app",
  "version": "1.0.0",
  "scripts": {
    "start": "node index.js"
  },
  "dependencies": {
    "express": "^4.19.2",
    "lodash": "^4.17.21"
  }
}
EOF

    cat << 'EOF' > "$target_dir/index.js"
const express = require('express');
console.log("node app ready");
EOF

    cat << 'EOF' > "$target_dir/.gitignore"
node_modules/
.DS_Store
dist/
EOF

    cat << 'EOF' > "$target_dir/.wtinclude"
node_modules/
dist/
EOF

    git -C "$target_dir" add package.json index.js .gitignore .wtinclude
    git -C "$target_dir" commit -qm "feat: initial commit of node service"

    # Populate realistic node_modules
    generate_tree_d "$target_dir/node_modules" "$file_count"
}

prepare_rust_repo() {
    local target_dir="$1"
    local file_count="${2:-2000}"

    init_git_repo "$target_dir" "real-rust"

    cat << 'EOF' > "$target_dir/Cargo.toml"
[package]
name = "real-rust-service"
version = "0.1.0"
edition = "2021"

[dependencies]
serde = { version = "1.0", features = ["derive"] }
tokio = { version = "1.0", features = ["full"] }
EOF

    mkdir -p "$target_dir/src"
    cat << 'EOF' > "$target_dir/src/main.rs"
fn main() {
    println!("real rust service running");
}
EOF

    cat << 'EOF' > "$target_dir/.gitignore"
target/
.DS_Store
EOF

    cat << 'EOF' > "$target_dir/.wtinclude"
target/
EOF

    git -C "$target_dir" add Cargo.toml src/main.rs .gitignore .wtinclude
    git -C "$target_dir" commit -qm "feat: initial commit of rust workspace"

    # Populate realistic target/debug tree
    generate_tree_rust_target "$target_dir/target" "$file_count"
}

prepare_python_repo() {
    local target_dir="$1"
    local file_count="${2:-2000}"

    init_git_repo "$target_dir" "real-python"

    cat << 'EOF' > "$target_dir/pyproject.toml"
[project]
name = "real-python-service"
version = "0.1.0"
dependencies = [
    "fastapi>=0.110.0",
    "uvicorn>=0.28.0"
]
EOF

    mkdir -p "$target_dir/app"
    cat << 'EOF' > "$target_dir/app/main.py"
def app():
    return "python service ok"
EOF

    cat << 'EOF' > "$target_dir/.gitignore"
.venv/
__pycache__/
*.pyc
.DS_Store
EOF

    cat << 'EOF' > "$target_dir/.wtinclude"
.venv/
__pycache__/
EOF

    git -C "$target_dir" add pyproject.toml app/main.py .gitignore .wtinclude
    git -C "$target_dir" commit -qm "feat: initial commit of python project"

    # Populate realistic .venv tree
    generate_tree_python_venv "$target_dir/.venv" "$file_count"
}

prepare_monorepo() {
    local target_dir="$1"

    init_git_repo "$target_dir" "real-monorepo"

    mkdir -p "$target_dir/apps/web" "$target_dir/crates/core" "$target_dir/services/api"

    cat << 'EOF' > "$target_dir/.gitignore"
node_modules/
target/
.venv/
__pycache__/
.DS_Store
EOF

    generate_tree_monorepo "$target_dir"

    git -C "$target_dir" add .gitignore .wtinclude apps/web/src crates/core/src services/api/app.py
    git -C "$target_dir" commit -qm "feat: initial commit of multi-ecosystem monorepo"
}

# CLI entrypoint when executed directly
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    cmd="${1:-help}"
    dest="${2:-}"

    case "$cmd" in
        node)
            [ -n "$dest" ] || { echo "Usage: $0 node <destination_dir> [file_count]" >&2; exit 1; }
            prepare_node_repo "$dest" "${3:-2000}"
            echo "Prepared Node.js repository at $dest"
            ;;
        rust)
            [ -n "$dest" ] || { echo "Usage: $0 rust <destination_dir> [file_count]" >&2; exit 1; }
            prepare_rust_repo "$dest" "${3:-2000}"
            echo "Prepared Rust repository at $dest"
            ;;
        python)
            [ -n "$dest" ] || { echo "Usage: $0 python <destination_dir> [file_count]" >&2; exit 1; }
            prepare_python_repo "$dest" "${3:-2000}"
            echo "Prepared Python repository at $dest"
            ;;
        monorepo)
            [ -n "$dest" ] || { echo "Usage: $0 monorepo <destination_dir>" >&2; exit 1; }
            prepare_monorepo "$dest"
            echo "Prepared Monorepo repository at $dest"
            ;;
        all)
            [ -n "$dest" ] || { echo "Usage: $0 all <destination_base_dir>" >&2; exit 1; }
            prepare_node_repo "$dest/node" 1000
            prepare_rust_repo "$dest/rust" 1000
            prepare_python_repo "$dest/python" 1000
            prepare_monorepo "$dest/monorepo"
            echo "Prepared all 4 ecosystems under $dest"
            ;;
        *)
            echo "Usage: $0 {node|rust|python|monorepo|all} <dest_dir>"
            exit 1
            ;;
    esac
fi
