#!/usr/bin/env bash
# Deliberately break one claim at a time on disposable copies. Every case must
# fail exactly its named assertion; assertions that cannot fail are decoration.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
JAVA="${JAVA_HOME:-}"
if [[ -z "$JAVA" ]] && command -v brew >/dev/null 2>&1; then
  JAVA="$(brew --prefix openjdk@17 2>/dev/null)/libexec/openjdk.jdk/Contents/Home"
fi
export JAVA_HOME="$JAVA"
signing_properties="$here/.android/signing.properties"
source_root="$here/artifacts"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

[[ -s "$source_root/release/app-release.apk" ]] || { echo "build first: ./build.sh all" >&2; exit 2; }
[[ -s "$signing_properties" ]] || { echo "missing gitignored signing properties: $signing_properties" >&2; exit 2; }
signing_prop() {
  local key="$1"
  awk -F= -v key="$key" '$1 == key { sub(/^[^=]*=/, ""); print; exit }' "$signing_properties"
}

run_artifact_case() { # name expected-failure mutate-command...
  local name="$1" expected="$2"; shift 2
  local root="$work/$name"
  mkdir -p "$root/release"
  cp "$source_root/release/app-release.apk" "$root/release/"
  cp "$source_root/release/jeliya-spike-160-release.aab" "$root/release/"
  "$@" "$root/release"
  set +e
  ARTIFACT_ROOT="$root" "$here/verify-artifacts.sh" release > "$root/output.txt" 2>&1
  local status=$?
  set -e
  [[ $status -ne 0 ]] || { echo "FAIL  $name: verifier stayed green"; cat "$root/output.txt"; return 1; }
  local failures
  failures=$(grep -c '^FAIL  ' "$root/output.txt" || true)
  if [[ "$failures" != 1 ]] || ! grep -Fxq "FAIL  $expected" "$root/output.txt"; then
    echo "FAIL  $name: expected only '$expected', got:"; grep '^FAIL  ' "$root/output.txt" || true
    return 1
  fi
  echo "PASS  $name -> only '$expected' failed"
}

mutate_aab_signature() {
  local dir="$1"
  local aab="$dir/jeliya-spike-160-release.aab"
  local tmp="$work/signature-edit"
  # Alter one signed payload while retaining every entry name and the original
  # META-INF signature. Appending after the ZIP directory is intentionally not
  # enough: jarsigner correctly ignores trailing bytes.
  rm -rf "$tmp"; mkdir -p "$tmp"; (cd "$tmp" && unzip -q "$aab")
  printf 'x' >> "$tmp/BundleConfig.pb"
  rm "$aab"; (cd "$tmp" && zip -qr "$aab" .)
}

mutate_remove_backup_rule() {
  local dir="$1"
  local aab="$dir/jeliya-spike-160-release.aab"
  local tmp="$work/aab-edit"
  rm -rf "$tmp"; mkdir -p "$tmp"; (cd "$tmp" && unzip -q "$aab")
  rm "$tmp/base/res/xml/backup_rules.xml"
  rm "$aab"; (cd "$tmp" && zip -qr "$aab" .)
  # Re-sign so only resource presence, not signature validity, fails.
  "$JAVA_HOME/bin/jarsigner" \
    -keystore "$(signing_prop storeFile)" -storepass "$(signing_prop storePassword)" \
    -keypass "$(signing_prop keyPassword)" -sigalg SHA256withRSA -digestalg SHA-256 \
    "$aab" "$(signing_prop keyAlias)" >/dev/null
}

mutate_add_forbidden_ffi() {
  local dir="$1"
  local aab="$dir/jeliya-spike-160-release.aab"
  local tmp="$work/ffi-edit"
  rm -rf "$tmp"; mkdir -p "$tmp"; (cd "$tmp" && unzip -q "$aab")
  cp "$tmp/base/lib/armeabi-v7a/libjeliya_spike_160.so" \
    "$tmp/base/lib/armeabi-v7a/libjeliya_ffi.so"
  rm "$aab"; (cd "$tmp" && zip -qr "$aab" .)
  "$JAVA_HOME/bin/jarsigner" \
    -keystore "$(signing_prop storeFile)" -storepass "$(signing_prop storePassword)" \
    -keypass "$(signing_prop keyPassword)" -sigalg SHA256withRSA -digestalg SHA-256 \
    "$aab" "$(signing_prop keyAlias)" >/dev/null
}

run_artifact_case bad-aab-signature \
  "AAB has a valid development signature" mutate_aab_signature
run_artifact_case missing-backup-resource \
  "AAB packages full-backup exclusions" mutate_remove_backup_rule
run_artifact_case forbidden-ffi-library \
  "AAB contains no Flutter or jeliya-ffi native library" mutate_add_forbidden_ffi

# Native evidence assertions: mutate one field at a time and require exactly one
# mismatch. This covers claims whose real producer is the physical APK.
base='{"boundary":"jeliya-core direct","dart":false,"endpoint_id_reported":true,"ffi":false,"network_mode":"real","relay_observation":"engine reported no relay URL","serialized_calls":7,"test_data":true}'
check_native() {
  local name="$1" expression="$2" expected="$3"
  local json="$work/$name.json" output="$work/$name.out"
  printf '%s\n' "$base" | jq "$expression" > "$json"
  set +e
  python3 - "$json" > "$output" 2>&1 <<'PY'
import json, sys
v=json.load(open(sys.argv[1]))
expected={"boundary":"jeliya-core direct","dart":False,"ffi":False,"network_mode":"real","endpoint_id_reported":True,"test_data":True,"serialized_calls":7}
wrong=[k for k,want in expected.items() if v.get(k) != want]
for key in wrong: print("FAIL  native " + key)
raise SystemExit(len(wrong))
PY
  local status=$?
  set -e
  [[ $status == 1 ]] || { echo "FAIL  $name: expected one mismatch, status=$status"; cat "$output"; return 1; }
  if [[ $(grep -c '^FAIL  ' "$output") != 1 ]] || \
      ! grep -Fxq "FAIL  native $expected" "$output"; then
    echo "FAIL  $name: wrong native assertion"; cat "$output"; return 1
  fi
  echo "PASS  $name -> only 'native $expected' failed"
}
check_native loopback-mode '.network_mode="loopback"' network_mode
check_native fabricated-ffi '.ffi=true' ffi
check_native missing-endpoint '.endpoint_id_reported=false' endpoint_id_reported
check_native skipped-first-run '.serialized_calls=5' serialized_calls

# Emulator gate: exercise the exact predicate against fake observations. No adb
# device or emulator is launched.
python3 - <<'PY'
def rejected(qemu, hardware, fingerprint):
    return qemu == "1" or hardware in {"ranchu", "goldfish"} or "generic" in fingerprint
cases=[("1","ranchu","generic/sdk"),("","goldfish","vendor/device"),("","mt6765","motorola/maui_retail")]
assert rejected(*cases[0]) and rejected(*cases[1]) and not rejected(*cases[2])
print("PASS  emulator gate rejects qemu/generic and accepts the measured Motorola hardware")
PY

echo "ALL DELIBERATE REGRESSIONS PASSED"
