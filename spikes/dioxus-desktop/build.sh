#!/usr/bin/env bash
# Produce the minimal "packaged" layout for the #159 spike.
#
# Deliberately does NOT use `dx`, for the same reason `spikes/dioxus-web` does
# not: the Dioxus CLI pulls openssl-sys, and pinning `dx` belongs to #176 rather
# than to a throwaway spike. cargo is enough to answer the feasibility question.
#
# "Packaged" here means exactly one thing: the shell finds its daemon by the
# documented resolution order — a bundled `jeliyad` sitting beside the
# executable — instead of by JELIYAD_BIN. That is the property #159 asks about.
# It is NOT a release bundle: no desktop entry, no icons, no .deb, no CMake
# install, no signing. Those belong to M4 (#193/#194), not here.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
profile="${1:-release}"
out="$here/bundle"

case "$profile" in
  release) flags=(--release) ;;
  debug)   flags=() ;;
  *) echo "usage: $0 [release|debug]" >&2; exit 2 ;;
esac

echo "==> building jeliyad ($profile)"
( cd "$repo" && cargo build -p jeliyad "${flags[@]}" )

echo "==> building the shell ($profile)"
( cd "$here" && cargo build "${flags[@]}" )

rm -rf "$out"
mkdir -p "$out"
cp "$here/target/$profile/jeliya-spike-dioxus-desktop" "$out/"
cp "$repo/target/$profile/jeliyad" "$out/"

echo
echo "==> bundle"
find "$out" -type f -printf '%10s  %P\n' | sort -rn
echo
echo "The shell resolves the daemon at \$exeDir/jeliyad. Run it with NO"
echo "JELIYAD_BIN set to exercise that path:"
echo
echo "    ( unset JELIYAD_BIN; $out/jeliya-spike-dioxus-desktop )"
