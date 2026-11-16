#!/bin/sh
# Writes Formula/axio.rb for a published release from its SHA256SUMS:
#   sh scripts/formula.sh X.Y.Z
set -eu
v=${1:?usage: sh scripts/formula.sh X.Y.Z}
v=${v#v}
cd "$(dirname "$0")/.."
url=https://github.com/umbra-me/axio/releases/download/v$v
sums=$(curl -fsSL "$url/SHA256SUMS")
sha() {
  s=$(printf '%s\n' "$sums" | awk -v f="axio-$1" '$2 == f || $2 == "*" f { print $1 }')
  [ -n "$s" ] || { echo "no axio-$1 in the v$v SHA256SUMS" >&2; exit 1; }
  echo "$s"
}
cat > Formula/axio.rb <<RUBY
# Written by scripts/formula.sh; do not edit by hand.
class Axio < Formula
  desc "Minimal, fast coding agent in one Rust binary"
  homepage "https://github.com/umbra-me/axio"
  version "$v"
  license "Apache-2.0"

  on_macos do
    on_arm do
      url "$url/axio-aarch64-apple-darwin"
      sha256 "$(sha aarch64-apple-darwin)"
    end
    on_intel do
      url "$url/axio-x86_64-apple-darwin"
      sha256 "$(sha x86_64-apple-darwin)"
    end
  end

  on_linux do
    on_arm do
      url "$url/axio-aarch64-unknown-linux-musl"
      sha256 "$(sha aarch64-unknown-linux-musl)"
    end
    on_intel do
      url "$url/axio-x86_64-unknown-linux-musl"
      sha256 "$(sha x86_64-unknown-linux-musl)"
    end
  end

  def install
    bin.install Dir["axio-*"].first => "axio"
    %w[ax axio-full axio-min].each { |name| bin.install_symlink "axio" => name }
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/axio --version")
  end
end
RUBY
echo "wrote Formula/axio.rb for $v"
