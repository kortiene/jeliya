#!/usr/bin/env bash
# AC-5 (second half) of issue #176, as an executable check:
#
#   "CI ... [does not] depend on React/Flutter tooling."
#
# The canonical Dioxus build recipe (scripts/build-web.sh) must not run `npm`,
# `vite`, `tsc`, `flutter`, or `dart`, and must not read `ui/dist` (React
# output). Accidental consumption of React output as the canonical artifact
# fails closed. This scans the build recipe itself.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
cd "$repo"

recipe="$repo/scripts/build-web.sh"
if [ ! -f "$recipe" ]; then
  echo "FAIL: canonical build recipe $recipe is missing" >&2
  exit 1
fi

fail=0
# Match invocations, not prose: comment lines (which describe the guard itself)
# are stripped first, then a word boundary is required before the tool name.
# The TRAILING boundary is any non-identifier character or end of line — the
# shell separates `npm<TAB>ci`, `npm;`, and `npm|` exactly like `npm ci`, so
# a literal-space-only boundary would launder a real invocation.
# `ui/dist` is forbidden as an input in any form, but `jeliya-ui/dist` (the
# Dioxus output) is not `ui/dist` and must not match.
code="$(grep -vE '^[[:space:]]*#' "$recipe")"
while IFS= read -r pattern; do
  if grep -Eq "$pattern" <<<"$code"; then
    echo "FAIL: canonical build recipe references forbidden tooling: /$pattern/"
    grep -nE "$pattern" <<<"$code" || true
    fail=1
  fi
done <<'PATTERNS'
(^|[^[:alnum:]_])npm([^[:alnum:]_-]|$)
(^|[^[:alnum:]_])npx([^[:alnum:]_-]|$)
(^|[^[:alnum:]_])vite([^[:alnum:]_-]|$)
(^|[^[:alnum:]_])tsc([^[:alnum:]_-]|$)
(^|[^[:alnum:]_])flutter([^[:alnum:]_-]|$)
(^|[^[:alnum:]_])dart([^[:alnum:]_-]|$)
(^|[^[:alnum:]_-])ui/dist
(^|[^[:alnum:]_-])node_modules
PATTERNS

if [ "$fail" -ne 0 ]; then
  echo
  echo "The canonical Dioxus build must not depend on React/Vite/Flutter tooling"
  echo "or consume ui/dist (#176 AC-5)."
  exit 1
fi

echo "OK: the canonical web build uses no npm/vite/tsc/flutter/dart and no ui/dist."
