#!/usr/bin/env bash
# Build the real #160 Android artifacts with a pinned, explicit toolchain.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
mode="${1:-all}"

SDK="${ANDROID_SDK_ROOT:-$HOME/Library/Android/sdk}"
NDK="${ANDROID_NDK_HOME:-$SDK/ndk/27.2.12479018}"
JAVA="${JAVA_HOME:-}"
if [[ -z "$JAVA" ]] && command -v brew >/dev/null 2>&1; then
  JAVA="$(brew --prefix openjdk@17 2>/dev/null)/libexec/openjdk.jdk/Contents/Home"
fi
DX="${DIOXUS_DX:-$HOME/.local/bin/dioxus-dx}"
TARGET="armv7-linux-androideabi"
ABI="armeabi-v7a"

export JAVA_HOME="$JAVA"
export ANDROID_HOME="$SDK"
export ANDROID_SDK_ROOT="$SDK"
export ANDROID_NDK_HOME="$NDK"
export NDK_HOME="$NDK"
export PATH="$SDK/platform-tools:$JAVA/bin:$PATH"

case "$mode" in
  debug|release|all) ;;
  *) echo "usage: $0 [debug|release|all]" >&2; exit 2 ;;
esac

need() { [[ -e "$1" ]] || { echo "missing prerequisite: $1" >&2; exit 2; }; }
need "$JAVA_HOME/bin/java"
need "$DX"
need "$SDK/platform-tools/adb"
need "$SDK/build-tools/35.0.0/aapt2"
need "$SDK/cmdline-tools/latest/bin/apkanalyzer"
need "$NDK/source.properties"
need "$NDK/toolchains/llvm/prebuilt/darwin-x86_64/bin/armv7a-linux-androideabi26-clang"
rustup target list --installed | grep -qx "$TARGET" || {
  echo "missing Rust target: rustup target add $TARGET" >&2
  exit 2
}

# Development signing only: generated outside source and gitignored. Release
# APK/AAB and the explicitly signed debug AAB use this key. Gradle's debug APK
# uses the host's standard Android debug keystore. Neither signer is a Play
# upload key; signer differences are retained rather than normalized away.
keystore="$here/.android/dev-signing.jks"
signing_properties="$here/.android/signing.properties"
mkdir -p "$(dirname "$keystore")"
if [[ ! -f "$keystore" ]]; then
  # Generate unpredictable throwaway credentials; neither password is printed.
  store_password="$(python3 -c 'import secrets; print(secrets.token_urlsafe(32))')"
  signing_key_pass="$store_password"
  "$JAVA_HOME/bin/keytool" -genkeypair -noprompt \
    -keystore "$keystore" -storepass "$store_password" \
    -alias jeliya-spike-160 -keypass "$signing_key_pass" \
    -keyalg RSA -keysize 2048 -validity 365 \
    -dname "CN=Jeliya issue 160 disposable development key,O=Jeliya,C=XX"
  {
    printf 'storeFile=%s\n' "$keystore"
    printf 'storePassword=%s\n' "$store_password"
    printf 'keyAlias=jeliya-spike-160\n'
    printf 'keyPassword=%s\n' "$signing_key_pass"
  } > "$signing_properties"
  chmod 600 "$signing_properties"
elif [[ ! -f "$signing_properties" ]]; then
  echo "keystore exists but gitignored signing properties are missing: $signing_properties" >&2
  echo "remove .android/dev-signing.jks to generate a new disposable signer" >&2
  exit 2
fi
chmod 600 "$signing_properties"

signing_prop() {
  local key="$1"
  awk -F= -v key="$key" '$1 == key { sub(/^[^=]*=/, ""); print; exit }' "$signing_properties"
}

patch_gradle_signing() {
  local generated="$1"
  local gradle="$generated/app/build.gradle.kts"
  # dx 0.7.9 exposes Android signing only through plaintext Dioxus.toml.
  # Keep source credential-free: insert a generated Gradle properties reader,
  # add a release signingConfig, and bind release to it. Exact anchors fail
  # closed when dx's template changes.
  python3 - "$gradle" "$signing_properties" <<'PY'
from pathlib import Path
import sys
path=Path(sys.argv[1]); props=sys.argv[2]; text=path.read_text()
if 'jeliyaSigning' in text:
    raise SystemExit(0)
plugins='''plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}'''
if plugins not in text:
    raise SystemExit('unexpected dx Gradle plugins block')
text=text.replace(plugins, '''import java.io.FileInputStream
import java.util.Properties

''' + plugins + f'''\n\nval jeliyaSigning = Properties().apply {{\n    FileInputStream(file("{props}")).use {{ load(it) }}\n}}''', 1)
anchor='''    buildTypes {
        getByName("debug") {'''
insert='''    signingConfigs {
        create("jeliyaSpikeRelease") {
            storeFile = file(jeliyaSigning.getProperty("storeFile"))
            storePassword = jeliyaSigning.getProperty("storePassword")
            keyAlias = jeliyaSigning.getProperty("keyAlias")
            keyPassword = jeliyaSigning.getProperty("keyPassword")
        }
    }
    buildTypes {
        getByName("debug") {'''
if anchor not in text:
    raise SystemExit('unexpected dx Gradle buildTypes block')
text=text.replace(anchor, insert, 1)
release='''        getByName("release") {
            isMinifyEnabled = true'''
release_signed='''        getByName("release") {
            isMinifyEnabled = true
            signingConfig = signingConfigs.getByName("jeliyaSpikeRelease")'''
if release not in text:
    raise SystemExit('unexpected dx Gradle release block')
text=text.replace(release, release_signed, 1)
path.write_text(text)
PY
}

mkdir -p "$here/artifacts"

record_versions() {
  {
    echo "built_at_utc=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "source_sha=$(git -C "$here" rev-parse HEAD)"
    echo "source_dirty=$(git -C "$here" status --porcelain | wc -l | tr -d ' ')"
    echo "host=$(uname -a)"
    echo "macos=$(sw_vers -productVersion) ($(sw_vers -buildVersion))"
    echo "java=$("$JAVA_HOME/bin/java" -version 2>&1 | head -1)"
    echo "rustc=$(rustc --version)"
    echo "cargo=$(cargo --version)"
    echo "dioxus=$($DX --version)"
    echo "ndk=$(awk -F' = ' '/Pkg.ReleaseName/{print $2}' "$NDK/source.properties") ($(awk -F' = ' '/Pkg.Revision/{print $2; exit}' "$NDK/source.properties"))"
    echo "platform_tools=$("$SDK/platform-tools/adb" version | awk '/^Version /{print $2}')"
    echo "build_tools=$("$SDK/build-tools/35.0.0/aapt2" version 2>&1 | awk '{print $NF}')"
    echo "compile_sdk=35"
    echo "min_sdk=26"
    echo "target_sdk=35"
    echo "rust_target=$TARGET"
    echo "android_abi=$ABI"
    echo "signing=debug APK Android debug key; other artifacts disposable generated key; NOT distributable"
  } > "$here/artifacts/build-metadata.txt"
}

stage_android_resources() {
  local profile="$1"
  local generated="$here/target/dx/jeliya-spike-dioxus-android/$profile/android/app/app/src/main/res/xml"
  # dx does not remove unknown resources, but Cargo may consider build.rs fresh
  # if only target/dx was deleted. Pre-stage as well as staging from build.rs so
  # a clean or partially-cleaned tree behaves identically.
  mkdir -p "$generated"
  cp "$here/android/backup_rules.xml" "$generated/backup_rules.xml"
  cp "$here/android/data_extraction_rules.xml" "$generated/data_extraction_rules.xml"
}

run_dx() {
  local profile="$1"
  local out="$here/artifacts/$profile"
  local generated="$here/target/dx/jeliya-spike-dioxus-android/$profile/android/app"
  local bundle_task aab_source
  rm -rf "$out"
  mkdir -p "$out"
  stage_android_resources "$profile"

  # dx 0.7.9's common AAB bundler ALWAYS invokes bundleRelease, even for a
  # debug Rust build. Use dx for the profile-correct APK/native build, then run
  # the generated Gradle bundle task explicitly so "debug AAB" is not a lie.
  local args=(bundle --android --target "$TARGET" --package-types apk --out-dir "$out")
  [[ "$profile" == release ]] && args+=(--release)
  echo "==> dx ${args[*]}"
  (cd "$here" && "$DX" "${args[@]}")
  patch_gradle_signing "$generated"

  if [[ "$profile" == debug ]]; then
    bundle_task=bundleDebug
    aab_source="$generated/app/build/outputs/bundle/debug/app-debug.aab"
  else
    # Without plaintext [bundle.android], dx assembles a debug Gradle APK even
    # when the Rust profile is release. Build the credential-free generated
    # release variant explicitly and replace dx's surfaced APK.
    echo "==> gradle assembleRelease"
    (cd "$generated" && ./gradlew assembleRelease)
    release_apk="$generated/app/build/outputs/apk/release/app-release.apk"
    [[ -s "$release_apk" ]] || { echo "assembleRelease did not produce $release_apk" >&2; exit 1; }
    rm -f "$out"/*.apk
    cp "$release_apk" "$out/app-release.apk"
    bundle_task=bundleRelease
    aab_source="$generated/app/build/outputs/bundle/release/app-release.aab"
  fi
  echo "==> gradle $bundle_task"
  (cd "$generated" && ./gradlew "$bundle_task")
  [[ -s "$aab_source" ]] || { echo "$bundle_task did not produce $aab_source" >&2; exit 1; }
  cp "$aab_source" "$out/jeliya-spike-160-$profile.aab"
  if [[ "$profile" == debug ]]; then
    # AGP 8.7's bundleDebug graph logs signDebugBundle here but emits an
    # unsigned AAB (jarsigner: "jar is unsigned"). AAB signing is JAR signing;
    # apply the same disposable development key explicitly and verify below.
    "$JAVA_HOME/bin/jarsigner" \
      -keystore "$keystore" -storepass "$(signing_prop storePassword)" \
      -keypass "$(signing_prop keyPassword)" -sigalg SHA256withRSA -digestalg SHA-256 \
      "$out/jeliya-spike-160-$profile.aab" "$(signing_prop keyAlias)" >/dev/null
  fi
}

[[ "$mode" == debug || "$mode" == all ]] && run_dx debug
[[ "$mode" == release || "$mode" == all ]] && run_dx release
record_versions

# Assert the four artifacts, ABI, package identity, signatures, backup policy,
# and absence of retiring client libraries. `verify-artifacts.sh` prints each
# independent claim and is reused by the deliberate-regression matrix.
for profile in debug release; do
  [[ "$mode" == all || "$mode" == "$profile" ]] || continue
  "$here/verify-artifacts.sh" "$profile"
done

echo
echo "==> artifacts"
find "$here/artifacts" -type f -maxdepth 2 -exec sh -c 'printf "%10s  %s\n" "$(wc -c < "$1")" "${1#'"$here"'/}"' _ {} \; | sort
