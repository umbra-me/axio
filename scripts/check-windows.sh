#!/usr/bin/env bash
# Type-check the cfg-gated code against Windows, from any platform.
#
# Every Windows break so far has been a `cfg`-gated compile error — an unused
# import left behind by a gated test, a constant no longer referenced on that
# platform — and every one was found by CI rather than locally, costing a
# round-trip each time. `cargo check` needs no linker, so the whole class is
# catchable here.
#
# `axio-provider` is excluded: it pulls in `ring`, whose build script needs a C
# toolchain for the target. It also contains no platform-specific code, which is
# why excluding it costs nothing.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

TARGET=x86_64-pc-windows-msvc

if ! rustup target list --installed | grep -qx "$TARGET"; then
  echo "installing $TARGET std…"
  rustup target add "$TARGET"
fi

cargo clippy -p axio-core -p axio-tools --all-targets --target "$TARGET" -- -D warnings
echo "windows: clean"
