# Jeliya app (Flutter)

The native shell for Jeliya on desktop and phones. On macOS it spawns (or
adopts) the local `jeliyad` daemon as a supervised sidecar; on Android it runs
the Rust engine in-process behind `FfiClient` (phones cannot spawn a sidecar
subprocess). Both transports sit behind the transport-agnostic Dart client in
[`../dart/jeliya_protocol`](../dart/jeliya_protocol). UI parity target is the
reference web client in [`../ui`](../ui) (spec: `docs/PROTOCOL.md`): the
three-pane desktop layout at or above 900dp, a bottom-tab mobile shell below.
There is no iOS platform scaffold yet — Android is the only mobile target that
runs today.

## Prerequisites

- **Rust toolchain + `cargo build`** at the repo root — the app supervises the
  `jeliyad` binary; debug runs pick up `target/debug/jeliyad` automatically.
- **Flutter** (stable channel) with macOS desktop support enabled.

Android builds additionally need (all consumed by
`../scripts/build-android-libs.mjs`):

- the three Android Rust targets — `rustup target add armv7-linux-androideabi
  aarch64-linux-android x86_64-linux-android`;
- NDK r29 at `~/Library/Android/sdk/ndk/29.0.14206865` (override with
  `ANDROID_NDK_HOME`) — the script drives its clang toolchain directly;
- a Dart SDK include dir for jeliya-ffi's `build.rs` (`dart_api_dl.c`): set
  `DART_SDK_INCLUDE` or `FLUTTER_ROOT`, or just have `flutter` on PATH (the
  script pins `FLUTTER_ROOT` from it).

## Running

### macOS

```sh
cargo build                 # from the repo root: builds jeliyad
cd app
flutter run -d macos
```

### Android

```sh
node scripts/build-android-libs.mjs   # from the repo root: libjeliya_ffi.so, all three ABIs
cd app
flutter run                           # with a device attached (or an emulator)
```

The `.so`s land in the gitignored `android/app/src/main/jniLibs/`, which
Gradle packages automatically — re-run the script after Rust-side changes.
There is no daemon process on Android: the app starts the engine in-process
and talks to it over `FfiClient`.

### Daemon binary resolution (macOS)

1. `JELIYAD_BIN=/path/to/jeliyad` environment override — the dev lever; wins
   over everything.
2. Bundled sidecar next to the app executable
   (`Contents/Resources/jeliyad`, `Contents/Helpers/jeliyad`) — the packaged
   path (Phase 5).
3. Debug builds only: the repo's `target/debug/jeliyad`.

### Data directory

- `JELIYA_DATA_DIR=/path` environment override (desktop only) — test
  automation and side-by-side profiles (takes precedence over both macOS
  defaults below; note the sandboxed release app can only write inside its
  container and the shared Jeliya dir, so arbitrary override paths only work
  in debug builds).
- macOS release: `~/Library/Application Support/Jeliya` — deliberately SHARED
  with a Homebrew-installed `jeliyad` (one identity and room store per user),
  reached from inside the sandbox via the exception in `Release.entitlements`.
- macOS debug: `~/Library/Application Support/JeliyaAppDev` (dev runs never
  touch real user data)
- Android: the platform app-support directory (via `path_provider`); the
  in-process engine owns its `engine/` subdirectory.

On desktop the daemon's portfile (`daemon.json`), blob store, and the app's
local prefs (`app_prefs.json`: last room, per-room drafts, local peer aliases)
all live here; on Android the engine keeps its stores under `engine/` and
`app_prefs.json` sits beside it.

### Loopback dev mode (macOS)

The app currently starts the daemon with `--loopback`: single-machine
networking for development. The daemon is spawned `--supervised`, so it exits
when the app dies (stdin watch) even if graceful teardown never runs; Cmd-Q
additionally runs the graceful order `client.stop()` →
`supervisor.shutdown()`.

This is a real asymmetry today: the Android build constructs its in-process
engine with `loopback: false`, so the phone is currently the surface with
real networking while the desktop sidecar stays loopback-only.

## Tests

```sh
cargo build                       # tests may drive the real daemon + FFI engine
cd app && flutter test            # widget tests inject the package mock client
cd ../dart/jeliya_protocol && dart test
```

Widget tests inject `MockClient` through the session seam (`test/helpers.dart`);
the desktop helpers tolerate the oversized test font's overflows, while the
mobile suites run on a strict 360-wide surface and assert the recorded
overflow list is empty. The package suite replays the golden conformance
corpus against the built daemon and the in-process FFI engine, and skips those
oracles cleanly when the artifacts are missing.

## Packaging (Phase 5)

```sh
node scripts/package-macos.mjs        # from the repo root
```

Builds a universal (arm64 + x86_64) `jeliyad`, a release `Jeliya.app` with the
sidecar bundled at `Contents/Helpers/jeliyad`, signs everything inner→outer
with the hardened runtime (sidecar: `Sidecar.entitlements` = sandbox +
inherit; app: `Release.entitlements` = sandbox + network + the shared-dir
exception), verifies signatures/entitlements AND the sandboxed spawn/teardown
contract at runtime, then emits `dist/Jeliya-v<version>-macos.dmg`.

Default is ad-hoc signing (runs on this machine only). With Apple Developer
enrollment, set `JELIYA_SIGN_IDENTITY="Developer ID Application: …"` and
`JELIYA_NOTARY_PROFILE=<notarytool profile>` to produce a notarized DMG — the
release workflow's `macos-app` job does the same automatically once the repo
secrets exist. Release builds are sandboxed, so `flutter run -d macos
--release` without the bundled sidecar will not find a daemon — use debug for
development and the packaging script for release builds.

Android release artifacts (the Play `.aab` and the per-ABI sideload APKs) are
documented in [`../packaging/README.md`](../packaging/README.md) under
"Android release builds".

## Layout

- `lib/main.dart` — thin entry: theme + `SessionScope` + phase routing, plus
  the per-platform session fork (desktop spawns/adopts the sidecar; Android
  builds the `FfiClient` session over the in-process engine).
- `lib/src/layout.dart` — the ONE form-factor seam: `kShellBreakpoint`
  (900dp) / `isMobileWidth`; every width fork in the app routes through it.
- `lib/src/theme.dart` — the design tokens (`JeliyaTokens`) ported from the
  web client.
- `lib/src/session/` — `DaemonSession` (supervisor + client + bootstrap),
  `RoomStore` (per-room state), `FleetStore` (agent-fleet polling),
  `PrefsStore` (local prefs).
- `lib/src/screens/` — screens; `shell.dart` owns the navigation state and
  forks at the breakpoint between the three-pane desktop layout and the
  bottom-tab mobile shell (`mobile_shell.dart`; the Rooms tab hosts a nested
  navigator: `mobile_rooms.dart` list → `mobile_room.dart` chat →
  `mobile_panel.dart` room detail). `modals/` — dialogs; join-with-ticket,
  invite, and Add Agent present full screen below the breakpoint.
- `lib/src/widgets/` — shared primitives (modal scaffold, connection banner,
  error note, copy button, buttons, avatar, sender name, template text, tree
  mark, progress bar, fetch control).
- `lib/src/l10n/` — `arb/` ICU catalog (`app_en.arb`, 444 keys, plus the
  full-catalog `app_fr.arb`), committed `flutter gen-l10n` output in `gen/`
  (`AppStrings`), `strings_context.dart` (the `context.strings` accessor),
  `tokens.dart` (never-translated tokens), and the `error_display.dart` /
  `wire_display.dart` display extensions over the generated catalog.

macOS notes: minimum window size is 960x620 (`MainFlutterWindow.swift`);
debug builds keep the sandbox OFF so they can spawn the repo-built daemon;
release builds are sandboxed with the co-signed bundled sidecar (see
`Release.entitlements` / `Sidecar.entitlements`).

Android notes: applicationId `com.incubtek.jeliya`, minSdk 26, three ABIs
(armeabi-v7a is required — real target devices run 32-bit-only Android);
predictive back is opted in (`enableOnBackInvokedCallback`) with the shell
keeping sole back authority — classic back is device-verified, the predictive
gesture itself still needs an Android 14+ pass; release signing reads the
optional gitignored `android/key.properties` and falls back to the debug
keystore (see [`../packaging/README.md`](../packaging/README.md)).
