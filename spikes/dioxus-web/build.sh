#!/usr/bin/env bash
# Build the disposable Dioxus web slice (issue #158) into ./dist.
#
# Deliberately does NOT use `dx`: the Dioxus CLI pulls openssl-sys, which needs
# system OpenSSL headers this machine does not have, and pinning `dx` belongs to
# #176 rather than to a throwaway spike. cargo + wasm-bindgen is enough to
# answer the feasibility question. Bundle numbers recorded from this build are
# NOT wasm-opt'd — say so wherever they are quoted.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
crate=jeliya_spike_dioxus_web
out="$here/dist"

echo "==> cargo build (release, wasm32-unknown-unknown)"
cd "$here"
cargo build --release --target wasm32-unknown-unknown

echo "==> wasm-bindgen"
rm -rf "$out"
mkdir -p "$out"
wasm-bindgen \
  --target web \
  --no-typescript \
  --out-dir "$out" \
  "$here/target/wasm32-unknown-unknown/release/$crate.wasm"

echo "==> assets"
cp "$here/index.html" "$out/index.html"
# Verbatim, unmodified — the point is to test whether it carries over as-is.
cp "$repo/ui/src/styles.css" "$out/styles.css"

echo
echo "==> bundle (no wasm-opt)"
find "$out" -type f -printf '%10s  %P\n' | sort -rn
total=$(find "$out" -type f -printf '%s\n' | awk '{t+=$1} END{print t}')
echo "$(printf '%10d' "$total")  TOTAL"
if command -v gzip >/dev/null; then
  gz=$(cat "$out/$crate"_bg.wasm | gzip -9 -c | wc -c)
  echo "$(printf '%10d' "$gz")  ${crate}_bg.wasm (gzip -9)"
fi
