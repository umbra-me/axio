#!/bin/sh
# Prints the CHANGELOG section for one version, and fails when Cargo.toml
# carries a different version or the section is missing.
#   sh scripts/release-notes.sh 0.1.0
set -eu
cd "$(dirname "$0")/.."
version=$1
crate=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)
if [ "$crate" != "$version" ]; then
  echo "Cargo.toml says $crate, the tag says $version" >&2
  exit 1
fi
notes=$(awk -v v="$version" '
  index($0, "## [" v "]") == 1 { on = 1; next }
  on && /^## / { exit }
  on { print }
' CHANGELOG.md)
if [ -z "$(printf %s "$notes" | tr -d '[:space:]')" ]; then
  echo "CHANGELOG.md has no \"## [$version]\" section" >&2
  exit 1
fi
printf '%s\n' "$notes"
