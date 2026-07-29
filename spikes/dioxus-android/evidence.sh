#!/usr/bin/env bash
# Drive the REAL release APK on a REAL physical Android device for issue #160.
# No emulator fallback exists. This script captures machine-verifiable evidence;
# manual TalkBack, predictive-gesture, and picker observations remain explicit.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SDK="${ANDROID_SDK_ROOT:-$HOME/Library/Android/sdk}"
ADB="${ADB:-$SDK/platform-tools/adb}"
PKG=dev.jeliya.spike160
ACTIVITY="$PKG/dev.dioxus.main.MainActivity"
APK="${APK:-$here/artifacts/release/app-release.apk}"
SERIAL="${ANDROID_SERIAL:-}"
CLEAN_INSTALL="${CLEAN_INSTALL:-0}"

[[ -x "$ADB" ]] || { echo "adb missing: $ADB" >&2; exit 2; }
[[ -s "$APK" ]] || { echo "build release first: ./build.sh release" >&2; exit 2; }

if [[ -z "$SERIAL" ]]; then
  devices=()
  while IFS= read -r device; do
    [[ -n "$device" ]] && devices+=("$device")
  done < <("$ADB" devices | awk 'NR>1 && $2=="device" {print $1}')
  [[ ${#devices[@]} == 1 ]] || {
    echo "expected exactly one authorized physical device; set ANDROID_SERIAL if needed" >&2
    "$ADB" devices -l >&2
    exit 2
  }
  SERIAL="${devices[0]}"
fi
adb=("$ADB" -s "$SERIAL")

qemu="$("${adb[@]}" shell getprop ro.kernel.qemu | tr -d '\r')"
hardware="$("${adb[@]}" shell getprop ro.hardware | tr -d '\r')"
fingerprint="$("${adb[@]}" shell getprop ro.build.fingerprint | tr -d '\r')"
if [[ "$qemu" == 1 || "$hardware" == ranchu || "$hardware" == goldfish || "$fingerprint" == *generic* ]]; then
  echo "refusing emulator: #160 AC4/AC5 require physical hardware" >&2
  exit 2
fi

stamp="$(date -u +%Y%m%dT%H%M%SZ)"
out="$here/evidence/run-$stamp"
mkdir -p "$out"

# Preserve the user's power settings. Physical evidence takes longer than this
# device's short lock timeout; keep an already-unlocked, USB-powered screen
# awake for the run, then restore exactly (including an absent setting).
original_stay_on="$("${adb[@]}" shell settings get global stay_on_while_plugged_in | tr -d '\r')"
original_timeout="$("${adb[@]}" shell settings get system screen_off_timeout | tr -d '\r')"
restore_setting() { # namespace key original
  if [[ "$3" == null || -z "$3" ]]; then
    "${adb[@]}" shell settings delete "$1" "$2" >/dev/null 2>&1 || true
  else
    "${adb[@]}" shell settings put "$1" "$2" "$3" >/dev/null 2>&1 || true
  fi
}
cleanup() {
  restore_setting global stay_on_while_plugged_in "$original_stay_on"
  restore_setting system screen_off_timeout "$original_timeout"
  if [[ -n "${original_accel:-}" ]]; then
    restore_setting system accelerometer_rotation "$original_accel"
    restore_setting system user_rotation "$original_rotation"
  fi
}
trap cleanup EXIT INT TERM
"${adb[@]}" shell settings put global stay_on_while_plugged_in 3
"${adb[@]}" shell settings put system screen_off_timeout 1800000
cp "$here/artifacts/build-metadata.txt" "$out/build-metadata.txt"
shasum -a 256 "$APK" > "$out/apk.sha256"

prop() { "${adb[@]}" shell getprop "$1" | tr -d '\r'; }
webview="$("${adb[@]}" shell dumpsys webviewupdate | sed -n 's/.*Current WebView package (name, version): (\([^,]*\), \([^)]*\)).*/\1 \2/p' | head -1 | tr -d '\r')"
{
  # Retain enough to correlate a rerun without publishing the hardware serial.
  echo "serial_suffix=${SERIAL: -4}"
  echo "serial_sha256=$(printf '%s' "$SERIAL" | shasum -a 256 | awk '{print $1}')"
  echo "manufacturer=$(prop ro.product.manufacturer)"
  echo "model=$(prop ro.product.model)"
  echo "device=$(prop ro.product.device)"
  echo "sku=$(prop ro.boot.hardware.sku)"
  echo "android=$(prop ro.build.version.release)"
  echo "api=$(prop ro.build.version.sdk)"
  echo "security_patch=$(prop ro.build.version.security_patch)"
  echo "abi_list=$(prop ro.product.cpu.abilist)"
  echo "fingerprint=$fingerprint"
  echo "hardware=$hardware"
  echo "qemu=${qemu:-0}"
  echo "webview=$webview"
  echo "navigation_mode=$("${adb[@]}" shell settings get secure navigation_mode | tr -d '\r')"
  echo "talkback_package=$("${adb[@]}" shell pm list packages com.google.android.marvin.talkback | tr -d '\r')"
} > "$out/device.txt"

# A clean run removes this disposable package and proves the final artifact's
# seven-call first-run path. Motorola hung once during package clear/uninstall,
# so bound the adb operation and fail rather than silently falling back to a
# replace install. Ordinary runs remain signature-compatible replace installs.
if [[ "$CLEAN_INSTALL" == 1 ]]; then
  "${adb[@]}" shell am force-stop "$PKG" >/dev/null 2>&1 || true
  "${adb[@]}" uninstall "$PKG" > "$out/uninstall.txt" 2>&1 &
  uninstall_pid=$!
  for _ in $(seq 1 120); do
    kill -0 "$uninstall_pid" 2>/dev/null || break
    sleep 1
  done
  if kill -0 "$uninstall_pid" 2>/dev/null; then
    kill "$uninstall_pid" 2>/dev/null || true
    wait "$uninstall_pid" 2>/dev/null || true
    echo "clean uninstall timed out after 120s" >&2
    exit 1
  fi
  wait "$uninstall_pid"
  grep -q '^Success$' "$out/uninstall.txt"
  if "${adb[@]}" shell pm path "$PKG" | grep -q '^package:'; then
    echo "package still installed after clean uninstall" >&2
    exit 1
  fi
fi
"${adb[@]}" logcat -c
"${adb[@]}" install -r -t "$APK" | tee "$out/install.txt"
"${adb[@]}" shell am force-stop "$PKG"
"${adb[@]}" shell am start -W -n "$ACTIVITY" | tee "$out/launch.txt"

for _ in $(seq 1 90); do
  if "${adb[@]}" logcat -d -v threadtime | grep -q 'SPIKE160_NATIVE'; then break; fi
  sleep 1
done
"${adb[@]}" logcat -d -v threadtime > "$out/logcat.txt"
grep 'SPIKE160_NATIVE' "$out/logcat.txt" | tail -1 > "$out/native-bootstrap.txt"
[[ -s "$out/native-bootstrap.txt" ]] || { echo "native bootstrap evidence absent" >&2; exit 1; }
sed 's/^.*SPIKE160_NATIVE //' "$out/native-bootstrap.txt" > "$out/native-bootstrap.json"
python3 - "$out/native-bootstrap.json" <<'PY'
import json, sys
value=json.load(open(sys.argv[1]))
expected={
  "boundary":"jeliya-core direct",
  "dart":False,
  "ffi":False,
  "network_mode":"real",
  "endpoint_id_reported":True,
  "test_data":True,
}
wrong={k:(value.get(k),v) for k,v in expected.items() if value.get(k) != v}
if wrong:
    raise SystemExit(f"native bootstrap claim mismatch: {wrong}")
if not isinstance(value.get("serialized_calls"), int) or value["serialized_calls"] < 5:
    raise SystemExit("serialized call evidence missing")
PY
if [[ "$CLEAN_INSTALL" == 1 ]]; then
  python3 - "$out/native-bootstrap.json" <<'PY'
import json, sys
value=json.load(open(sys.argv[1]))
if value.get("serialized_calls") != 7:
    raise SystemExit(f"clean install did not take seven-call first-run path: {value.get('serialized_calls')}")
PY
fi
if grep -Eq 'FATAL EXCEPTION|Fatal signal|ANR in dev\.jeliya\.spike160' "$out/logcat.txt"; then
  echo "release app crashed or ANRed" >&2
  exit 1
fi
pid="$("${adb[@]}" shell pidof "$PKG" | tr -d '\r')"
[[ -n "$pid" ]] || { echo "release process is not alive" >&2; exit 1; }

# Fail closed if the device is locked: lock-screen pixels and hierarchy are not
# app evidence (this exact false-positive was caught during the spike).
keyguard="$("${adb[@]}" shell dumpsys window policy | sed -n 's/^[[:space:]]*showing=\([^ ]*\).*/\1/p' | head -1 | tr -d '\r')"
if [[ "$keyguard" == true ]]; then
  echo "device is locked; unlock it and rerun evidence.sh" >&2
  exit 2
fi

# Native bootstrap may finish before the first Dioxus/WebView render on a cold
# install. Wait for the actual rendered status rather than racing a one-shot
# UIAutomator dump and misclassifying a healthy startup tree as failure.
rendered=0
for attempt in $(seq 1 30); do
  "${adb[@]}" shell uiautomator dump /sdcard/jeliya-spike160.xml >/dev/null
  "${adb[@]}" pull /sdcard/jeliya-spike160.xml "$out/hierarchy.xml" >/dev/null
  if grep -q 'In-process bootstrap completed' "$out/hierarchy.xml"; then
    rendered=1
    break
  fi
  sleep 1
done
"${adb[@]}" shell rm /sdcard/jeliya-spike160.xml
[[ "$rendered" == 1 ]] || { echo "rendered bootstrap did not appear within 30s" >&2; exit 1; }
"${adb[@]}" exec-out screencap -p > "$out/bootstrap.png"

grep -q 'In-process bootstrap completed' "$out/hierarchy.xml"
grep -q 'Message field' "$out/hierarchy.xml"
grep -q 'class="android.widget.EditText"' "$out/hierarchy.xml"
grep -q 'Choose a test file' "$out/hierarchy.xml"
grep -q '/no_backup/dioxus-m0-spike-v1' "$out/hierarchy.xml"

# IME: the field begins below the fold, and UIAutomator reports a zero-height
# node until it is visible. Scroll the real WebView until the EditText has
# tappable bounds; never tap guessed coordinates.
coords=""
for attempt in $(seq 1 8); do
  "${adb[@]}" shell uiautomator dump /sdcard/jeliya-spike160-ime.xml >/dev/null
  "${adb[@]}" pull /sdcard/jeliya-spike160-ime.xml "$out/ime-hierarchy-$attempt.xml" >/dev/null
  coords="$(python3 - "$out/ime-hierarchy-$attempt.xml" <<'PY' || true
import re, sys, xml.etree.ElementTree as ET
root=ET.parse(sys.argv[1]).getroot()
for node in root.iter('node'):
    if node.attrib.get('class') == 'android.widget.EditText':
        x1,y1,x2,y2=map(int,re.findall(r'\d+',node.attrib['bounds']))
        if x2 > x1 and y2-y1 >= 24 and y1 < 1500:
            print((x1+x2)//2,(y1+y2)//2)
            break
PY
)"
  [[ -n "$coords" ]] && break
  "${adb[@]}" shell input swipe 360 1320 360 520 450
  sleep 1
 done
[[ -n "$coords" ]] || { echo "could not scroll the real EditText into view" >&2; exit 1; }
read -r x y <<< "$coords"
"${adb[@]}" shell input tap "$x" "$y"
sleep 2
"${adb[@]}" shell input text 'hardware_test'
"${adb[@]}" shell dumpsys input_method > "$out/ime.txt"
grep -q 'mInputShown=true\|mIsInputViewShown=true' "$out/ime.txt"
"${adb[@]}" exec-out screencap -p > "$out/ime.png"
"${adb[@]}" shell input keyevent KEYCODE_BACK

# Resume: HOME then reopen. Native code performs room.list, explicitly not a
# fabricated reconnect. Capture the updated hierarchy after it settles.
"${adb[@]}" shell input keyevent KEYCODE_HOME
sleep 2
"${adb[@]}" shell am start -n "$ACTIVITY" >/dev/null
sleep 3
"${adb[@]}" shell uiautomator dump /sdcard/jeliya-spike160-resume.xml >/dev/null
"${adb[@]}" pull /sdcard/jeliya-spike160-resume.xml "$out/resume-hierarchy.xml" >/dev/null
grep -q 'authoritative room.list after resume' "$out/resume-hierarchy.xml"

# Rotation: ask the physical display to rotate and prove rendered viewport text
# changes. The global cleanup trap restores user rotation before leaving.
original_accel="$("${adb[@]}" shell settings get system accelerometer_rotation | tr -d '\r')"
original_rotation="$("${adb[@]}" shell settings get system user_rotation | tr -d '\r')"
"${adb[@]}" shell settings put system accelerometer_rotation 0
"${adb[@]}" shell settings put system user_rotation 1
landscape_rendered=0
for attempt in $(seq 1 30); do
  "${adb[@]}" shell uiautomator dump /sdcard/jeliya-spike160-landscape.xml >/dev/null
  "${adb[@]}" pull /sdcard/jeliya-spike160-landscape.xml "$out/landscape-hierarchy.xml" >/dev/null
  if grep -q 'landscape' "$out/landscape-hierarchy.xml"; then
    landscape_rendered=1
    break
  fi
  sleep 1
done
[[ "$landscape_rendered" == 1 ]] || { echo "rendered landscape state did not appear within 30s" >&2; exit 1; }
"${adb[@]}" exec-out screencap -p > "$out/landscape.png"
python3 - "$out/bootstrap.png" "$out/landscape.png" "$out/landscape-hierarchy.xml" <<'PY'
import struct, sys, xml.etree.ElementTree as ET

def png_size(path):
    raw=open(path,'rb').read(24)
    if raw[:8] != b'\x89PNG\r\n\x1a\n': raise SystemExit(f'not PNG: {path}')
    return struct.unpack('>II',raw[16:24])
portrait=png_size(sys.argv[1]); landscape=png_size(sys.argv[2])
if not (portrait[1] > portrait[0] and landscape[0] > landscape[1]):
    raise SystemExit(f'rotation dimensions did not change: {portrait} -> {landscape}')
texts=[n.attrib.get('text','') for n in ET.parse(sys.argv[3]).getroot().iter('node')]
if not any('landscape' in text for text in texts):
    raise SystemExit('rendered orientation did not report landscape')
PY
restore_setting system accelerometer_rotation "$original_accel"
restore_setting system user_rotation "$original_rotation"
original_accel=""

# Backup policy and protected placement are asserted without root. `dumpsys
# package` proves allowBackup=false; logcat's canonical native evidence plus UI
# path proves Android returned noBackupFilesDir. Release is intentionally not
# debuggable, so run-as cannot inspect its private files.
"${adb[@]}" shell dumpsys package "$PKG" > "$out/package.txt"
grep -q 'ALLOW_BACKUP' "$out/package.txt" && { echo "allowBackup unexpectedly enabled" >&2; exit 1; } || true
"${adb[@]}" shell bmgr backupnow "$PKG" > "$out/backup-attempt.txt" 2>&1
grep -q 'Backup is not allowed' "$out/backup-attempt.txt"

# Accessibility service output and UI tree are retained even before manual
# TalkBack testing. Never silently enable TalkBack: that changes user settings.
"${adb[@]}" shell dumpsys accessibility > "$out/accessibility.txt"
"${adb[@]}" shell settings get secure enabled_accessibility_services > "$out/enabled-accessibility-services.txt"

cat <<EOF
PASS  physical non-emulator device: $(prop ro.product.model), API $(prop ro.build.version.sdk)
PASS  release process alive and emitted in-process native bootstrap evidence
PASS  rendered hierarchy exposes bootstrap, IME field, and SAF action
PASS  IME shown and accepted text
PASS  resume triggered authoritative room.list (not reconnect)
PASS  rotation evidence captured
PASS  accessibility hierarchy/service output captured
EVIDENCE_DIR=$out

MANUAL REQUIRED (record honestly in README):
  1. Tap 'Choose a test file'; select a harmless file and verify selected name + content:// URI.
  2. Enable TalkBack and traverse heading/status/input/button; record spoken output.
  3. Switch to gesture navigation if available, perform predictive Back, and record animation/result.
EOF
