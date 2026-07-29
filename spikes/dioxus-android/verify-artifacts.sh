#!/usr/bin/env bash
# Inspect one real dx/Gradle artifact pair and report every independent claim.
# Usage: ./verify-artifacts.sh debug|release
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
profile="${1:-release}"
SDK="${ANDROID_SDK_ROOT:-$HOME/Library/Android/sdk}"
JAVA="${JAVA_HOME:-}"
if [[ -z "$JAVA" ]] && command -v brew >/dev/null 2>&1; then
  JAVA="$(brew --prefix openjdk@17 2>/dev/null)/libexec/openjdk.jdk/Contents/Home"
fi
export JAVA_HOME="$JAVA"

case "$profile" in
  debug|release) ;;
  *) echo "usage: $0 [debug|release]" >&2; exit 2 ;;
esac

artifact_root="${ARTIFACT_ROOT:-$here/artifacts}"
out="$artifact_root/$profile"
apk="$(find "$out" -maxdepth 1 -name '*.apk' -type f | head -1)"
aab="$(find "$out" -maxdepth 1 -name '*.aab' -type f | head -1)"
[[ -s "$apk" && -s "$aab" ]] || { echo "missing $profile APK/AAB" >&2; exit 2; }

fail=0
check() {
  local name="$1"; shift
  if "$@"; then
    printf 'PASS  %s\n' "$name"
  else
    printf 'FAIL  %s\n' "$name"
    fail=$((fail + 1))
  fi
}
# Invoked indirectly by check(), so shellcheck cannot see the call sites.
# shellcheck disable=SC2329
contains() { grep -q -- "$1" "$2"; }
# shellcheck disable=SC2329
absent() { ! grep -Eq -- "$1" "$2"; }

# Generate the reports first. A tool failure is itself a named verification
# failure rather than an errexit with no clue which claim was unmeasured.
if "$SDK/build-tools/35.0.0/aapt2" dump badging "$apk" > "$out/apk-badging.txt" 2>&1; then
  printf 'PASS  aapt2 reads APK badging\n'
else
  printf 'FAIL  aapt2 reads APK badging\n'
  fail=$((fail + 1))
fi
check "APK package identity is isolated spike namespace" \
  contains "package: name='dev.jeliya.spike160'" "$out/apk-badging.txt"
check "APK carries selected armeabi-v7a ABI" \
  contains "native-code: 'armeabi-v7a'" "$out/apk-badging.txt"

"$SDK/build-tools/35.0.0/apksigner" verify --verbose --print-certs "$apk" \
  > "$out/apk-signature.txt" 2>&1
check "APK has a valid v2 development signature" \
  contains 'Verified using v2 scheme (APK Signature Scheme v2): true' "$out/apk-signature.txt"

"$JAVA_HOME/bin/jarsigner" -verify -certs "$aab" > "$out/aab-signature.txt" 2>&1
check "AAB has a valid development signature" \
  contains 'jar verified.' "$out/aab-signature.txt"

"$SDK/cmdline-tools/latest/bin/apkanalyzer" dex packages --defined-only "$apk" \
  > "$out/dex-packages.txt" 2>&1
check "R8 retained JNI-reflected SAF method" \
  contains 'MainActivity void launchSafPicker()' "$out/dex-packages.txt"
check "R8 retained JNI-reflected protected-state method" \
  contains 'MainActivity java.lang.String prepareProtectedState()' "$out/dex-packages.txt"

unzip -l "$apk" > "$out/apk-entries.txt" 2>&1
check "APK carries Dioxus native library for selected ABI" \
  contains 'lib/armeabi-v7a/libjeliya_spike_160.so' "$out/apk-entries.txt"
check "APK contains no Flutter or jeliya-ffi native library" \
  absent 'libjeliya_ffi|flutter|libapp\.so' "$out/apk-entries.txt"

"$SDK/build-tools/35.0.0/aapt2" dump xmltree "$apk" --file AndroidManifest.xml \
  > "$out/manifest.txt" 2>&1
check "manifest disables Android backup" \
  contains 'android:allowBackup(0x01010280)=false' "$out/manifest.txt"
check "manifest references legacy full-backup exclusions" \
  contains 'android:fullBackupContent' "$out/manifest.txt"
check "manifest references API31 data-extraction exclusions" \
  contains 'android:dataExtractionRules' "$out/manifest.txt"
check "manifest opts application into predictive Back" \
  contains 'android:enableOnBackInvokedCallback' "$out/manifest.txt"

"$SDK/build-tools/35.0.0/aapt2" dump resources "$apk" > "$out/resources.txt" 2>&1
check "APK resolves full-backup rules resource" \
  contains 'xml/backup_rules' "$out/resources.txt"
check "APK resolves data-extraction rules resource" \
  contains 'xml/data_extraction_rules' "$out/resources.txt"

unzip -l "$aab" > "$out/aab-entries.txt" 2>&1
check "AAB packages full-backup exclusions" \
  contains 'base/res/xml/backup_rules.xml' "$out/aab-entries.txt"
check "AAB packages data-extraction exclusions" \
  contains 'base/res/xml/data_extraction_rules.xml' "$out/aab-entries.txt"
check "AAB carries Dioxus native library for selected ABI" \
  contains 'base/lib/armeabi-v7a/libjeliya_spike_160.so' "$out/aab-entries.txt"
check "AAB contains no Flutter or jeliya-ffi native library" \
  absent 'libjeliya_ffi|flutter|libapp\.so' "$out/aab-entries.txt"

printf '\n%s: %d failed assertion(s)\n' "$profile" "$fail"
exit "$fail"
