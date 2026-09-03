#!/usr/bin/env sh
set -eu

FLASHWT_BIN_INPUT=${1:?usage: eval "\$(mkfixture.sh /path/to/flashwt)"}
FLASHWT_BIN_INPUT=$(cd "$(dirname "$FLASHWT_BIN_INPUT")" && pwd)/$(basename "$FLASHWT_BIN_INPUT")

[ -x "$FLASHWT_BIN_INPUT" ] || { echo "mkfixture: not executable: $FLASHWT_BIN_INPUT" >&2; exit 1; }
command -v git >/dev/null || { echo "mkfixture: git not on PATH" >&2; exit 1; }

FILES=${FLASHWT_FIXTURE_FILES:-40}
ROOT=${FLASHWT_FIXTURE_ROOT:-/tmp/flashwt-verify}
mkdir -p "$ROOT"
FIXTURE=$(mktemp -d "$ROOT/XXXXXX")
ORIGIN="$FIXTURE/origin"
STORE="$FIXTURE/store"
mkdir -p "$ORIGIN" "$STORE"

git -C "$ORIGIN" init --quiet
git -C "$ORIGIN" config user.email flashwt-verify@example.com
git -C "$ORIGIN" config user.name "flashwt-verify"

i=0
while [ "$i" -lt "$FILES" ]; do
  dir="$ORIGIN/heavy/pkg$(printf '%02d' $((i % 20)))/nested"
  mkdir -p "$dir"
  printf 'fake-heavy file %s of %s\n' "$i" "$FILES" > "$dir/file-$i.txt"
  i=$((i + 1))
done

printf 'heavy/\n' > "$ORIGIN/.gitignore"
printf 'heavy/\n' > "$ORIGIN/.flashwtinclude"
printf 'tracked source\n' > "$ORIGIN/src.txt"
git -C "$ORIGIN" add .
git -C "$ORIGIN" commit --quiet -m init

cat <<EOF
FLASHWT_FIXTURE='$FIXTURE'
FLASHWT_ORIGIN='$ORIGIN'
FLASHWT_STORE='$STORE'
FLASHWT_BIN='$FLASHWT_BIN_INPUT'
FLASHWT_NO_TINY_BYPASS=1
export FLASHWT_FIXTURE FLASHWT_ORIGIN FLASHWT_STORE FLASHWT_BIN FLASHWT_NO_TINY_BYPASS
flashwt() { "$FLASHWT_BIN_INPUT" "\$@"; }
EOF
