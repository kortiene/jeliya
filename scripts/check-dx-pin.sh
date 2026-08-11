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
# canonical recipe. Only THIS script is excluded (its pattern definitions
# would self-match); the other check-*.sh helpers are scanned like any
# script — their pattern definitions live in grep invocations, which the
# segment classifier already skips as diagnostics.
# Backslash-newline continuations are joined first: `cargo install --locked \`
# on one line and `dioxus-cli` on the next is one command, and a per-line scan
# would see neither half match the predicate.
scan() {
  find .github/workflows scripts packaging -type f ! -name 'check-dx-pin.sh' -print0 2>/dev/null |
    xargs -0 sed -e ':a' -e '/\\$/{N;s/\\\n/ /;ba' -e '}' 2>/dev/null |
    grep -hE "$1" || true
}
fail=0

# `cargo [+toolchain] [OPTIONS] install` is the documented grammar, and an
# option may carry a SEPARATE value token (`--config KEY=VALUE`) — so any
# tokens are tolerated between `cargo` and the subcommand. Over-matching only
# ever FLAGS more (fail-closed), never less.
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

# A pinned version token: a COMPLETE exact version. `--version` requires the
# `=` exact-version operator plus full x.y.z (a bare requirement like
# `--version 0` is a semver RANGE cargo may resolve to newer tooling);
# `tool@X` requires full x.y.z. The one variable accepted is `$locked_wbg`,
# which build-web.sh derives from Cargo.lock and hard-fails on mismatch.
# The pin must belong to the PROTECTED package: `cargo install a b@x.y.z`
# leaves `a` unpinned even though the segment contains a version. `--version`
# still counts segment-wide because cargo itself rejects --version with more
# than one crate.
pinned_for() { # $1 = package name regex, $2 = segment
  grep -Eq -- "$1@[0-9]+\.[0-9]+\.[0-9]+" <<<"$2" && return 0
  grep -Eq -- "$1@\\$\{?locked_wbg\}?" <<<"$2" && return 0
  grep -Eq -- '--version[= ]=[0-9]+\.[0-9]+\.[0-9]+' <<<"$2" && return 0
  grep -Eq -- '--version[= ]=\$\{?locked_wbg\}?' <<<"$2"
}

# 1. Any dioxus-cli / dx install must be version-pinned.
while IFS= read -r line; do
  while IFS= read -r seg; do
    is_diagnostic "$seg" && continue
    if grep -Eq 'cargo([[:space:]]+[^[:space:]]+)*[[:space:]]+(install|binstall).*dioxus-cli' <<<"$seg" && ! pinned_for 'dioxus-cli' "$seg"; then
      echo "FAIL: unpinned dioxus-cli install: $seg"
      fail=1
    fi
    # A curl/wget of a dx release must carry the pinned version in the
    # FETCHED URL itself — a version token in an output filename or another
    # argument pins nothing about what the remote resource tracks.
    if grep -Eq '(curl|wget).*dioxus' <<<"$seg"; then
      # Each DIOXUS URL must itself carry the version — an unrelated
      # versioned URL in the same transfer list pins nothing about the
      # Dioxus resource, and a dioxus name appearing only in an output
      # filename identifies no fetched resource at all.
      saw_dioxus_url=0
      dioxus_urls_ok=1
      for word in $seg; do
        case "$word" in
          *://*dioxus*)
            saw_dioxus_url=1
            grep -Eq 'v?[0-9]+\.[0-9]+\.[0-9]+' <<<"$word" || dioxus_urls_ok=0
            ;;
        esac
      done
      if [ "$saw_dioxus_url" -ne 1 ] || [ "$dioxus_urls_ok" -ne 1 ]; then
        echo "FAIL: unpinned dx download (every fetched Dioxus URL must carry the version): $seg"
        fail=1
      fi
    fi
  done < <(segments "$line")
done < <(scan 'dioxus-cli|(curl|wget).*dioxus')

# 2. Every wasm-bindgen(-cli) install must be version-pinned.
while IFS= read -r line; do
  while IFS= read -r seg; do
    is_diagnostic "$seg" && continue
    if grep -Eq 'cargo([[:space:]]+[^[:space:]]+)*[[:space:]]+(install|binstall).*wasm-bindgen' <<<"$seg" && ! pinned_for 'wasm-bindgen[a-z-]*' "$seg"; then
      echo "FAIL: unpinned wasm-bindgen-cli install: $seg"
      fail=1
    fi
  done < <(segments "$line")
done < <(scan 'cargo([[:space:]]+[^[:space:]]+)*[[:space:]]+(install|binstall).*wasm-bindgen')

# 3. An action-based install (taiki-e/install-action's `tool:` input) is the
#    same fetch surface through a different door: a `tool:` naming these
#    binaries must carry an explicit `@version` too.
while IFS= read -r line; do
  is_diagnostic "$line" && continue
  # Literal numeric version ONLY: an action input has no validated shell
  # variable, and `@${{ vars.X }}` resolves at workflow time to whatever the
  # repository variable happens to hold — that is not a pin.
  if ! grep -Eq '@[0-9]+\.[0-9]+\.[0-9]+' <<<"$line"; then
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
