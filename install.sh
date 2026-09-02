#!/bin/sh
# Install wt from a GitHub release, or from a local dist directory.
#
#   curl -fsSL https://raw.githubusercontent.com/OWNER/wt/main/install.sh | sh
#
# Environment:
#   WT_REPO       GitHub repo as owner/name (default: sharzilnfz/wt)
#   WT_VERSION    release tag to install (default: latest)
#   WT_BIN_DIR    install directory (default: ~/.local/bin)
#   WT_DIST_DIR   install from this local directory instead of downloading;
#                 must contain the same wt-v<version>-<target>.tar.gz files
#                 the release carries (used by scripts/smoke-install.sh)
#   WT_COMPLETIONS  auto (default) installs shell completions when a
#                 completion directory can be located; no skips them
set -eu

REPO=${WT_REPO:-sharzilnfz/wt}
BIN_DIR=${WT_BIN_DIR:-$HOME/.local/bin}

case "$(uname -s)/$(uname -m)" in
  Darwin/arm64) TARGET=aarch64-apple-darwin ;;
  Darwin/*) TARGET=x86_64-apple-darwin ;;
  Linux/x86_64) TARGET=x86_64-unknown-linux-gnu ;;
  *)
    echo "wt: unsupported platform $(uname -s)/$(uname -m)" >&2
    exit 1
    ;;
esac

# Always ends up as $TMP/<name>; prints the path.
fetch() {
  if [ -n "${WT_DIST_DIR:-}" ]; then
    cp "$WT_DIST_DIR/$1" "$TMP/$1"
  else
    URL="https://github.com/$REPO/releases/${WT_VERSION:+download/$WT_VERSION/}$1"
    [ -n "$WT_VERSION" ] || URL="https://github.com/$REPO/releases/latest/download/$1"
    curl -fSL --proto '=https' -o "$TMP/$1" "$URL"
  fi
  printf '%s\n' "$TMP/$1"
}

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

if [ -z "${WT_VERSION:-}" ] && [ -z "${WT_DIST_DIR:-}" ]; then
  # Resolve "latest" to a concrete tag so the version check below is honest.
  WT_VERSION=$(curl -fsSI --proto '=https' \
    "https://github.com/$REPO/releases/latest" |
    grep -i '^location:' | sed -E 's|.*/tag/(v[^[:space:]]+).*|\1|' | tr -d '\r')
  [ -n "$WT_VERSION" ] || {
    echo "wt: could not determine latest release of $REPO" >&2
    exit 1
  }
fi
VERSION_NO_V=${WT_VERSION#v}
[ -n "$VERSION_NO_V" ] || VERSION_NO_V=unknown
# Normalize bare versions (0.1.0 -> v0.1.0) so both forms resolve to the
# same GitHub release asset; VERSION_NO_V stays bare for archive names.
if [ -n "${WT_VERSION:-}" ]; then
  WT_VERSION="v${VERSION_NO_V}"
fi

ARCHIVE="wt-v${VERSION_NO_V}-${TARGET}.tar.gz"
fetch "$ARCHIVE" >/dev/null
fetch "$ARCHIVE.sha256" >/dev/null

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

# Verify the checksum before extracting anything.
EXPECTED=$(cut -d' ' -f1 <"$TMP/$ARCHIVE.sha256" | tr -d ' \n')
ACTUAL=$(sha256_file "$TMP/$ARCHIVE")
[ "$ACTUAL" = "$EXPECTED" ] || {
  echo "wt: checksum mismatch for $ARCHIVE (expected $EXPECTED, got $ACTUAL)" >&2
  exit 1
}

mkdir -p "$BIN_DIR"
tar xzf "$TMP/$ARCHIVE" -C "$TMP"
mv "$TMP/wt-v${VERSION_NO_V}-${TARGET}/wt" "$BIN_DIR/wt"
chmod +x "$BIN_DIR/wt"

# Prove the binary actually runs on this machine before claiming success.
INSTALLED=$("$BIN_DIR/wt" --version)
echo "$INSTALLED"
case "$INSTALLED" in
  flashwt\ * | flash-wt\ * | wt\ * | wt-hydrate\ *) ;;
  *)
    echo "wt: $BIN_DIR/wt did not run correctly" >&2
    exit 1
    ;;
esac

case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *)
    echo "note: add $BIN_DIR to your PATH, e.g.:" >&2
    echo "  export PATH=\"$BIN_DIR:\$PATH\"" >&2
    ;;
esac

# Best-effort shell completion installation: generate scripts with the
# freshly installed binary and drop them into the first completion
# directory that applies. Per-user directories (under $HOME) are only
# created for the active login shell; pre-existing system-wide
# directories are used whenever present. Skip entirely with
# WT_COMPLETIONS=no.
try_completion() {
  shell=$1
  dir=$2
  case "$dir" in
    "$HOME"/*) mkdir -p "$dir" 2>/dev/null ;;
  esac
  [ -d "$dir" ] && [ -w "$dir" ] || return 1
  "$BIN_DIR/wt" completions "$shell" >"$dir/wt" 2>/dev/null || {
    rm -f "$dir/wt"
    return 1
  }
  echo "installed $shell completions at $dir/wt"
  case "$shell" in
    zsh)
      echo "note: for zsh, add the directory to fpath and run compinit, e.g.:" >&2
      echo "  fpath=($dir \$fpath)" >&2
      echo "  autoload -Uz compinit && compinit" >&2
      ;;
  esac
}

install_completions() {
  shell=$1
  shift
  for dir in "$@"; do
    case "$dir" in
      "$HOME"/*)
        [ "$(basename "${SHELL:-}")" = "$shell" ] || continue
        ;;
    esac
    try_completion "$shell" "$dir" && return 0
  done
  return 0
}

if [ "${WT_COMPLETIONS:-auto}" != no ]; then
  install_completions zsh \
    "$HOME/.zsh/completion" \
    /usr/local/share/zsh/site-functions \
    /usr/share/zsh/site-functions
  install_completions bash \
    "$HOME/.bash_completion.d" \
    "${BASH_COMPLETION_USER_DIR:-$HOME/.local/share/bash-completion}/completions" \
    /etc/bash_completion.d
  install_completions fish \
    "$HOME/.config/fish/completions"
fi

echo "installed wt at $BIN_DIR/wt"
