#!/usr/bin/env bash
# AC-3 of issue #176, as an executable check:
#
#   "One canonical Dioxus artifact has deterministic assets."
#
# Builds the web artifact TWICE from a clean checkout into two directories and
# asserts a byte-identical asset tree: identical sorted file list and identical
# per-file SHA-256. A difference fails the check (§5, §11).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
cd "$repo"

a="$repo/crates/jeliya-ui/dist-a"
b="$repo/crates/jeliya-ui/dist-b"

echo "==> build 1 -> dist-a"
"$here/build-web.sh" "$a" >/dev/null
echo "==> build 2 -> dist-b"
"$here/build-web.sh" "$b" >/dev/null

# Identical sorted relative file list.
list_a="$(cd "$a" && LC_ALL=C find . -type f | LC_ALL=C sort)"
list_b="$(cd "$b" && LC_ALL=C find . -type f | LC_ALL=C sort)"
if [ "$list_a" != "$list_b" ]; then
  echo "FAIL: the two builds produced different file lists" >&2
  diff <(printf '%s\n' "$list_a") <(printf '%s\n' "$list_b") || true
  exit 1
fi

# Identical per-file bytes.
if ! diff -r "$a" "$b" >/dev/null; then
  echo "FAIL: the two builds are not byte-identical" >&2
  diff -r "$a" "$b" || true
  exit 1
fi

digest_a="$(cd "$a" && LC_ALL=C find . -type f | LC_ALL=C sort | xargs sha256sum | sha256sum | awk '{print $1}')"
digest_b="$(cd "$b" && LC_ALL=C find . -type f | LC_ALL=C sort | xargs sha256sum | sha256sum | awk '{print $1}')"
if [ "$digest_a" != "$digest_b" ]; then
  echo "FAIL: aggregate artifact digests differ: $digest_a != $digest_b" >&2
  exit 1
fi

rm -rf "$a" "$b"
echo "OK: two clean builds produced a byte-identical artifact (sha256 $digest_a)."
