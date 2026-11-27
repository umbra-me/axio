# Written by scripts/formula.sh; do not edit by hand.
class Axio < Formula
  desc "Minimal, fast coding agent in one Rust binary"
  homepage "https://github.com/umbra-me/axio"
  version "0.7.0"
  license "Apache-2.0"

  on_macos do
    on_arm do
      url "https://github.com/umbra-me/axio/releases/download/v0.7.0/axio-aarch64-apple-darwin"
      sha256 "bbc79e277a86a3fcf0731286a6e18ddc08756d00e59e4392f9532682ef375948"
    end
    on_intel do
      url "https://github.com/umbra-me/axio/releases/download/v0.7.0/axio-x86_64-apple-darwin"
      sha256 "f5da35d14094ff29926b28b29949bbba2dd20b4076fc731d737eb1da46b8d1e3"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/umbra-me/axio/releases/download/v0.7.0/axio-aarch64-unknown-linux-musl"
      sha256 "746af523d4a8465fff94eda643b621147cecf50d9f5222e0dd001ee60351e1a3"
    end
    on_intel do
      url "https://github.com/umbra-me/axio/releases/download/v0.7.0/axio-x86_64-unknown-linux-musl"
      sha256 "31ec49a4c585fcb821ea48861a526449ec5d986625a79eaf48f733104571a97f"
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
