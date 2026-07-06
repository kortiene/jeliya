# Homebrew formula for the `jeliyad` daemon.
#
# Filled for v0.4.0 — the first release under the Jeliya name (the project
# renamed from Bantaba on 2026-07-05; docs/naming.md). Earlier releases
# shipped `bantabad-*` archives and cannot be installed by this formula.
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
  version "0.4.0"
  license "MIT OR Apache-2.0"

  on_macos do
    on_arm do
      url "https://github.com/kortiene/jeliya/releases/download/v#{version}/jeliyad-v#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "e00692bdf33e80de651d0dff7da8eaade0116d3e82f96d1e5a963526a8bfd144"
    end
    on_intel do
      url "https://github.com/kortiene/jeliya/releases/download/v#{version}/jeliyad-v#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "b5644323dc93be9bc627a98495bc300e4c486c1f5448dd7c4f07ee421aa1dce1"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/kortiene/jeliya/releases/download/v#{version}/jeliyad-v#{version}-aarch64-unknown-linux-musl.tar.gz"
      sha256 "acf7d96515250db4de2b4190ad494e81e09a8847c9c5eda8e8f1ba49220575df"
    end
    on_intel do
      url "https://github.com/kortiene/jeliya/releases/download/v#{version}/jeliyad-v#{version}-x86_64-unknown-linux-musl.tar.gz"
      sha256 "3842f28bf1768e08092b96d4a175ebe4490b285ca9836fe77557e57618d5c9dd"
    end
  end

  def install
    bin.install "jeliyad"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/jeliyad --version")
  end
end
