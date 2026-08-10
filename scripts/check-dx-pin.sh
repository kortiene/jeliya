#!/usr/bin/env bash
# AC-5 (first half) of issue #176, as an executable check:
#
#   "CI cannot fetch an unpinned `dx`."
#
# The canonical build uses cargo + a pinned wasm-bindgen and does NOT use `dx`
# (Open Question O-4), so AC-5's "no unpinned dx" is trivially met — but the
# check must still forbid an unpinned fetch anywhere. It fails if any workflow
# or script installs `dioxus-cli`/`dx` without an explicit pinned version, and
# it requires every `wasm-bindgen`/`wasm-bindgen-cli` install to be version-
# pinned so the reproducible build's bindgen cannot drift.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
cd "$repo"

# The actual fetch surfaces: the CI workflows and the canonical build recipe.
# The other `check-*.sh` guards only DEFINE these patterns (to search for them),
# so scanning them would flag the check against itself.
scan_paths=(.github/workflows scripts/build-web.sh)
fail=0

# A diagnostic (`echo`/`printf`), a pattern definition (`grep`), or a comment is
# not an invocation — the check must not flag a help string or a regex that
# merely QUOTES a pinned install command.
is_diagnostic() {
  case "$1" in
    *echo*|*printf*|*grep*) return 0 ;;
  esac
  [[ "$1" =~ ^[[:space:]]*# ]]
}

# A pinned version token: `--version =X` / `--version=X` / `tool@X`, where `X`
# is a literal digit OR a shell variable that holds the locked version.
pinned() {
  grep -Eq -- '--version[= ]=?[0-9$]|@[0-9$]' <<<"$1"
}

# 1. Any dioxus-cli / dx install must be version-pinned.
while IFS= read -r line; do
  is_diagnostic "$line" && continue
  if grep -Eq 'cargo (install|binstall).*dioxus-cli' <<<"$line" && ! pinned "$line"; then
    echo "FAIL: unpinned dioxus-cli install: $line"
    fail=1
  fi
  # A curl/wget of a dx release must carry a pinned version tag.
  if grep -Eq '(curl|wget).*dioxus' <<<"$line" && ! grep -Eq 'v?[0-9]+\.[0-9]+\.[0-9]+' <<<"$line"; then
    echo "FAIL: unpinned dx download: $line"
    fail=1
  fi
done < <(grep -rhnE 'dioxus-cli|(curl|wget).*dioxus' "${scan_paths[@]}" 2>/dev/null || true)

# 2. Every wasm-bindgen(-cli) install must be version-pinned.
while IFS= read -r line; do
  is_diagnostic "$line" && continue
  if grep -Eq 'cargo (install|binstall).*wasm-bindgen' <<<"$line" && ! pinned "$line"; then
    echo "FAIL: unpinned wasm-bindgen-cli install: $line"
    fail=1
  fi
done < <(grep -rhnE 'cargo (install|binstall).*wasm-bindgen' "${scan_paths[@]}" 2>/dev/null || true)

if [ "$fail" -ne 0 ]; then
  echo
  echo "CI must not fetch an unpinned dx or wasm-bindgen (#176 AC-5)."
  exit 1
fi

echo "OK: no unpinned dx/wasm-bindgen fetch in workflows or scripts."
