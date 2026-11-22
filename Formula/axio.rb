# Written by scripts/formula.sh; do not edit by hand.
class Axio < Formula
  desc "Minimal, fast coding agent in one Rust binary"
  homepage "https://github.com/umbra-me/axio"
  version "0.5.0"
  license "Apache-2.0"

  on_macos do
    on_arm do
      url "https://github.com/umbra-me/axio/releases/download/v0.5.0/axio-aarch64-apple-darwin"
      sha256 "ef76fee37e07864e6038ce36f82e4118bb44960788a814d4836b6e2ae609ff40"
    end
    on_intel do
      url "https://github.com/umbra-me/axio/releases/download/v0.5.0/axio-x86_64-apple-darwin"
      sha256 "16bb967bfc0443600b24eb1dc3cd87e78ca72208db3224228c3960a52a49b80e"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/umbra-me/axio/releases/download/v0.5.0/axio-aarch64-unknown-linux-musl"
      sha256 "230946eedc4ee43ef66072bc7a756e212284523d90d5760f2c101131766d22d8"
    end
    on_intel do
      url "https://github.com/umbra-me/axio/releases/download/v0.5.0/axio-x86_64-unknown-linux-musl"
      sha256 "f3ad62f07c1940771433bb69745aec963bce0d79d01d1d86106adb775129573f"
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
