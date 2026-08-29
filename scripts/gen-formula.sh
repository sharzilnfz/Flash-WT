#!/bin/sh
# Fill the checksum placeholders in Formula/wt.rb for a release.
#
#   WT_REPO=owner/name ./scripts/gen-formula.sh <version> <dist-dir>
#
# <version> is the release tag without the leading v (e.g. 0.1.0);
# <dist-dir> holds the wt-v<version>-<target>.tar.gz.sha256 files.
#
# The generated formula installs shell completions by invoking
# `wt completions <shell>` on the binary it installs, so the release
# archives carry no completion artifacts and this script fills no
# completion placeholders.
set -eu

VERSION=${1:?usage: gen-formula.sh <version> <dist-dir>}
DIST=${2:-.}
TEMPLATE="$(dirname "$0")/../Formula/wt.rb"

sha_for() {
  cut -d' ' -f1 <"$DIST/wt-v$VERSION-$1.tar.gz.sha256" | tr -d ' \n'
}

# Override with a file:// path (or anything else) to test the formula
# without a real release; scripts/smoke-install.sh does exactly that.
DOWNLOAD_BASE=${WT_DOWNLOAD_BASE:-https://github.com/$WT_REPO/releases/download}

sed \
  -e "s|__REPO__|${WT_REPO:?WT_REPO must be set to owner/repo}|g" \
  -e "s|__VERSION__|$VERSION|g" \
  -e "s|__DOWNLOAD_BASE__|$DOWNLOAD_BASE|g" \
  -e "s|__SHA256_AARCH64_APPLE_DARWIN__|$(sha_for aarch64-apple-darwin)|" \
  -e "s|__SHA256_X86_64_APPLE_DARWIN__|$(sha_for x86_64-apple-darwin)|" \
  -e "s|__SHA256_X86_64_UNKNOWN_LINUX_GNU__|$(sha_for x86_64-unknown-linux-gnu)|" \
  "$TEMPLATE"
