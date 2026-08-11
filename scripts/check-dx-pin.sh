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
# would see neither half match the predicate. YAML MULTILINE SCALARS are the
# same trap through a different syntax, twice over: a FOLDED block (`run: >`,
# with or without a trailing YAML comment) and a PLAIN multiline scalar
# (`run: cargo install` with `dioxus-cli` on the next, deeper-indented line)
# both execute as the single space-joined command, so both are space-joined
# per YAML file before scanning — exactly the YAML folding semantics. A
# plain-scalar continuation line is one that is neither a `key:` mapping
# entry, a `- ` list item, a comment, nor blank. LITERAL blocks (`run: |`)
# are NOT joined: they execute line by line, so the per-line scan already
# sees each command as executed, and joining them would let a leading `echo`
# line launder a later install through the diagnostic classifier. Only
# *.yml/*.yaml files get the YAML joins: shell lines are separate commands,
# and joining them would launder the same way.
# All matching is CASE-INSENSITIVE: a download URL spelling the project
# `DioxusLabs/Dioxus` names the same fetch surface, and over-matching only
# ever FLAGS more (fail-closed), never less.
# LIMITS (accepted, recorded): this is a TEXT scan of reviewed files. A tool
# name routed through a shell variable, an Actions `${{ ... }}` expression,
# or a matrix value cannot be traced to its value — reviewed workflows and
# scripts must name these tools literally, and review owns that rule; the
# scan defends against accidents, not an adversarial committer (the same
# stance as the embed guard's, where #183's sealed manifest owns integrity).
scan() {
  find .github/workflows scripts packaging -type f ! -name 'check-dx-pin.sh' -print0 2>/dev/null |
    while IFS= read -r -d '' f; do
      case "$f" in
        *.yml|*.yaml) yaml=1 ;;
        *) yaml=0 ;;
      esac
      awk -v yaml="$yaml" -v q="'" '
        # The run-key detector tolerates any list-dash spacing and quoted
        # keys: `-  run:` (two spaces) and `- "run":` are the same executed
        # step, and misreading them as a NON-run key would JOIN a literal
        # shell block — exactly the laundering the run split prevents.
        BEGIN { runre = "^[[:space:]]*(-[[:space:]]+)?[\"" q "]?run[\"" q "]?[[:space:]]*:" }
        function flush() { if (mode) { print buf; mode = 0 } }
        function joinline(l) { sub(/^[[:space:]]+/, "", l); buf = buf " " l }
        {
          if (mode == 2) {
            # A lone block-scalar header on the line after the key re-routes
            # the block: a fold marker joins like any fold; a literal marker
            # keeps per-line semantics for `run:` (shell lines are separate
            # commands) but JOINS for any other key — a literal ACTION INPUT
            # (`tool: |` + indented name) is one value, and per-line printing
            # would hide the name from the tool-pin scan. Header forms carry
            # an optional indentation digit and chomp indicator (`>2`, `>-`).
            if ($0 ~ /^[[:space:]]*>[0-9+-]*[[:space:]]*(#.*)?$/) { mode = 1; next }
            if ($0 ~ /^[[:space:]]*\|[0-9+-]*[[:space:]]*(#.*)?$/) {
              if (runkey) { flush(); print; next }
              mode = 1; next
            }
          }
          if (mode == 1) { # folded block: blank lines stay inside the fold
            if ($0 ~ /^[[:space:]]*$/) next
            match($0, /[^[:space:]]/)
            if (RSTART > basecol) { joinline($0); next }
            flush()
          } else if (mode == 2) { # plain scalar: deeper non-key/list/comment lines continue it
            if ($0 ~ /^[[:space:]]*$/) { flush() } else {
              match($0, /[^[:space:]]/)
              if (RSTART > basecol && $0 !~ /^[[:space:]]*#/ &&
                  $0 !~ /^[[:space:]]*-([[:space:]]|$)/ &&
                  $0 !~ /^[[:space:]]*[^[:space:]:#][^:]*:([[:space:]]|$)/) {
                joinline($0); next
              }
              flush()
            }
          }
          if (yaml && $0 ~ /:[[:space:]]*>[0-9+-]*[[:space:]]*(#.*)?$/) {
            mode = 1
            match($0, /[^[:space:]]/); basecol = RSTART
            buf = $0
            sub(/[[:space:]]*>[0-9+-]*[[:space:]]*(#.*)?$/, "", buf)
            next
          }
          if (yaml && $0 ~ /:[[:space:]]*\|[0-9+-]*[[:space:]]*(#.*)?$/) {
            # Literal marker on the key line: same run-vs-input split as the
            # own-line dispatch above.
            if ($0 ~ runre) { print; next }
            mode = 1
            match($0, /[^[:space:]]/); basecol = RSTART
            buf = $0
            sub(/[[:space:]]*\|[0-9+-]*[[:space:]]*(#.*)?$/, "", buf)
            next
          }
          if (yaml && $0 ~ /^[[:space:]]*(- )?[^[:space:]:#][^:]*:[[:space:]]+[^|>[:space:]]/) {
            mode = 2
            runkey = ($0 ~ runre)
            match($0, /[^[:space:]]/); basecol = RSTART
            buf = $0
            next
          }
          if (yaml && $0 ~ /^[[:space:]]*(- )?[^[:space:]:#][^:]*:[[:space:]]*(#.*)?$/) {
            # Empty-valued key: the value may arrive on continuation lines
            # (a plain multiline scalar, or a block header on its own line —
            # both handled by mode 2 above). Nested mappings and lists are
            # excluded by the continuation rules and flush unjoined.
            mode = 2
            runkey = ($0 ~ runre)
            match($0, /[^[:space:]]/); basecol = RSTART
            buf = $0
            sub(/[[:space:]]*#.*$/, "", buf)
            next
          }
          print
        }
        END { flush() }
      ' "$f" 2>/dev/null
    done |
    sed -e ':a' -e '/\\$/{N;s/\\\n/ /;ba' -e '}' 2>/dev/null |
    grep -hiE "$1" || true
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
# A whitespace-preceded `#` starts a comment the executing layer discards —
# shell and YAML alike — so a version mentioned after it (`cargo install
# wasm-bindgen-cli # pin later at @0.2.126`) pins NOTHING about what runs.
# Strip the tail before classification so a comment cannot launder a pin.
# Whitespace-preceded only: `${var#pat}` expansions and URL fragments carry
# no space before their `#`, and a real version token never does either. A
# WHOLE-LINE comment is dropped outright — it runs to end of line, so an
# operator inside it must not mint a non-`#`-prefixed segment that dodges
# the diagnostic classifier.
strip_comment_tail() {
  sed -e 's/^[[:space:]]*#.*$//' -e 's/[[:space:]]#.*$//' <<<"$1"
}
# Comment handling ORDER is load-bearing: whole-line comments are dropped
# before splitting (the comment spans the line, so an operator inside it
# must not mint scannable segments), but segment-LOCAL tails are stripped
# only AFTER splitting — a quoted `#` inside one command (`echo "a # b" &&
# cargo install dioxus-cli`) is not a shell comment, and stripping the whole
# line first deleted the real install that followed it. The residual cost is
# accepted and fail-closed both ways: a quoted `#` truncates only its own
# segment, and text after a real mid-line comment is still scanned
# (over-flagging at worst, never laundering a later command).
segments() {
  case "$(sed 's/^[[:space:]-]*//' <<<"$1")" in '#'*) return 0 ;; esac
  awk '{ n = split($0, s, /&&|\|\||;|\|/); for (i = 1; i <= n; i++) print s[i] }' <<<"$1" |
    sed 's/[[:space:]]#.*$//'
}
is_diagnostic() {
  # No exemption for a segment carrying an executable substitution: the
  # shell runs `$(cargo install dioxus-cli)` BEFORE the enclosing echo —
  # and process substitution (`<(...)`, `>(...)`) executes just the same —
  # so a diagnostic invocation cannot launder what its arguments execute.
  case "$1" in *'$('* | *'`'* | *'<('* | *'>('*) return 1 ;; esac
  case "$(sed 's/^[[:space:]-]*//' <<<"$1")" in
    '#'*|echo|echo\ *|printf|printf\ *|grep|grep\ *) return 0 ;;
  esac
  return 1
}

# A pinned version token: a COMPLETE exact version. `--version` requires the
# `=` exact-version operator plus full x.y.z (a bare requirement like
# `--version 0` is a semver RANGE cargo may resolve to newer tooling);
# `tool@X` requires full x.y.z. `--version` counts segment-wide because
# cargo itself rejects --version with more than one crate; an `@` pin must
# sit on EVERY protected crate operand itself — `cargo install a b@x.y.z`
# leaves `a` unpinned, and a pinned SIBLING (`wasm-bindgen-macro@x.y.z`)
# pins nothing about `wasm-bindgen-cli` in the same command. The one
# variable accepted is `$locked_wbg`, which build-web.sh derives from
# Cargo.lock and hard-fails on mismatch — matched exactly and
# case-sensitively: `$LOCKED_WBG`, `$locked_wbg_evil`, and
# `${locked_wbg:-latest}` are all DIFFERENT, unvalidated expansions. If the
# segment names the package but no operand-shaped occurrence can be
# validated (exotic quoting, indirection), it fails closed as unpinned.
pinned_for() { # $1 = package name regex, $2 = segment
  # A GIT source resolves the repository's moving default branch no matter
  # what package version is requested — `--git URL tool@x.y.z` still fetches
  # whatever the branch points at today. Only an immutable full-SHA `--rev`
  # pins a git install; without one the segment is unpinned regardless of
  # any version token below.
  if grep -Eiq -- '--git' <<<"$2"; then
    grep -Eq -- '--rev[= ][0-9a-f]{40}([^0-9a-f]|$)' <<<"$2" || return 1
  fi
  grep -Eiq -- '--version[= ]=[0-9]+\.[0-9]+\.[0-9]+' <<<"$2" && return 0
  grep -Eq -- '--version[= ]=(\$locked_wbg([^A-Za-z0-9_]|$)|\$\{locked_wbg\})' <<<"$2" && return 0
  found=0
  set -f
  for word in $2; do
    word="${word//[\"\']/}"
    case "$word" in -*|'') continue ;; esac
    if grep -Eiq -- "^$1(@|$)" <<<"$word"; then
      found=1
      # An exact pre-release/build pin (`@0.7.0-alpha.3`) is as complete a
      # version as `@x.y.z` — cargo installs exactly it.
      grep -Eiq -- "^$1@[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$" <<<"$word" && continue
      grep -Eq -- '@\$(\{locked_wbg\}|locked_wbg)$' <<<"$word" && continue
      set +f
      return 1
    fi
  done
  set +f
  [ "$found" -eq 1 ]
}

# 1. Any dioxus-cli / dx install must be version-pinned.
while IFS= read -r line; do
  while IFS= read -r seg; do
    is_diagnostic "$seg" && continue
    if grep -Eiq 'cargo([[:space:]]+[^[:space:]]+)*[[:space:]]+(install|binstall).*dioxus-cli' <<<"$seg" && ! pinned_for 'dioxus-cli' "$seg"; then
      echo "FAIL: unpinned dioxus-cli install: $seg"
      fail=1
    fi
    # A curl/wget of a dx OR wasm-bindgen release must carry the pinned
    # version in the FETCHED URL itself — a version token in an output
    # filename or another argument pins nothing about what the remote
    # resource tracks, and a direct binary download is the same fetch
    # surface as a cargo install.
    if grep -Eiq '(curl|wget).*(dioxus|wasm-bindgen)' <<<"$seg"; then
      # Each PROTECTED URL must itself carry the version — an unrelated
      # versioned URL in the same transfer list pins nothing about the
      # protected resource, and a protected name appearing only in an
      # output filename identifies no fetched resource at all. Words are
      # lowercased first: `DioxusLabs/Dioxus` in a download URL names the
      # same fetched resource.
      saw_protected_url=0
      protected_urls_ok=1
      for word in $seg; do
        case "${word,,}" in
          *://*dioxus* | *://*wasm-bindgen*)
            saw_protected_url=1
            # The version must sit in the PATH of the fetched resource: a
            # numeric host (`http://1.2.3.4/...`) or a versioned CDN prefix
            # pins nothing about what the path tracks — and a path through
            # `latest` is by definition unpinned no matter what version
            # tokens surround it.
            url_path="${word,,}"
            url_path="${url_path#*://}"
            url_path="${url_path#*/}"
            case "$url_path" in *latest*) protected_urls_ok=0 ;; *)
              grep -Eq 'v?[0-9]+\.[0-9]+\.[0-9]+' <<<"$url_path" || protected_urls_ok=0
              ;;
            esac
            ;;
        esac
      done
      if [ "$saw_protected_url" -ne 1 ] || [ "$protected_urls_ok" -ne 1 ]; then
        echo "FAIL: unpinned dx/wasm-bindgen download (every fetched Dioxus or wasm-bindgen URL must carry the version): $seg"
        fail=1
      fi
    fi
  done < <(segments "$line")
done < <(scan 'dioxus-cli|(curl|wget).*(dioxus|wasm-bindgen)')

# 2. Every wasm-bindgen(-cli) install must be version-pinned.
while IFS= read -r line; do
  while IFS= read -r seg; do
    is_diagnostic "$seg" && continue
    if grep -Eiq 'cargo([[:space:]]+[^[:space:]]+)*[[:space:]]+(install|binstall).*wasm-bindgen' <<<"$seg" && ! pinned_for 'wasm-bindgen[a-z-]*' "$seg"; then
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
  # repository variable happens to hold — that is not a pin. The version must
  # survive YAML comment stripping: in `tool: wasm-bindgen-cli # @0.2.126`
  # the action receives only the unversioned name, so a version the raw line
  # carries after `#` pins nothing. The input is a comma/newline-separated
  # LIST (multiline forms arrive here joined), so EVERY protected token must
  # carry its own trailing pin — a pinned neighbor pins nothing. The key
  # itself may be QUOTED (`"tool":` is the same YAML mapping key), so the
  # feed accepts optional quotes and space before the colon — and it may sit
  # inside a FLOW mapping (`with: { tool: wasm-bindgen-cli }`), so a `tool:`
  # reached through `{` or `,` is scanned identically (the per-token loop
  # below ignores the brace tokens).
  val="$(sed 's/^[^:]*://' <<<"$(strip_comment_tail "$line")")"
  set -f
  for tok in $(tr ',' ' ' <<<"$val"); do
    # YAML strips the quotes before the action sees the name — the scan
    # must too, or `tool: "wasm-bindgen-cli"` dodges the protected-name
    # anchor while installing the unpinned tool. Exact pre-release pins
    # (`@0.7.0-alpha.3`) are complete versions and pass.
    tok="${tok//[\"\']/}"
    if grep -Eiq '^(dioxus-cli|wasm-bindgen)' <<<"$tok" &&
      ! grep -Eq '@[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$' <<<"$tok"; then
      echo "FAIL: unpinned action-based tool install: $line"
      fail=1
    fi
  done
  set +f
done < <(scan '(^[[:space:]]*|[{,][[:space:]]*)["'\'']?tool["'\'']?[[:space:]]*:.*(dioxus-cli|wasm-bindgen)')

if [ "$fail" -ne 0 ]; then
  echo
  echo "CI must not fetch an unpinned dx or wasm-bindgen (#176 AC-5)."
  exit 1
fi

echo "OK: no unpinned dx/wasm-bindgen fetch in workflows or scripts."
