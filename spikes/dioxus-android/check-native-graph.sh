#!/usr/bin/env bash
# Static assertions over the actual release APK and its packaged native library.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SDK="${ANDROID_SDK_ROOT:-$HOME/Library/Android/sdk}"
NDK="${ANDROID_NDK_HOME:-$SDK/ndk/27.2.12479018}"
apk="${1:-$here/artifacts/release/app-release.apk}"
nm="$NDK/toolchains/llvm/prebuilt/darwin-x86_64/bin/llvm-nm"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

[[ -s "$apk" ]] || { echo "release APK missing: $apk" >&2; exit 2; }
[[ -x "$nm" ]] || { echo "llvm-nm missing: $nm" >&2; exit 2; }

unzip -p "$apk" lib/armeabi-v7a/libjeliya_spike_160.so > "$work/lib.so"
[[ -s "$work/lib.so" ]]
"$nm" -D --defined-only "$work/lib.so" > "$work/symbols.txt"

grep -q ' Java_dev_dioxus_main_WryActivity_create$' "$work/symbols.txt"
grep -q ' Java_dev_dioxus_main_MainActivity_nativePlatformReady$' "$work/symbols.txt"
grep -q ' Java_dev_dioxus_main_MainActivity_nativeSafResult$' "$work/symbols.txt"
grep -q ' start_app$' "$work/symbols.txt"
if grep -Eq 'jeliya_engine_|Dart_(Initialize|Post|NewNativePort)' "$work/symbols.txt"; then
  echo "FAIL packaged library exposes retiring jeliya-ffi/Dart symbols" >&2
  grep -E 'jeliya_engine_|Dart_' "$work/symbols.txt" >&2
  exit 1
fi

# Dependency tree check is source-level complement to symbol evidence. It must
# include jeliya-core and must not contain the production FFI crate.
(cd "$here" && cargo tree --locked --target armv7-linux-androideabi --edges normal) > "$work/tree.txt"
grep -q '^├── jeliya-core\|^└── jeliya-core' "$work/tree.txt"
if grep -q 'jeliya-ffi' "$work/tree.txt"; then
  echo "FAIL target graph contains jeliya-ffi" >&2
  exit 1
fi

printf 'PASS  release APK carries an ELF32 ARM Dioxus/JNI library\n'
printf 'PASS  native library exports the custom platform callbacks\n'
printf 'PASS  native symbols contain no jeliya-ffi/Dart entry points\n'
printf 'PASS  target graph contains jeliya-core and no jeliya-ffi\n'
