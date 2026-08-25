#!/bin/sh
# Bumps the three committed version numbers together:
#   package.json "version", api/Cargo.toml [package] version,
#   contract/openapi.yaml info.version.
# Usage: scripts/bump-version.sh X.Y.Z
# scripts/check-version-sync.sh (run in CI by test.yml) fails the build if
# they ever drift, so always bump through this script. POSIX sh on purpose.
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
sed -i "0,/^[[:space:]]*\"version\": \"[^\"]*\"/s//  \"version\": \"$new_version\"/" package.json
sed -i "0,/^version = \"[^\"]*\"$/s//version = \"$new_version\"/" api/Cargo.toml
sed -i "0,/^  version: .*$/s//  version: $new_version/" contract/openapi.yaml

sh scripts/check-version-sync.sh

echo "Bumped to $new_version. Don't forget to:"
echo "  1. refresh api/Cargo.lock:  cd api && cargo metadata --format-version 1 >/dev/null"
echo "  2. add a $new_version section to CHANGELOG.md"
