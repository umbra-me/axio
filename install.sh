#!/bin/sh
# Builds axio from this checkout and adds the short names:
#   axio, ax       your configured profile
#   axio-full      the full profile
#   axio-min       the minimal profile
# They are links to one binary, which reads the name it was started under.
set -eu
cd "$(dirname "$0")"
cargo install --path . --locked
bin="${CARGO_HOME:-$HOME/.cargo}/bin"
for name in ax axio-full axio-min; do
  ln -sf axio "$bin/$name"
done
echo "installed axio, ax, axio-full and axio-min in $bin"
echo "default profile: $("$bin/axio" profile | head -1 | sed 's/default profile: //')  (change it with: axio profile full | min)"
