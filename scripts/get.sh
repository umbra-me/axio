#!/bin/sh
# Installs an axio release binary and checks it against the release's
# SHA256SUMS:
#   curl -fsSL https://raw.githubusercontent.com/umbra-me/axio/main/scripts/get.sh | sh
# AXIO_VERSION=X.Y.Z picks a version (default: the latest release);
# AXIO_INSTALL_DIR picks the folder (default: ~/.local/bin).
set -eu
repo=umbra-me/axio
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) target=aarch64-apple-darwin ;;
  Darwin-x86_64) target=x86_64-apple-darwin ;;
  Linux-x86_64) target=x86_64-unknown-linux-musl ;;
  Linux-aarch64 | Linux-arm64) target=aarch64-unknown-linux-musl ;;
  *)
    echo "axio: no release binary for $(uname -s) $(uname -m)." >&2
    echo "Windows: download it from https://github.com/$repo/releases" >&2
    echo "Elsewhere: cargo install --git https://github.com/$repo --locked" >&2
    exit 1
    ;;
esac
if [ -n "${AXIO_DOWNLOAD_URL:-}" ]; then
  base=$AXIO_DOWNLOAD_URL # tests only
elif [ -n "${AXIO_VERSION:-}" ]; then
  base=https://github.com/$repo/releases/download/v${AXIO_VERSION#v}
else
  base=https://github.com/$repo/releases/latest/download
fi
dir=${AXIO_INSTALL_DIR:-$HOME/.local/bin}
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM

fetch() {
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL -o "$2" "$1"
  else
    wget -q -O "$2" "$1"
  fi
}
sum() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

asset=axio-$target
fetch "$base/$asset" "$tmp/axio" || { echo "axio: could not download $base/$asset" >&2; exit 1; }
fetch "$base/SHA256SUMS" "$tmp/SHA256SUMS" || { echo "axio: could not download SHA256SUMS" >&2; exit 1; }
want=$(awk -v f="$asset" '$2 == f || $2 == "*" f { print $1 }' "$tmp/SHA256SUMS")
got=$(sum "$tmp/axio")
if [ -z "$want" ] || [ "$want" != "$got" ]; then
  echo "axio: $asset does not match SHA256SUMS; nothing was installed" >&2
  exit 1
fi
mkdir -p "$dir"
chmod 755 "$tmp/axio"
mv -f "$tmp/axio" "$dir/axio"
for name in ax axio-full axio-min; do
  ln -sf axio "$dir/$name"
done
echo "installed $("$dir/axio" --version) in $dir (also ax, axio-full, axio-min)"
case ":$PATH:" in
  *":$dir:"*) ;;
  *) echo "add $dir to your PATH, for example: export PATH=\"$dir:\$PATH\"" ;;
esac
