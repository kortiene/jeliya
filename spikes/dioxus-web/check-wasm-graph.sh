#!/usr/bin/env bash
# Acceptance criterion 1 of issue #158, as an executable check:
#
#   "WASM feature graph contains no native core/Iroh dependency."
#
# The clean-slate architecture record requires the browser client to stay free
# of Iroh and native core (docs/dioxus-architecture.md, Decision 3). This
# asserts it against the resolved wasm32 dependency graph rather than against
# anyone's intent.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$here"

# Crate-name prefixes that must never appear in a browser build.
forbidden=(iroh jeliya-core jeliyad jeliya-ffi quinn rustls tokio-rustls hickory)

graph="$(cargo tree --target wasm32-unknown-unknown --edges normal --prefix none --no-dedupe 2>/dev/null | awk '{print $1}' | sort -u)"

fail=0
for name in "${forbidden[@]}"; do
  if grep -qx -- "$name" <<<"$graph"; then
    echo "FAIL: '$name' is reachable from the wasm32 build"
    cargo tree --target wasm32-unknown-unknown --edges normal --invert "$name" 2>/dev/null | head -20
    fail=1
  fi
done

if [ "$fail" -ne 0 ]; then
  echo
  echo "The browser client must not link native core or Iroh (#158 AC1)."
  exit 1
fi

echo "OK: no native core or Iroh crate in the wasm32 graph."
echo "    $(wc -l <<<"$graph") crates resolved for wasm32-unknown-unknown."
