#!/usr/bin/env bash
# Build every feature combination the CI matrix builds, with CI's flags.
#
# `cargo build -p axio --no-default-features` passes locally and fails in CI,
# because CI sets `RUSTFLAGS: -D warnings` and a plain local build does not. The
# gap is exactly one flag wide and it has cost a red build: a field used only by
# the interactive surface is dead code in the headless one, which is a warning,
# which is an error there and not here.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

export RUSTFLAGS="${RUSTFLAGS:--D warnings}"

echo "default features"
cargo build -p axio >/dev/null

echo "no default features (headless)"
cargo build -p axio --no-default-features >/dev/null

echo "features: clean"
