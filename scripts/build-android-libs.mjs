#!/usr/bin/env node
// Build libjeliya_ffi.so for every shipped Android ABI and place them under
// app/android/app/src/main/jniLibs/<abi>/, which Gradle auto-packages into the
// APK (matching the abiFilters in app/android/app/build.gradle.kts).
//
//   node scripts/build-android-libs.mjs [--debug] [--abi armeabi-v7a,...]
//
// Drives the NDK r29 clang toolchain DIRECTLY (per-target linker + CC/AR), the
// same way the runtime-proof smoke binary was built. This deliberately does
// NOT use cargo-ndk (its 4.1.2 CLI panics against this repo's asdf-managed
// Rust) or cargokit (archived 2026-03). API level 26 == minSdk 26 is baked into
// the clang wrapper name, so the native link floor matches the Gradle minSdk.
//
// Prereqs: the three rust targets (rustup target add armv7-linux-androideabi
// aarch64-linux-android x86_64-linux-android), NDK r29 at
// ~/Library/Android/sdk/ndk/29.0.14206865 (override with ANDROID_NDK_HOME),
// and a Dart SDK include dir for jeliya-ffi's build.rs (dart_api_dl.c) — set
// DART_SDK_INCLUDE or FLUTTER_ROOT, or have `flutter` on PATH.
//
// Node 22+; no npm deps.

import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync, existsSync, realpathSync, statSync } from "node:fs";
import { homedir } from "node:os";
import { delimiter, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const args = process.argv.slice(2);
const profile = args.includes("--debug") ? "debug" : "release";
const API = "26"; // == minSdk 26

const NDK = process.env.ANDROID_NDK_HOME ||
  join(homedir(), "Library/Android/sdk/ndk/29.0.14206865");
const TB = join(NDK, "toolchains/llvm/prebuilt/darwin-x86_64/bin");

// rust target triple -> { abi dir under jniLibs, clang wrapper prefix }
const TARGETS = {
  "armv7-linux-androideabi": { abi: "armeabi-v7a", clang: `armv7a-linux-androideabi${API}-clang` },
  "aarch64-linux-android":   { abi: "arm64-v8a",   clang: `aarch64-linux-android${API}-clang` },
  "x86_64-linux-android":    { abi: "x86_64",      clang: `x86_64-linux-android${API}-clang` },
};

const abiFilter = (() => {
  const i = args.indexOf("--abi");
  if (i < 0) return null;
  return new Set(args[i + 1].split(","));
})();

const jniLibs = join(repoRoot, "app/android/app/src/main/jniLibs");
const soName = "libjeliya_ffi.so";

if (!existsSync(NDK)) {
  console.error(`build-android-libs: NDK not found at ${NDK} (set ANDROID_NDK_HOME)`);
  process.exit(1);
}

// jeliya-ffi's build.rs compiles the Dart SDK's dart_api_dl.c and resolves the
// include dir from DART_SDK_INCLUDE, then FLUTTER_ROOT (crates/jeliya-ffi/
// build.rs). Its last-ditch `dart`-on-PATH probe is machine-dependent, so when
// neither override is set, pin FLUTTER_ROOT here from the `flutter` launcher
// on PATH (symlinks resolved: the real launcher lives at <root>/bin/flutter).
const FLUTTER_ROOT = (() => {
  if (process.env.DART_SDK_INCLUDE || process.env.FLUTTER_ROOT) {
    return process.env.FLUTTER_ROOT;
  }
  const launcher = (process.env.PATH ?? "")
    .split(delimiter)
    .map((dir) => join(dir, "flutter"))
    .find((candidate) => existsSync(candidate));
  if (!launcher) {
    console.error(
      "build-android-libs: no DART_SDK_INCLUDE/FLUTTER_ROOT set and no `flutter` on PATH — " +
        "jeliya-ffi's build.rs needs one of them to locate dart_api_dl.c",
    );
    process.exit(1);
  }
  const root = dirname(dirname(realpathSync(launcher)));
  console.log(`build-android-libs: FLUTTER_ROOT=${root} (from \`flutter\` on PATH)`);
  return root;
})();

const log = (m) => console.log(`build-android-libs: ${m}`);
let built = 0;

for (const [triple, { abi, clang }] of Object.entries(TARGETS)) {
  if (abiFilter && !abiFilter.has(abi)) continue;
  const clangPath = join(TB, clang);
  if (!existsSync(clangPath)) {
    console.error(`build-android-libs: missing clang wrapper ${clangPath}`);
    process.exit(1);
  }
  // cargo's per-target linker var (TRIPLE upper-cased, '-' -> '_') + the cc
  // crate's CC/AR/RANLIB (underscore-normalized triple, shell-safe).
  const cargoVar = `CARGO_TARGET_${triple.toUpperCase().replace(/-/g, "_")}_LINKER`;
  const u = triple.replace(/-/g, "_");
  const env = {
    ...process.env,
    ...(FLUTTER_ROOT ? { FLUTTER_ROOT } : {}),
    [cargoVar]: clangPath,
    [`CC_${u}`]: clangPath,
    [`CXX_${u}`]: `${clangPath}++`,
    [`AR_${u}`]: join(TB, "llvm-ar"),
    [`RANLIB_${u}`]: join(TB, "llvm-ranlib"),
    RUSTFLAGS: `${process.env.RUSTFLAGS ?? ""} -C strip=symbols`.trim(),
  };
  log(`building ${triple} (${abi}, ${profile})`);
  const profileArgs = profile === "release" ? ["--release"] : [];
  execFileSync(
    "cargo",
    ["build", ...profileArgs, "-p", "jeliya-ffi", "--lib", "--target", triple],
    { cwd: repoRoot, env, stdio: ["ignore", "inherit", "inherit"] },
  );
  const src = join(repoRoot, "target", triple, profile, soName);
  if (!existsSync(src)) {
    console.error(`build-android-libs: expected ${src} not produced`);
    process.exit(1);
  }
  const destDir = join(jniLibs, abi);
  mkdirSync(destDir, { recursive: true });
  copyFileSync(src, join(destDir, soName));
  const mb = (statSync(src).size / 1e6).toFixed(1);
  log(`  -> jniLibs/${abi}/${soName} (${mb} MB)`);
  built++;
}

log(`done — ${built} ABI(s) built into app/android/app/src/main/jniLibs/`);
