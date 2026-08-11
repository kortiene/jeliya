#!/usr/bin/env bash
# Build the ONE canonical, reproducible Dioxus web artifact (#176) into
# crates/jeliya-ui/dist (or $1).
#
# Canonical toolchain (Open Question O-4): `cargo build --target
# wasm32-unknown-unknown` + a PINNED `wasm-bindgen-cli` whose version MUST equal
# the `wasm-bindgen` the workspace `Cargo.lock` resolves, or the generated
# bindings and the compiled module disagree and the app fails to boot. This is
# the path the #158 spike proved in this repo; it needs no `dx` and therefore no
# system OpenSSL. It runs `npm`/`vite`/`tsc`/`flutter`/`dart` NOWHERE and never
# reads `ui/dist` (asserted by scripts/check-web-build-toolchain.sh).
#
# Determinism (§5): two clean-checkout builds on the same pinned toolchain
# produce a byte-identical asset tree. Path prefixes are remapped, locale is
# fixed, no build timestamp is embedded, and `wasm-opt` is NOT run (size budgets
# are #198). scripts/check-web-determinism.sh verifies byte-equality.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
cd "$repo"

crate=jeliya_ui_web
out="${1:-$repo/crates/jeliya-ui/dist}"

# Reproducibility controls. A fixed SOURCE_DATE_EPOCH and remapped path prefix
# keep absolute paths and timestamps out of the binary; LC_ALL=C fixes any
# collation the pipeline performs; incremental compilation is off so output does
# not depend on prior build state.
export LC_ALL=C
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-1700000000}"
export CARGO_INCREMENTAL=0
# The canonical flags are EXACTLY these — caller RUSTFLAGS (say, -C
# debuginfo=1) would change the emitted wasm while the marker records only
# the standard pins, and the determinism check runs both of its samples under
# the same inherited environment, so it cannot catch an environment-specific
# artifact. CARGO_ENCODED_RUSTFLAGS outranks RUSTFLAGS in cargo's precedence
# and is cleared for the same reason — as are RUSTC and its wrappers, which
# would make cargo invoke a compiler other than the one the pin below
# validates (rustc on PATH), letting a noncanonical compiler pass every gate.
unset RUSTFLAGS RUSTC RUSTC_WRAPPER RUSTC_WORKSPACE_WRAPPER
# Cargo also accepts profile, rustflags, and compiler overrides from the
# environment (CARGO_PROFILE_RELEASE_OPT_LEVEL=0, CARGO_BUILD_RUSTFLAGS,
# CARGO_TARGET_<T>_RUSTFLAGS, CARGO_BUILD_RUSTC and its wrapper forms, ...) —
# each changes the emitted wasm or the invoked compiler while the marker
# records only the standard pins, invisible to the determinism check for the
# same both-samples-inherit-it reason. Strip them all.
for cargo_var in $(compgen -e | grep -E '^CARGO_(PROFILE_|BUILD_RUSTFLAGS$|BUILD_RUSTDOCFLAGS$|BUILD_RUSTC(_WRAPPER|_WORKSPACE_WRAPPER)?$|TARGET_.*_RUSTFLAGS$)' || true); do
  unset "$cargo_var"
done
# Space-safe flag delivery: RUSTFLAGS is whitespace-split, so a checkout or
# home directory containing a space would shear a remap argument apart and
# fail the build. CARGO_ENCODED_RUSTFLAGS is unit-separator-delimited for
# exactly this; it also outranks RUSTFLAGS, so setting it doubles as the
# override-channel pin.
US=$'\x1f'
export CARGO_ENCODED_RUSTFLAGS="--remap-path-prefix=${repo}=.${US}--remap-path-prefix=${HOME}=~"

# Cargo CONFIG FILES are one more override channel: $CARGO_HOME/config.toml
# can set build.rustc or [profile.release] keys that bypass every
# environment-level pin above, invisibly to the determinism check (both of
# its samples read the same config). Build under a fresh CARGO_HOME whose
# config is empty, sharing the real registry/git caches by symlink so
# nothing re-downloads. A committed repo-level .cargo/config.toml (none
# exists today) would still apply, as a reviewed file should.
real_cargo_home="${CARGO_HOME:-$HOME/.cargo}"
iso_cargo_home="$(mktemp -d)"
trap 'rm -rf "$iso_cargo_home"' EXIT
for shared in registry git; do
  if [ -d "$real_cargo_home/$shared" ]; then
    ln -s "$real_cargo_home/$shared" "$iso_cargo_home/$shared"
  fi
done
export CARGO_HOME="$iso_cargo_home"

# The wasm-bindgen CLI version MUST match the locked library version exactly.
locked_wbg="$(awk '
  $1=="name" && $3=="\"wasm-bindgen\"" {found=1; next}
  found && $1=="version" {gsub(/"/,"",$3); print $3; exit}
' "$repo/Cargo.lock")"
if [ -z "$locked_wbg" ]; then
  echo "FAIL: could not read the locked wasm-bindgen version from Cargo.lock" >&2
  exit 1
fi
if ! command -v wasm-bindgen >/dev/null; then
  echo "FAIL: wasm-bindgen ($locked_wbg) is not installed." >&2
  echo "      cargo install --locked --version =$locked_wbg wasm-bindgen-cli" >&2
  exit 1
fi
cli_wbg="$(wasm-bindgen --version | awk '{print $2}')"
if [ "$cli_wbg" != "$locked_wbg" ]; then
  echo "FAIL: wasm-bindgen CLI $cli_wbg != locked wasm-bindgen $locked_wbg" >&2
  echo "      cargo install --locked --version =$locked_wbg wasm-bindgen-cli" >&2
  exit 1
fi

# The canonical artifact is a function of the exact compiler, not just the
# lockfile: the workspace `rust-version = 1.91` is only the MSRV floor, and a
# different stable rustc emits different wasm while the determinism check
# still passes (both of its samples use the same unpinned compiler). Pin the
# compiler exactly as the CLI is pinned, and record it in the marker. CI and
# release both set up 1.96.0 (ci.yml jeliya-ui-web, release.yml embedded-ui).
pinned_rustc="1.96.0"
active_rustc="$(rustc --version | awk '{print $2}')"
if [ "$active_rustc" != "$pinned_rustc" ]; then
  echo "FAIL: rustc $active_rustc != pinned $pinned_rustc (the canonical compiler)." >&2
  echo "      rustup toolchain install $pinned_rustc" >&2
  echo "      RUSTUP_TOOLCHAIN=$pinned_rustc bash scripts/build-web.sh" >&2
  exit 1
fi

# Honor an externally-set CARGO_TARGET_DIR (the determinism check builds each
# sample in its own fresh dir); wasm-bindgen must read from the same one, or a
# caller-set target dir would silently make it consume a stale default-path
# artifact. Exporting it also pins cargo itself: a user-level
# `[build] target-dir` in $CARGO_HOME/config.toml would otherwise send the
# compile elsewhere while this script reads the default path.
target_dir="${CARGO_TARGET_DIR:-$repo/target}"
export CARGO_TARGET_DIR="$target_dir"

echo "==> cargo build --locked --release -p jeliya-ui --features web (wasm32)"
cargo build --locked --release -p jeliya-ui --features web \
  --target wasm32-unknown-unknown

echo "==> wasm-bindgen $locked_wbg (no wasm-opt)"
# $out is caller-supplied: refuse to delete a directory that is non-empty
# and not a previous artifact output (the marker is this script's own
# tombstone) — a typo'd path must not be irreversibly destroyed.
if [ -e "$out" ] && [ ! -f "$out/.dioxus-artifact" ] && [ -n "$(ls -A "$out" 2>/dev/null)" ]; then
  echo "FAIL: refusing to replace $out — it exists, is non-empty, and carries no .dioxus-artifact marker." >&2
  echo "      pass a new or empty directory, or a previous build output." >&2
  exit 1
fi
rm -rf "$out"
mkdir -p "$out"
wasm-bindgen \
  --target web \
  --no-typescript \
  --out-dir "$out" \
  "$target_dir/wasm32-unknown-unknown/release/$crate.wasm"

echo "==> assets (canonical, single-source)"
cp "$repo/crates/jeliya-ui/index.html" "$out/index.html"
# The one canonical stylesheet, consumed from its single source (§7, AC-4).
cp "$repo/ui/src/styles.css" "$out/styles.css"

# The build-time artifact marker the daemon embed guard checks for (§9). Pure
# static content so it never perturbs determinism; #183 later replaces it with
# a content-addressed sealed manifest.
cat > "$out/.dioxus-artifact" <<EOF
renderer=dioxus-web
crate=jeliya-ui
rustc=$pinned_rustc
wasm_bindgen=$locked_wbg
EOF

echo
echo "==> artifact ($out)"
# Portable size report: BSD find has no -printf, and stat's size flag differs
# between GNU (-c%s) and BSD (-f%z); `wc -c` is POSIX everywhere.
total=0
while IFS= read -r file; do
  size=$(wc -c < "$file" | tr -d ' ')
  rel=${file#"$out"/}
  printf '%10d  %s\n' "$size" "$rel"
  total=$((total + size))
done < <(LC_ALL=C find "$out" -type f | LC_ALL=C sort)
printf '%10d  TOTAL\n' "$total"
