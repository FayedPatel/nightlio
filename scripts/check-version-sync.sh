#!/bin/sh
# Asserts the three committed version numbers agree:
#   package.json        "version"        (canonical)
#   api/Cargo.toml      [package] version (what /api/config reports via
#                       CARGO_PKG_VERSION — the git tag never reaches the
#                       published image, publish.yml retags without
#                       rebuilding, so the committed version is the truth)
#   contract/openapi.yaml  info.version
# Run by .github/workflows/test.yml, which publish.yml gates on, so a
# drifted version can never ship. Bump all three with
# scripts/bump-version.sh. POSIX sh on purpose: no jq/node/yq required.
set -eu
cd "$(dirname "$0")/.."

# First "version" line of each file; sed keeps this dependency-free.
pkg_version=$(sed -n 's/^[[:space:]]*"version": "\([^"]*\)",\{0,1\}$/\1/p' package.json | head -n 1)
cargo_version=$(sed -n 's/^version = "\([^"]*\)"$/\1/p' api/Cargo.toml | head -n 1)
openapi_version=$(sed -n 's/^  version: \(.*\)$/\1/p' contract/openapi.yaml | head -n 1)

fail() {
  echo "version-sync: $1" >&2
  echo "  package.json          .version        = ${pkg_version:-<not found>}" >&2
  echo "  api/Cargo.toml        [package]version = ${cargo_version:-<not found>}" >&2
  echo "  contract/openapi.yaml info.version    = ${openapi_version:-<not found>}" >&2
  echo "Fix: scripts/bump-version.sh X.Y.Z bumps all three together." >&2
  exit 1
}

[ -n "$pkg_version" ] || fail "could not read the version from package.json"
[ -n "$cargo_version" ] || fail "could not read the version from api/Cargo.toml"
[ -n "$openapi_version" ] || fail "could not read info.version from contract/openapi.yaml"

if [ "$pkg_version" != "$cargo_version" ] || [ "$pkg_version" != "$openapi_version" ]; then
  fail "the three committed versions have drifted apart"
fi

echo "version-sync: all three versions agree on $pkg_version"
