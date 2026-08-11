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

# The actual fetch surfaces: every workflow, every script, and the packaging
# helpers — the contract forbids an unpinned fetch ANYWHERE, not only in the
# canonical recipe. The `check-*.sh` guards only DEFINE these patterns (to
# search for them), so they are excluded rather than flagged against
# themselves.
# Backslash-newline continuations are joined first: `cargo install --locked \`
# on one line and `dioxus-cli` on the next is one command, and a per-line scan
# would see neither half match the predicate.
scan() {
  find .github/workflows scripts packaging -type f ! -name 'check-*.sh' -print0 2>/dev/null |
    xargs -0 sed -e ':a' -e '/\\$/{N;s/\\\n/ /;ba' -e '}' 2>/dev/null |
    grep -hE "$1" || true
}
fail=0

# A joined line may chain several commands (`cargo install x && echo done`);
# each segment is classified on its own so a diagnostic tail cannot launder a
# real install — substring matching used to skip the whole line. A segment is
# diagnostic only when its INVOCATION is echo/printf/grep, or it is a comment.
segments() {
  awk '{ n = split($0, s, /&&|\|\||;|\|/); for (i = 1; i <= n; i++) print s[i] }' <<<"$1"
}
is_diagnostic() {
  case "$(sed 's/^[[:space:]-]*//' <<<"$1")" in
    '#'*|echo|echo\ *|printf|printf\ *|grep|grep\ *) return 0 ;;
  esac
  return 1
}

# A pinned version token: `--version =X` / `--version=X` / `tool@X`, where `X`
# is a literal digit — or exactly `$locked_wbg`, the ONE variable accepted,
# because build-web.sh derives it from Cargo.lock and hard-fails on mismatch.
# Any other variable could be sourced dynamically (even "latest") and is not
# a pin.
pinned() {
  grep -Eq -- '--version[= ]=?[0-9]|@[0-9]' <<<"$1" && return 0
  grep -Eq -- '(--version[= ]=?|@)\$\{?locked_wbg\}?' <<<"$1"
}

# 1. Any dioxus-cli / dx install must be version-pinned.
while IFS= read -r line; do
  while IFS= read -r seg; do
    is_diagnostic "$seg" && continue
    if grep -Eq 'cargo (install|binstall).*dioxus-cli' <<<"$seg" && ! pinned "$seg"; then
      echo "FAIL: unpinned dioxus-cli install: $seg"
      fail=1
    fi
    # A curl/wget of a dx release must carry a pinned version tag.
    if grep -Eq '(curl|wget).*dioxus' <<<"$seg" && ! grep -Eq 'v?[0-9]+\.[0-9]+\.[0-9]+' <<<"$seg"; then
      echo "FAIL: unpinned dx download: $seg"
      fail=1
    fi
  done < <(segments "$line")
done < <(scan 'dioxus-cli|(curl|wget).*dioxus')

# 2. Every wasm-bindgen(-cli) install must be version-pinned.
while IFS= read -r line; do
  while IFS= read -r seg; do
    is_diagnostic "$seg" && continue
    if grep -Eq 'cargo (install|binstall).*wasm-bindgen' <<<"$seg" && ! pinned "$seg"; then
      echo "FAIL: unpinned wasm-bindgen-cli install: $seg"
      fail=1
    fi
  done < <(segments "$line")
done < <(scan 'cargo (install|binstall).*wasm-bindgen')

# 3. An action-based install (taiki-e/install-action's `tool:` input) is the
#    same fetch surface through a different door: a `tool:` naming these
#    binaries must carry an explicit `@version` too.
while IFS= read -r line; do
  is_diagnostic "$line" && continue
  # Literal numeric version ONLY: an action input has no validated shell
  # variable, and `@${{ vars.X }}` resolves at workflow time to whatever the
  # repository variable happens to hold — that is not a pin.
  if ! grep -Eq '@[0-9]' <<<"$line"; then
    echo "FAIL: unpinned action-based tool install: $line"
    fail=1
  fi
done < <(scan '^[[:space:]]*tool:.*(dioxus-cli|wasm-bindgen)')

if [ "$fail" -ne 0 ]; then
  echo
  echo "CI must not fetch an unpinned dx or wasm-bindgen (#176 AC-5)."
  exit 1
fi

echo "OK: no unpinned dx/wasm-bindgen fetch in workflows or scripts."
