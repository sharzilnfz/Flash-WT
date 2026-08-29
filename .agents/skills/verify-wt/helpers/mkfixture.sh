#!/usr/bin/env sh
# Build an isolated fixture for verifying `wt`: a throwaway git repo with a
# fake-heavy directory plus a private store, both under one temp base dir.
#
# Usage:
#   eval "$(mkfixture.sh /path/to/wt)"
#   # then $WT_FIXTURE (base dir), $WT_ORIGIN (repo), $WT_STORE, $WT_BIN are set
#
# Env knobs:
#   WT_FIXTURE_FILES   number of files under heavy/ (default 40)
#   WT_FIXTURE_ROOT    parent dir for the fixture (default /tmp/wt-verify)
#
# The fixture is NOT cleaned up by this script. Cleanup is the caller's job:
# run `wt remove`/`wt sweep` per the skill, then delete $WT_FIXTURE.
set -eu

WT_BIN_INPUT=${1:?usage: eval "\$(mkfixture.sh /path/to/wt)"}
WT_BIN_INPUT=$(cd "$(dirname "$WT_BIN_INPUT")" && pwd)/$(basename "$WT_BIN_INPUT")

[ -x "$WT_BIN_INPUT" ] || { echo "mkfixture: not executable: $WT_BIN_INPUT" >&2; exit 1; }
command -v git >/dev/null || { echo "mkfixture: git not on PATH" >&2; exit 1; }

FILES=${WT_FIXTURE_FILES:-40}
ROOT=${WT_FIXTURE_ROOT:-/tmp/wt-verify}
mkdir -p "$ROOT"
FIXTURE=$(mktemp -d "$ROOT/XXXXXX")
ORIGIN="$FIXTURE/origin"
STORE="$FIXTURE/store"
mkdir -p "$ORIGIN" "$STORE"

git -C "$ORIGIN" init --quiet
git -C "$ORIGIN" config user.email wt-verify@example.com
git -C "$ORIGIN" config user.name "wt-verify"

i=0
while [ "$i" -lt "$FILES" ]; do
  dir="$ORIGIN/heavy/pkg$(printf '%02d' $((i % 20)))/nested"
  mkdir -p "$dir"
  printf 'fake-heavy file %s of %s\n' "$i" "$FILES" > "$dir/file-$i.txt"
  i=$((i + 1))
done

printf 'heavy/\n' > "$ORIGIN/.gitignore"
printf 'heavy/\n' > "$ORIGIN/.wtinclude"
printf 'tracked source\n' > "$ORIGIN/src.txt"
git -C "$ORIGIN" add .
git -C "$ORIGIN" commit --quiet -m init

cat <<EOF
WT_FIXTURE='$FIXTURE'
WT_ORIGIN='$ORIGIN'
WT_STORE='$STORE'
WT_BIN='$WT_BIN_INPUT'
export WT_STORE
wt() { "$WT_BIN_INPUT" "\$@"; }
EOF
