#!/bin/sh
set -eu

VERSION=${1:?usage: gen-formula.sh <version> <dist-dir>}
VERSION=${VERSION#v}
DIST=${2:-.}
TEMPLATE="$(dirname "$0")/../Formula/flashwt.rb"

sha_for() {
  cut -d' ' -f1 <"$DIST/flashwt-v$VERSION-$1.tar.gz.sha256" | tr -d ' \n'
}

DOWNLOAD_BASE=${FLASHWT_DOWNLOAD_BASE:-https://github.com/$FLASHFLASHWT_REPO/releases/download}

TMP_OUT=$(mktemp)
trap 'rm -f "$TMP_OUT"' EXIT
sed \
  -e "s|__REPO__|${FLASHFLASHWT_REPO:?FLASHFLASHWT_REPO must be set to owner/repo}|g" \
  -e "s|__VERSION__|$VERSION|g" \
  -e "s|__DOWNLOAD_BASE__|$DOWNLOAD_BASE|g" \
  -e "s|__SHA256_AARCH64_APPLE_DARWIN__|$(sha_for aarch64-apple-darwin)|" \
  -e "s|__SHA256_X86_64_APPLE_DARWIN__|$(sha_for x86_64-apple-darwin)|" \
  -e "s|__SHA256_X86_64_UNKNOWN_LINUX_GNU__|$(sha_for x86_64-unknown-linux-gnu)|" \
  "$TEMPLATE" >"$TMP_OUT"

if command -v ruby >/dev/null 2>&1; then
  ruby -c "$TMP_OUT" >/dev/null
fi

cat "$TMP_OUT"

