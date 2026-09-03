#!/bin/sh
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

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1"
  else
    shasum -a 256 "$1"
  fi
}

package() {
  V=$1
  TAG_DIR="$DIST/v$V"
  mkdir -p "$TAG_DIR"
  for T in aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-gnu; do
    DIR="$WORK/flashwt-v$V-$T"
    mkdir "$DIR"
    if [ "$V" = "$VERSION" ]; then
      cp target/release/flashwt "$DIR/flashwt"
      ln -s flashwt "$DIR/flash-flashwt"
    else
      printf '#!/bin/sh\necho "flashwt %s"\n' "$V" >"$DIR/flashwt"
      chmod +x "$DIR/flashwt"
      ln -s flashwt "$DIR/flash-flashwt"
    fi
    tar czf "$TAG_DIR/flashwt-v$V-$T.tar.gz" -C "$WORK" "flashwt-v$V-$T"
    (
      cd "$TAG_DIR" && sha256_file "flashwt-v$V-$T.tar.gz"
    ) >"$TAG_DIR/flashwt-v$V-$T.tar.gz.sha256"
  done
}
package "$VERSION"
package "$OLD_VERSION"

echo "== curl installer path"
FLASHWT_DIST_DIR="$DIST/v$VERSION" FLASHWT_VERSION="v$VERSION" FLASHWT_BIN_DIR="$WORK/bin" \
  sh install.sh
GOT=$("$WORK/bin/flashwt" --version)
[ "$GOT" = "flashwt $VERSION" ] ||
  { echo "smoke: expected 'flashwt $VERSION', got '$GOT'" >&2; exit 1; }
echo "ok: curl path installed flashwt $VERSION and verified it runs"

echo "== checksum rejection"
mkdir -p "$WORK/bad"
cp "$DIST/v$VERSION/flashwt-v$VERSION-$TARGET.tar.gz" "$WORK/bad/"
printf 'deadbeef\n' >"$WORK/bad/flashwt-v$VERSION-$TARGET.tar.gz.sha256"
if FLASHWT_DIST_DIR="$WORK/bad" FLASHWT_VERSION="v$VERSION" FLASHWT_BIN_DIR="$WORK/bin2" \
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
  FLASHFLASHWT_REPO=local/smoke FLASHWT_DOWNLOAD_BASE="file://$DIST" \
    ./scripts/gen-formula.sh "$V" "$DIST/v$V"
}

export HOMEBREW_NO_AUTO_UPDATE=1
TAP="$(brew --repository)/Library/Taps/local/homebrew-smoke"
rm -rf "$TAP"
mkdir -p "$TAP/Formula"
git init -q "$TAP"

formula_for "$OLD_VERSION" >"$TAP/Formula/flashwt.rb"
brew uninstall --ignore-dependencies flashwt >/dev/null 2>&1 || true
brew install local/smoke/flashwt
GOT=$("$(brew --prefix)/bin/flashwt" --version)
[ "$GOT" = "flashwt $OLD_VERSION" ] ||
  { echo "smoke: brew install produced '$GOT'" >&2; exit 1; }
echo "ok: brew installed flashwt $OLD_VERSION"

formula_for "$VERSION" >"$TAP/Formula/flashwt.rb"
brew upgrade flashwt
GOT=$("$(brew --prefix)/bin/flashwt" --version)
[ "$GOT" = "flashwt $VERSION" ] ||
  { echo "smoke: brew upgrade produced '$GOT'" >&2; exit 1; }
echo "ok: brew upgraded flashwt to $VERSION"

brew uninstall flashwt || true

