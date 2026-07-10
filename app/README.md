# Jeliya desktop (Flutter)

The native macOS shell for Jeliya: a Flutter app that spawns (or adopts) the
local `jeliyad` daemon as a supervised sidecar and talks to it over the
transport-agnostic Dart client in
[`../dart/jeliya_protocol`](../dart/jeliya_protocol). UI parity target is the
reference web client in [`../ui`](../ui) (spec: `docs/PROTOCOL.md`).

## Prerequisites

- **Rust toolchain + `cargo build`** at the repo root — the app supervises the
  `jeliyad` binary; debug runs pick up `target/debug/jeliyad` automatically.
- **Flutter** (stable channel) with macOS desktop support enabled.

## Running

```sh
cargo build                 # from the repo root: builds jeliyad
cd app
flutter run -d macos
```

### Daemon binary resolution

1. Bundled sidecar next to the app executable
   (`Contents/Resources/jeliyad`, `Contents/Helpers/jeliyad`) — the packaged
   path (Phase 5).
2. `JELIYAD_BIN=/path/to/jeliyad` environment override — the dev lever.
3. Debug builds only: the repo's `target/debug/jeliyad`.

### Data directory

- `JELIYA_DATA_DIR=/path` environment override — test automation and
  side-by-side profiles (takes precedence over both defaults below; note the
  sandboxed release app can only write inside its container and the shared
  Jeliya dir, so arbitrary override paths only work in debug builds).
- Release: `~/Library/Application Support/Jeliya` — deliberately SHARED with a
  Homebrew-installed `jeliyad` (one identity and room store per user), reached
  from inside the sandbox via the exception in `Release.entitlements`.
- Debug: `~/Library/Application Support/JeliyaAppDev` (dev runs never touch
  real user data)

The daemon's portfile (`daemon.json`), blob store, and the app's local prefs
(`app_prefs.json`: last room, per-room drafts, local peer aliases) all live
here.

### Loopback dev mode

The app currently starts the daemon with `--loopback`: single-machine
networking for development. The daemon is spawned `--supervised`, so it exits
when the app dies (stdin watch) even if graceful teardown never runs; Cmd-Q
additionally runs the graceful order `client.stop()` →
`supervisor.shutdown()`.

## Tests

```sh
cargo build                       # tests may drive the real daemon
cd app && flutter test            # widget tests use the package mock client
cd ../dart/jeliya_protocol && dart test
```

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

## Layout

- `lib/main.dart` — thin entry: theme + `SessionScope` + phase routing.
- `lib/src/theme.dart` — the design tokens (`JeliyaTokens`) ported from the
  web client.
- `lib/src/session/` — `DaemonSession` (supervisor + client + bootstrap),
  `RoomStore` (per-room state), `PrefsStore` (local prefs).
- `lib/src/screens/` — screens; `modals/` — dialogs.
- `lib/src/widgets/` — shared primitives (modal scaffold, error note, copy
  button, avatar, tree mark, progress bar, fetch control).
- `lib/src/l10n/` — `arb/` ICU catalog (`app_en.arb`, 442 keys, plus the
  full-catalog `app_fr.arb`), committed `flutter gen-l10n` output in `gen/`
  (`AppStrings`), `strings_context.dart` (the `context.strings` accessor),
  `tokens.dart` (never-translated tokens), and the `error_display.dart` /
  `wire_display.dart` display extensions over the generated catalog.

macOS notes: minimum window size is 960x620 (`MainFlutterWindow.swift`);
debug builds keep the sandbox OFF so they can spawn the repo-built daemon;
release builds are sandboxed with the co-signed bundled sidecar (see
`Release.entitlements` / `Sidecar.entitlements`).
