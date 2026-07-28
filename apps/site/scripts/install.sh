#!/bin/sh
# axio installer — https://axio.sh/install
#
#   curl -fsSL https://axio.sh/install | sh
#
# This builds axio from source with cargo. There is no published binary: no
# release has been tagged, so there is nothing to download and verify. When one
# is cut this script will prefer it and fall back to a source build.
#
# It installs into your cargo bin directory as your own user. It does not use
# sudo, does not write outside CARGO_HOME, and does not modify your shell
# profile. Read it before running it — that advice applies to every script
# anyone asks you to pipe into a shell, including this one.

set -eu

REPO="https://github.com/umbra-me/axio"
MIN_RUST="1.88"
REF="${AXIO_INSTALL_REF:-}"

# Colour only when stdout is a terminal. Piped into `sh`, stdout is usually not,
# and escape codes in a log are noise.
if [ -t 1 ]; then
  b="$(printf '\033[1m')"; dim="$(printf '\033[2m')"; r="$(printf '\033[0m')"
  amber="$(printf '\033[33m')"; red="$(printf '\033[31m')"
else
  b=''; dim=''; r=''; amber=''; red=''
fi

say()  { printf '%s\n' "$*"; }
step() { printf '%s==>%s %s\n' "$amber" "$r" "$*"; }
die()  { printf '%serror:%s %s\n' "$red" "$r" "$*" >&2; exit 1; }

usage() {
  cat <<EOF
${b}axio installer${r}

  curl -fsSL https://axio.sh/install | sh

${b}Environment${r}
  AXIO_INSTALL_REF   Build a branch, tag or commit instead of the default branch.
  CARGO_HOME         Where the binary is installed. Defaults to ~/.cargo.

${b}What it does${r}
  Builds axio from source with cargo and installs it into your cargo bin
  directory. Needs a Rust toolchain at ${MIN_RUST} or newer. No sudo, no writes
  outside CARGO_HOME, no changes to your shell profile.
EOF
}

for arg in "$@"; do
  case "$arg" in
    -h|--help) usage; exit 0 ;;
    *) die "unknown argument: $arg (try --help)" ;;
  esac
done

# --- preflight ---------------------------------------------------------------

case "$(uname -s)" in
  Linux|Darwin|MSYS*|MINGW*|CYGWIN*) ;;
  *) die "unsupported platform: $(uname -s). Build from source: $REPO" ;;
esac

command -v cargo >/dev/null 2>&1 || die "cargo not found.
  axio is built from source, so a Rust toolchain is required.
  Install one with rustup:  https://rustup.rs
  Then run this script again."

# `cargo --version` prints e.g. "cargo 1.89.0 (abc123 2026-01-01)". Compare on
# major.minor only: a patch release never gates a build.
rust_version="$(cargo --version 2>/dev/null | awk '{print $2}')"
[ -n "$rust_version" ] || die "could not read the version from 'cargo --version'."

version_lt() {
  # Returns 0 when $1 is older than $2, comparing major then minor numerically.
  a_major="${1%%.*}"; a_rest="${1#*.}"; a_minor="${a_rest%%.*}"
  b_major="${2%%.*}"; b_rest="${2#*.}"; b_minor="${b_rest%%.*}"
  [ "$a_major" -lt "$b_major" ] && return 0
  [ "$a_major" -gt "$b_major" ] && return 1
  [ "$a_minor" -lt "$b_minor" ]
}

if version_lt "$rust_version" "$MIN_RUST"; then
  die "Rust $rust_version is too old; axio needs $MIN_RUST or newer.
  Update with:  rustup update stable"
fi

cargo_bin="${CARGO_HOME:-$HOME/.cargo}/bin"

say ""
say "${b}axio${r} ${dim}— building from source${r}"
say "  toolchain   Rust $rust_version"
say "  source      $REPO${REF:+ @ $REF}"
say "  destination $cargo_bin"
say ""
say "${dim}No release has been tagged yet, so there is no binary to download.${r}"
say "${dim}This compiles the current source, which takes a few minutes.${r}"
say ""

# --- install -----------------------------------------------------------------

step "cargo install"
if [ -n "$REF" ]; then
  cargo install --git "$REPO" --rev "$REF" --locked axio
else
  cargo install --git "$REPO" --locked axio
fi

# --- verify ------------------------------------------------------------------

# Check the binary that was just installed, not whatever `axio` resolves to —
# reporting success from a different copy already on PATH would be a lie.
installed="$cargo_bin/axio"
[ -x "$installed" ] || die "cargo reported success but $installed is not executable."

say ""
step "installed"
"$installed" --version 2>/dev/null || say "  (this build does not report --version)"

if command -v axio >/dev/null 2>&1 && [ "$(command -v axio)" = "$installed" ]; then
  say ""
  say "  ${b}axio --doctor${r}   ${dim}what axio can see, offline${r}"
  say "  ${b}axio${r}             ${dim}start an interactive session${r}"
else
  say ""
  say "  ${amber}$cargo_bin is not first on your PATH.${r}"
  say "  Add it, or run the binary directly:"
  say "    $installed --doctor"
fi
say ""
