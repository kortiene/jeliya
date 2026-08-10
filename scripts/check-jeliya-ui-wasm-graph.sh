#!/usr/bin/env bash
# AC-2 of issue #176, as an executable check:
#
#   "WASM graph excludes Iroh/native crates."
#
# Generalized from spikes/dioxus-web/check-wasm-graph.sh to the production crate
# and its `web` renderer feature. The clean-slate architecture record requires
# the browser client to stay free of Iroh and every native crate
# (docs/dioxus-architecture.md, Decision 3). This asserts it against the
# resolved wasm32 dependency graph rather than against anyone's intent.
# `cargo tree` resolves the graph for the target without compiling it, so this
# runs even where the wasm std target is not installed.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
cd "$repo"

# Crate-name prefixes that must never appear in a browser build: Iroh, native
# core/daemon/FFI, the QUIC/TLS stack, the async runtime, the DNS resolver, the
# native WebView (`wry`/`tao`), OpenSSL, and any WebSocket/native-TLS crate.
forbidden=(
  iroh jeliya-core jeliyad jeliya-ffi
  quinn rustls tokio-rustls hickory tokio
  wry tao openssl-sys native-tls tungstenite tokio-tungstenite
)

graph="$(cargo tree -p jeliya-ui --features web \
  --target wasm32-unknown-unknown --edges normal --prefix none --no-dedupe \
  2>/dev/null | awk '{print $1}' | sort -u)"

fail=0
for name in "${forbidden[@]}"; do
  if grep -qx -- "$name" <<<"$graph"; then
    echo "FAIL: '$name' is reachable from the jeliya-ui wasm32 'web' build"
    cargo tree -p jeliya-ui --features web \
      --target wasm32-unknown-unknown --edges normal --invert "$name" 2>/dev/null | head -20
    fail=1
  fi
done

if [ "$fail" -ne 0 ]; then
  echo
  echo "The browser client must not link native core or Iroh (#176 AC-2)."
  exit 1
fi

echo "OK: no native core or Iroh crate in the jeliya-ui wasm32 'web' graph."
echo "    $(wc -l <<<"$graph") crates resolved for wasm32-unknown-unknown."
