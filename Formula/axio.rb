# Written by scripts/formula.sh; do not edit by hand.
class Axio < Formula
  desc "Minimal, fast coding agent in one Rust binary"
  homepage "https://github.com/umbra-me/axio"
  version "0.6.0"
  license "Apache-2.0"

  on_macos do
    on_arm do
      url "https://github.com/umbra-me/axio/releases/download/v0.6.0/axio-aarch64-apple-darwin"
      sha256 "5107095422e0889e907f5bf3882177a091527b6c2c8bddb2cf0934b20d0bdfdf"
    end
    on_intel do
      url "https://github.com/umbra-me/axio/releases/download/v0.6.0/axio-x86_64-apple-darwin"
      sha256 "f9f3cf6004c04cfc5ae34600ef5c5a6f394c0581bf2d4a52cf55c2acff7225be"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/umbra-me/axio/releases/download/v0.6.0/axio-aarch64-unknown-linux-musl"
      sha256 "60655838a3ab728d39c8d2cdd4787ba056093eccfa431ea6659a93d1b92ac59c"
    end
    on_intel do
      url "https://github.com/umbra-me/axio/releases/download/v0.6.0/axio-x86_64-unknown-linux-musl"
      sha256 "fb54bc76116ef7c2c3afbdbf48970c8c304cb3cf3a491a368561d3272b6b925f"
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
