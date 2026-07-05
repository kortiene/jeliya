# Homebrew formula for the `jeliyad` daemon.
#
# NOT YET INSTALLABLE: the project was renamed Bantaba → Jeliya on 2026-07-05
# (docs/naming.md). Releases v0.1.0/v0.2.0 shipped `bantabad-*` archives under
# the old name; this formula looks for `jeliyad-<tag>-<target>` assets, so the
# `version` and sha256 values below are stale old-name leftovers. It becomes
# installable at the first post-rename release — copy that tag's version and
# four sha256 values in. See packaging/README.md ("Release status").
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
  version "0.2.0"
  license "MIT OR Apache-2.0"

  on_macos do
    on_arm do
      url "https://github.com/kortiene/jeliya/releases/download/v#{version}/jeliyad-v#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "485cf9d5a86020fa7cbfb2ed6ce1f33a8f9090374eafe24d65ddca479f858d84"
    end
    on_intel do
      url "https://github.com/kortiene/jeliya/releases/download/v#{version}/jeliyad-v#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "59ca2968cb2a0b8bb952ccbd1ea2801d5fd7c61bddf77971be0c8b3b427eeb60"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/kortiene/jeliya/releases/download/v#{version}/jeliyad-v#{version}-aarch64-unknown-linux-musl.tar.gz"
      sha256 "f58ab4b597c5b50707e26708deca66856fd542ffa3e2dd14a8bca3a019ff63a0"
    end
    on_intel do
      url "https://github.com/kortiene/jeliya/releases/download/v#{version}/jeliyad-v#{version}-x86_64-unknown-linux-musl.tar.gz"
      sha256 "13d7ca5c86ebaf02b03e9ccaa77cfda40795f1ba6fb56cc8d130a9fb4f869896"
    end
  end

  def install
    bin.install "jeliyad"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/jeliyad --version")
  end
end
