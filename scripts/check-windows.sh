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
# toolchain for the target. It contains no platform-specific code, so excluding
# it costs nothing.
#
# The `axio` binary is excluded for the same reason and it does NOT cost
# nothing: it depends on `axio-provider`, and it is where the `cfg`-gated code
# lives. Its non-Linux compilation is checked by CI and by nothing here — a
# Linux-only enum variant is dead code elsewhere, which `-D warnings` makes an
# error, and this script will not see it. That gap is real; naming it is the
# best available substitute for closing it.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

TARGET=x86_64-pc-windows-msvc

if ! rustup target list --installed | grep -qx "$TARGET"; then
  echo "installing $TARGET std…"
  rustup target add "$TARGET"
fi

cargo clippy -p axio-core -p axio-tools --all-targets --target "$TARGET" -- -D warnings
echo "windows: clean"
