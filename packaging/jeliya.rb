# Homebrew formula for the `jeliyad` daemon.
#
# Currently filled for v0.4.1. v0.4.0 was the first release under the Jeliya
# name (the project renamed from Bantaba on 2026-07-05; docs/naming.md). Earlier
# releases shipped `bantabad-*` archives and cannot be installed by this formula.
#
# This belongs in a tap, not homebrew-core: publish it as
#   kortiene/homebrew-jeliya  (repo `homebrew-jeliya`, file `Formula/jeliya.rb`)
# then users install with:
#   brew install kortiene/jeliya/jeliya
#
# To update for a new release:
#   1. Set `version` to the release number (no leading "v").
#   2. Replace each sha256 with the value from the matching release sidecar.
#      The release workflow uploads a `<asset>.sha256` next to every archive.
class Jeliya < Formula
  desc "Jeliya peer-to-peer daemon (jeliyad): serves the Jeliya UI over a local WebSocket"
  homepage "https://github.com/kortiene/jeliya"
  version "0.4.1"
  license "MIT OR Apache-2.0"

  on_macos do
    on_arm do
      url "https://github.com/kortiene/jeliya/releases/download/v#{version}/jeliyad-v#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "b997bdc735bc5437c7974f5841d14a56d72c2bd89e3845866137566d30b38698"
    end
    on_intel do
      url "https://github.com/kortiene/jeliya/releases/download/v#{version}/jeliyad-v#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "a91e2cb9e794027d62e0551e9cef0c1fa58f60494801a903084b738480c4bf97"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/kortiene/jeliya/releases/download/v#{version}/jeliyad-v#{version}-aarch64-unknown-linux-musl.tar.gz"
      sha256 "0bebf060bb30e088c056d82a41b1f7bc4317e3ae6daa03c01f4235c79919f8aa"
    end
    on_intel do
      url "https://github.com/kortiene/jeliya/releases/download/v#{version}/jeliyad-v#{version}-x86_64-unknown-linux-musl.tar.gz"
      sha256 "46fbb43762d1077943e30977c02dcd2acd0831903a0d2128aaa6382fb95ca26a"
    end
  end

  def install
    bin.install "jeliyad"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/jeliyad --version")
  end
end
