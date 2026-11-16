# Written by scripts/formula.sh; do not edit by hand.
class Axio < Formula
  desc "Minimal, fast coding agent in one Rust binary"
  homepage "https://github.com/umbra-me/axio"
  version "0.2.0"
  license "Apache-2.0"

  on_macos do
    on_arm do
      url "https://github.com/umbra-me/axio/releases/download/v0.2.0/axio-aarch64-apple-darwin"
      sha256 "7b32aa464d5aa0c4545781af78fbf721b8c533a12ed65c21039a153773c54074"
    end
    on_intel do
      url "https://github.com/umbra-me/axio/releases/download/v0.2.0/axio-x86_64-apple-darwin"
      sha256 "c21d7099badb1ce12267fb04729321a41b9078c48f6668c322fa692256021306"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/umbra-me/axio/releases/download/v0.2.0/axio-aarch64-unknown-linux-musl"
      sha256 "1e675d6e3e43600285d93f8260a912f5f3d7fbb85fb6c3859cbddbf16213fecf"
    end
    on_intel do
      url "https://github.com/umbra-me/axio/releases/download/v0.2.0/axio-x86_64-unknown-linux-musl"
      sha256 "e86196db200179076af7ae3e7faf3cf5204e1be5ff49c73a990b9b98cd8ef681"
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
