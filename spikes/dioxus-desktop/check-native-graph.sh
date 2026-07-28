#!/usr/bin/env bash
# Assert the TRANSPORT dependency subtree pulls no TLS backend (issue #159).
#
# The distinction this script exists to keep is narrow and easy to lose:
#
#   * `dioxus-desktop` DOES pull `tungstenite` with `native-tls`, hence
#     `openssl-sys`, non-optionally, on every non-Android target. That is an
#     upstream fact this spike cannot change and does not try to — see README
#     "Negative results".
#   * This spike's OWN transport, `tokio-tungstenite`, must stay TLS-free. It
#     dials loopback, where there is nothing to authenticate a certificate
#     against, and a TLS backend there would be pure attack surface.
#
# So "no TLS in the graph" is already false and checking for it would be
# useless. What is checkable, and what this asserts, is that no TLS crate is
# reachable THROUGH tokio-tungstenite.
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$here"

fail=0
tls_crates=(native-tls openssl openssl-sys rustls tokio-native-tls tokio-rustls rustls-pki-types)

echo "==> transport subtree (tokio-tungstenite)"
subtree="$(cargo tree --package jeliya-spike-dioxus-desktop --invert --edges normal 2>/dev/null | head -0; \
           cargo tree --edges normal --package tokio-tungstenite 2>/dev/null)"
if [[ -z "$subtree" ]]; then
  echo "  ERROR  could not resolve the tokio-tungstenite subtree"; exit 2
fi

for crate in "${tls_crates[@]}"; do
  # Match a crate name at the start of a tree entry, not a substring.
  if echo "$subtree" | grep -qE "[[:space:]]${crate} v[0-9]"; then
    echo "  FAIL   ${crate} is reachable through tokio-tungstenite"
    fail=1
  fi
done
[[ $fail == 0 ]] && echo "  PASS   no TLS backend under the transport"

echo
echo "==> recording the known-unavoidable path (not a failure)"
if cargo tree --invert --edges normal --package openssl-sys 2>/dev/null | grep -q "dioxus-desktop"; then
  echo "  NOTE   openssl-sys reaches the binary via dioxus-desktop -> tungstenite[native-tls]."
  echo "         Upstream, non-optional on non-Android targets. README records it."
else
  echo "  NOTE   openssl-sys is no longer pulled by dioxus-desktop — the README's"
  echo "         'Negative results' section is now STALE and must be updated."
  fail=1
fi

echo
if [[ $fail == 0 ]]; then echo "NATIVE GRAPH OK"; else echo "NATIVE GRAPH CHANGED"; fi
exit $fail
