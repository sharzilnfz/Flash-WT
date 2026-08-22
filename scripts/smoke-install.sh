#!/bin/sh
# Exercise both install paths without a GitHub release.
#
#   ./scripts/smoke-install.sh
#
# The curl path is tested end to end against a local directory laid out
# exactly like a release. The brew path is tested too when Homebrew is
# available: an older version is installed first, then upgraded, which
# covers both "installs cleanly" and "upgrades cleanly".
set -eu
cd "$(dirname "$0")/.."

VERSION=$(awk '/^version = /{gsub(/"/,"");sub(/version = /,"");print;exit}' Cargo.toml)
OLD_VERSION=0.0.1

if command -v cargo >/dev/null; then
  :
else
  echo "smoke: cargo not found" >&2
  exit 1
fi

case "$(uname -s)/$(uname -m)" in
  Darwin/arm64) TARGET=aarch64-apple-darwin ;;
  Darwin/*) TARGET=x86_64-apple-darwin ;;
  Linux/x86_64) TARGET=x86_64-unknown-linux-gnu ;;
  *) echo "smoke: unsupported platform" >&2; exit 1 ;;
esac

echo "== building release binary for $TARGET"
cargo build --release --quiet

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
DIST="$WORK/dist"

# Lay out the dist directory like GitHub's release downloads: one
# directory per tag, holding the tarballs and checksums. The same binary
# is packaged under all three target names so the generated formula has
# a complete set of checksums.
package() {
  V=$1
  TAG_DIR="$DIST/v$V"
  mkdir -p "$TAG_DIR"
  for T in aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-gnu; do
    # Top-level directory inside the tarball, matching what CI packages.
    DIR="$WORK/wt-v$V-$T"
    mkdir "$DIR"
    if [ "$V" = "$VERSION" ]; then
      cp target/release/wt "$DIR/wt"
    else
      # The stand-in older version must be distinguishable from the real
      # one so the upgrade assertions below mean something.
      printf '#!/bin/sh\necho "wt %s"\n' "$V" >"$DIR/wt"
      chmod +x "$DIR/wt"
    fi
    tar czf "$TAG_DIR/wt-v$V-$T.tar.gz" -C "$WORK" "wt-v$V-$T"
    (
      cd "$TAG_DIR" &&
        if command -v sha256sum >/dev/null; then
          sha256sum "wt-v$V-$T.tar.gz"
        else
          shasum -a 256 "wt-v$V-$T.tar.gz"
        fi
    ) >"$TAG_DIR/wt-v$V-$T.tar.gz.sha256"
  done
}
package "$VERSION"
package "$OLD_VERSION"

echo "== curl installer path"
WT_DIST_DIR="$DIST/v$VERSION" WT_VERSION="v$VERSION" WT_BIN_DIR="$WORK/bin" \
  sh install.sh
GOT=$("$WORK/bin/wt" --version)
[ "$GOT" = "wt $VERSION" ] ||
  { echo "smoke: expected 'wt $VERSION', got '$GOT'" >&2; exit 1; }
echo "ok: curl path installed wt $VERSION and verified it runs"

echo "== checksum rejection"
mkdir -p "$WORK/bad"
cp "$DIST/v$VERSION/wt-v$VERSION-$TARGET.tar.gz" "$WORK/bad/"
printf 'deadbeef\n' >"$WORK/bad/wt-v$VERSION-$TARGET.tar.gz.sha256"
if WT_DIST_DIR="$WORK/bad" WT_VERSION="v$VERSION" WT_BIN_DIR="$WORK/bin2" \
  sh install.sh 2>/dev/null; then
  echo "smoke: installer accepted a corrupt download" >&2
  exit 1
fi
echo "ok: installer rejects bad checksums"

if ! command -v brew >/dev/null; then
  echo "skip: brew not installed; formula path untested here"
  exit 0
fi

echo "== brew formula path"
formula_for() {
  V=$1
  WT_REPO=local/smoke WT_DOWNLOAD_BASE="file://$DIST" \
    ./scripts/gen-formula.sh "$V" "$DIST/v$V"
}

# Recent Homebrew refuses bare formula files, so stage a throwaway tap.
export HOMEBREW_NO_AUTO_UPDATE=1
TAP="$(brew --repository)/Library/Taps/local/homebrew-smoke"
rm -rf "$TAP"
mkdir -p "$TAP/Formula"
git init -q "$TAP"

formula_for "$OLD_VERSION" >"$TAP/Formula/wt.rb"
brew uninstall --ignore-dependencies wt >/dev/null 2>&1 || true
brew install local/smoke/wt
GOT=$("$(brew --prefix)/bin/wt" --version)
[ "$GOT" = "wt $OLD_VERSION" ] ||
  { echo "smoke: brew install produced '$GOT'" >&2; exit 1; }
echo "ok: brew installed wt $OLD_VERSION"

formula_for "$VERSION" >"$TAP/Formula/wt.rb"
brew upgrade wt
GOT=$("$(brew --prefix)/bin/wt" --version)
[ "$GOT" = "wt $VERSION" ] ||
  { echo "smoke: brew upgrade produced '$GOT'" >&2; exit 1; }
echo "ok: brew upgraded wt to $VERSION"

brew uninstall wt || true
