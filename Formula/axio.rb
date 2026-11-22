# Written by scripts/formula.sh; do not edit by hand.
class Axio < Formula
  desc "Minimal, fast coding agent in one Rust binary"
  homepage "https://github.com/umbra-me/axio"
  version "0.4.0"
  license "Apache-2.0"

  on_macos do
    on_arm do
      url "https://github.com/umbra-me/axio/releases/download/v0.4.0/axio-aarch64-apple-darwin"
      sha256 "82130a988ef3f9f328079eae836677dc2f24a7e4aa5a8115217931dac2357a2a"
    end
    on_intel do
      url "https://github.com/umbra-me/axio/releases/download/v0.4.0/axio-x86_64-apple-darwin"
      sha256 "e3f81df62d46dcca96ffaff91ff2d284d8c1a5978be1c0bd794070bbe8ed36d7"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/umbra-me/axio/releases/download/v0.4.0/axio-aarch64-unknown-linux-musl"
      sha256 "01352756a3da8a854493667dd56922b8f0f0756cb7a8714c83223bdf7b2b943a"
    end
    on_intel do
      url "https://github.com/umbra-me/axio/releases/download/v0.4.0/axio-x86_64-unknown-linux-musl"
      sha256 "31d1e6707647771c3475950154038f0f384b669621f0e56b77856151fe5a3cfe"
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
