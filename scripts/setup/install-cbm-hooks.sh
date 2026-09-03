#!/usr/bin/env sh
set -eu

HOOKS_DIR="$(git rev-parse --git-path hooks)"
mkdir -p "$HOOKS_DIR"

cat << 'EOF' > "$HOOKS_DIR/post-checkout"
#!/usr/bin/env sh
if [ "${3:-1}" = "1" ] && command -v codebase-memory-mcp >/dev/null 2>&1; then
  COMMON_DIR="$(git rev-parse --git-common-dir)"
  MAIN_ROOT="$(git -C "$COMMON_DIR/.." rev-parse --show-toplevel)"
  codebase-memory-mcp cli index_repository --repo-path "$MAIN_ROOT" --name instant-worktrees --mode moderate --persistence true >/dev/null 2>&1 &
fi
EOF

cat << 'EOF' > "$HOOKS_DIR/post-merge"
#!/usr/bin/env sh
if command -v codebase-memory-mcp >/dev/null 2>&1; then
  COMMON_DIR="$(git rev-parse --git-common-dir)"
  MAIN_ROOT="$(git -C "$COMMON_DIR/.." rev-parse --show-toplevel)"
  codebase-memory-mcp cli index_repository --repo-path "$MAIN_ROOT" --name instant-worktrees --mode moderate --persistence true >/dev/null 2>&1 &
fi
EOF

chmod +x "$HOOKS_DIR/post-checkout" "$HOOKS_DIR/post-merge"
