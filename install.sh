#!/bin/sh
set -eu

REPO=${FLASHFLASHWT_REPO:-sharzilnfz/Flash-WT}
BIN_DIR=${FLASHWT_BIN_DIR:-$HOME/.local/bin}

case "$(uname -s)/$(uname -m)" in
  Darwin/arm64) TARGET=aarch64-apple-darwin ;;
  Darwin/*) TARGET=x86_64-apple-darwin ;;
  Linux/x86_64) TARGET=x86_64-unknown-linux-gnu ;;
  *)
    echo "flashwt: unsupported platform $(uname -s)/$(uname -m)" >&2
    exit 1
    ;;
esac

fetch() {
  if [ -n "${FLASHWT_DIST_DIR:-}" ]; then
    cp "$FLASHWT_DIST_DIR/$1" "$TMP/$1"
  else
    URL="https://github.com/$REPO/releases/${FLASHWT_VERSION:+download/$FLASHWT_VERSION/}$1"
    [ -n "$FLASHWT_VERSION" ] || URL="https://github.com/$REPO/releases/latest/download/$1"
    curl -fSL --proto '=https' -o "$TMP/$1" "$URL"
  fi
  printf '%s\n' "$TMP/$1"
}

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

if [ -z "${FLASHWT_VERSION:-}" ] && [ -z "${FLASHWT_DIST_DIR:-}" ]; then
  FLASHWT_VERSION=$(curl -fsSI --proto '=https' \
    "https://github.com/$REPO/releases/latest" |
    grep -i '^location:' | sed -E 's|.*/tag/(v[^[:space:]]+).*|\1|' | tr -d '\r')
  [ -n "$FLASHWT_VERSION" ] || {
    echo "flashwt: could not determine latest release of $REPO" >&2
    exit 1
  }
fi
VERSION_NO_V=${FLASHWT_VERSION#v}
[ -n "$VERSION_NO_V" ] || VERSION_NO_V=unknown
if [ -n "${FLASHWT_VERSION:-}" ]; then
  FLASHWT_VERSION="v${VERSION_NO_V}"
fi

ARCHIVE="flashwt-v${VERSION_NO_V}-${TARGET}.tar.gz"
fetch "$ARCHIVE" >/dev/null
fetch "$ARCHIVE.sha256" >/dev/null

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

EXPECTED=$(cut -d' ' -f1 <"$TMP/$ARCHIVE.sha256" | tr -d ' \n')
ACTUAL=$(sha256_file "$TMP/$ARCHIVE")
[ "$ACTUAL" = "$EXPECTED" ] || {
  echo "flashwt: checksum mismatch for $ARCHIVE (expected $EXPECTED, got $ACTUAL)" >&2
  exit 1
}

mkdir -p "$BIN_DIR"
tar xzf "$TMP/$ARCHIVE" -C "$TMP"
EXTRACTED="$TMP/flashwt-v${VERSION_NO_V}-${TARGET}"
if [ -f "$EXTRACTED/flashwt" ]; then
  mv "$EXTRACTED/flashwt" "$BIN_DIR/flashwt"
fi
chmod +x "$BIN_DIR/flashwt"
ln -sf flashwt "$BIN_DIR/flash-flashwt"

INSTALLED=$("$BIN_DIR/flashwt" --version)
echo "$INSTALLED"
case "$INSTALLED" in
  flashwt\ * | flash-flashwt\ *) ;;
  *)
    echo "flashwt: $BIN_DIR/flashwt did not run correctly" >&2
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

try_completion() {
  shell=$1
  dir=$2
  case "$dir" in
    "$HOME"/*) mkdir -p "$dir" 2>/dev/null ;;
  esac
  [ -d "$dir" ] && [ -w "$dir" ] || return 1
  "$BIN_DIR/flashwt" completions "$shell" >"$dir/flashwt" 2>/dev/null || {
    rm -f "$dir/flashwt"
    return 1
  }
  echo "installed $shell completions at $dir/flashwt"
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

if [ "${FLASHWT_COMPLETIONS:-auto}" != no ]; then
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

echo "installed flashwt at $BIN_DIR/flashwt"

