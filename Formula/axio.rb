# Written by scripts/formula.sh; do not edit by hand.
class Axio < Formula
  desc "Minimal, fast coding agent in one Rust binary"
  homepage "https://github.com/umbra-me/axio"
  version "0.3.0"
  license "Apache-2.0"

  on_macos do
    on_arm do
      url "https://github.com/umbra-me/axio/releases/download/v0.3.0/axio-aarch64-apple-darwin"
      sha256 "0b967c4b9045b4d1b213fb1eb854d2ab9385002bbb35108cafd2c562aa959b0f"
    end
    on_intel do
      url "https://github.com/umbra-me/axio/releases/download/v0.3.0/axio-x86_64-apple-darwin"
      sha256 "58b179a7779efc84fad33064591d12d4b846b304a50efd715405ef675de16826"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/umbra-me/axio/releases/download/v0.3.0/axio-aarch64-unknown-linux-musl"
      sha256 "6c464d60cd2ab2cc4fa42f5c8f033fc1b3730e98c8cb0dba012705e9f92820fd"
    end
    on_intel do
      url "https://github.com/umbra-me/axio/releases/download/v0.3.0/axio-x86_64-unknown-linux-musl"
      sha256 "0a1620e6647dfc78fb933d014d2e65e287688df52d3ad942ddbd65784199d176"
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
