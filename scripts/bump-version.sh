#!/bin/sh
# Bumps the three committed version numbers together:
#   package.json "version", api/Cargo.toml [package] version,
#   contract/openapi.yaml info.version.
# Usage: scripts/bump-version.sh X.Y.Z
# scripts/check-version-sync.sh (run in CI by test.yml) fails the build if
# they ever drift, so always bump through this script. POSIX sh + awk on
# purpose (no GNU sed -i / 0,addr extensions), so it runs on macOS/BSD too.
set -eu
cd "$(dirname "$0")/.."

new_version="${1:-}"
case "$new_version" in
  *.*.*) ;; # looks like X.Y.Z — good enough, cargo will reject real garbage
  *)
    echo "usage: scripts/bump-version.sh X.Y.Z (e.g. 0.7.0)" >&2
    exit 1
    ;;
esac

# First "version" line of each file only — same anchors as check-version-sync.sh.
replace_first() { # $1 file  $2 ERE pattern  $3 replacement for the matched part
  # sub() swaps only the matched portion, so anything after it on the line
  # (package.json's trailing comma) survives — same behavior as the old sed.
  awk -v pat="$2" -v rep="$3" \
    '!done && $0 ~ pat { sub(pat, rep); done = 1 } { print }' \
    "$1" >"$1.tmp" && mv "$1.tmp" "$1"
}
replace_first package.json          '^[[:space:]]*"version": "[^"]*"' "  \"version\": \"$new_version\""
replace_first api/Cargo.toml        '^version = "[^"]*"$'             "version = \"$new_version\""
replace_first contract/openapi.yaml '^  version: .*$'                 "  version: $new_version"

sh scripts/check-version-sync.sh

echo "Bumped to $new_version. Don't forget to:"
echo "  1. refresh api/Cargo.lock:  cd api && cargo metadata --format-version 1 >/dev/null"
echo "  2. add a $new_version section to CHANGELOG.md"
