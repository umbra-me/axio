# Written by scripts/formula.sh; do not edit by hand.
class Axio < Formula
  desc "Minimal, fast coding agent in one Rust binary"
  homepage "https://github.com/umbra-me/axio"
  version "0.1.1"
  license "Apache-2.0"

  on_macos do
    on_arm do
      url "https://github.com/umbra-me/axio/releases/download/v0.1.1/axio-aarch64-apple-darwin"
      sha256 "a120493449459ed5b577ea7b0f384f921853895605214ed7f0af4b15d4249d11"
    end
    on_intel do
      url "https://github.com/umbra-me/axio/releases/download/v0.1.1/axio-x86_64-apple-darwin"
      sha256 "6211f6552e74c3424b9ee0dac2cbfebbc95f6af1b67a39d427291ae184f2c043"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/umbra-me/axio/releases/download/v0.1.1/axio-aarch64-unknown-linux-musl"
      sha256 "2f3547502e162f176043388911556ce08332c26b3b74377debebbb12720d8b79"
    end
    on_intel do
      url "https://github.com/umbra-me/axio/releases/download/v0.1.1/axio-x86_64-unknown-linux-musl"
      sha256 "ef8eaad7eb17d90f56268edc322664e5c9ac43180075f43755c8c07c9f9205e8"
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
