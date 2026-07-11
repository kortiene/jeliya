# Jeliya packaging & distribution

These files distribute the `jeliyad` daemon as prebuilt, per-platform binaries.
The installer scripts and archive URLs are wired to `kortiene/jeliya` and have
been live since `v0.3.0` (see the release-status section below). `jeliya.rb`
is a per-release Homebrew formula: its `version` and sha256 values are
refreshed from each release's sidecars. The last section covers the Android
app's release builds.

## Files

| File | What it is |
| --- | --- |
| `../.github/workflows/release.yml` | GitHub Actions release build. Triggers on a `v*` tag push; re-runs the ci.yml gates, builds `jeliyad` for five targets and attaches the archives (+ `.sha256` sidecars) to the GitHub Release; a separate `macos-app` job builds and uploads the `Jeliya.app` DMG. |
| `install.sh` | POSIX-sh one-liner installer for macOS + Linux (`curl \| sh`). Detects OS/arch, downloads the matching `.tar.gz`, installs `jeliyad` to `/usr/local/bin` (or `~/.local/bin`). |
| `install.ps1` | Windows PowerShell equivalent. Downloads the `.zip`, expands to `%LOCALAPPDATA%\Programs\Jeliya`, adds it to the user PATH. |
| `jeliya.rb` | Homebrew formula template. Belongs in a tap (`kortiene/homebrew-jeliya`), not homebrew-core. |
| `jeliya-app.rb` | Homebrew CASK template for the desktop app (`Jeliya.app` DMG from the `macos-app` release job; built by `../scripts/package-macos.mjs`). Belongs in the same tap, as `Casks/jeliya.rb`. |

## How they fit together

1. You push a tag like `v0.1.0`.
2. `release.yml` builds one archive per target and uploads them to the Release:
   - `jeliyad-v0.1.0-aarch64-apple-darwin.tar.gz`
   - `jeliyad-v0.1.0-x86_64-apple-darwin.tar.gz`
   - `jeliyad-v0.1.0-x86_64-unknown-linux-musl.tar.gz`
   - `jeliyad-v0.1.0-aarch64-unknown-linux-musl.tar.gz`
   - `jeliyad-v0.1.0-x86_64-pc-windows-msvc.zip`
   - plus a `<asset>.sha256` next to each one.
3. End users install with `install.sh` / `install.ps1` (which resolve the
   latest tag, or a pinned `JELIYA_VERSION`), or with `brew install` once
   `jeliya.rb` is published in a tap with the real URLs + sha256s filled in.

## Mandatory build ordering (UI before cargo)

The release binary is built with the cargo feature `embed-ui`, which embeds
`ui/dist` into the binary via `rust-embed`. **`ui/dist` must exist before the
cargo build**, so every build path does, in order:

```sh
cd ui && npm ci && npm run build      # produces ui/dist  (do this FIRST)
cargo build --release -p jeliyad --features embed-ui   # (or `cargo zigbuild` for musl)
```

`release.yml` already enforces this ordering (the "Build UI" step runs before
the cargo build in every job). If you build a release binary by hand, do the
same or the UI will be missing/stale.

Linux targets use `cargo zigbuild` against `*-unknown-linux-musl` to produce
static binaries and dodge glibc-version breakage (the tree has C deps — `ring`,
`libsqlite3-sys` — and a QUIC/UDP stack via `iroh`, so a C toolchain is
required; zig supplies it).

## Release status (and the 2026-07-05 rename)

The project was renamed **Bantaba → Jeliya** on 2026-07-05 (`docs/naming.md`).
The rename is complete and the bridging release it required has already
shipped:

1. **GitHub repo:** renamed — `git remote -v` resolves to
   `git@github.com:kortiene/jeliya.git`.
2. **Old releases:** `v0.1.0` and `v0.2.0` were published under the old name
   with `bantabad-<tag>-<target>` archives containing a `bantabad` binary.
   The formula and install scripts look for `jeliyad-<tag>-<target>` — they
   could not install those old releases.
3. **Bridging release:** `v0.3.0` ("first installable Jeliya release") and
   `v0.3.1` were cut after the repo rename, built by `release.yml` from
   `-p jeliyad`, and packaged as `jeliyad-<tag>-<target>` archives. `jeliya.rb`
   was filled in with the matching version and sha256 values for both tags.
4. **Homebrew tap:** the top-level README's install command
   (`brew install kortiene/jeliya/jeliya`) resolves against a
   `kortiene/homebrew-jeliya` tap carrying the `jeliya.rb` formula, so the tap
   is reachable under the new name. This directory doesn't track that tap
   repo's own history, so we can confirm the naming is correct but not
   whether the tap repo was renamed in place or created fresh.
5. **Redistribution rights:** confirmed for publishing built binaries that
   include the pinned `iroh-rooms` git dependency (unchanged by the rename).

## Per-release follow-up

For the next release, update `version` in `jeliya.rb`, push the new `v*` tag,
then replace the formula sha256 values from the new release sidecars.

`release.yml` needs no slug edit — it always builds the repo it runs in.

## Signing / notarization = Phase 2 (in progress)

The daemon archives are **unsigned**. A *browser* download of an unsigned
binary trips Gatekeeper (macOS) and SmartScreen (Windows). The `curl | sh` and
Homebrew install paths do **not** set the quarantine bit, so they install
cleanly. The `macos-app` DMG job already carries the full Developer ID +
notarization path and activates automatically once the six
`MACOS_*`/`NOTARY_*` repo secrets exist — Apple Developer enrollment is
pending, no secrets are set, and no DMG has shipped in a release yet (the job
landed after `v0.4.3`; every release to date carries only the unsigned daemon
archives). Signing the bare daemon archives (issue #1) and Windows
Authenticode (issue #2) are tracked in
[`../docs/signing-notarization.md`](../docs/signing-notarization.md).

## Android release builds

The Android app (`app/android`) release-signs from an **optional, gitignored**
`app/android/key.properties`. Without it, release builds fall back to the
debug keystore so `flutter run --release` works on-device — those artifacts
are **not distributable** (Play rejects debug signatures, and sideloaded
upgrades break when the signature later changes). No keystore is checked in
anywhere, and none should ever be.

### One-time keystore setup

Create an upload keystore **outside the repo** (never commit it):

```sh
keytool -genkeypair -v \
  -keystore "$HOME/keystores/jeliya-upload.jks" \
  -keyalg RSA -keysize 2048 -validity 10000 \
  -alias upload
```

Then create `app/android/key.properties` (gitignored via
`app/android/.gitignore`, alongside `*.jks`/`*.keystore`):

```properties
storeFile=/Users/you/keystores/jeliya-upload.jks
storePassword=...
keyAlias=upload
keyPassword=...
```

Relative `storeFile` paths resolve against `app/android/app`; an absolute
path is simplest.

### The two build commands

```sh
cd app
flutter build appbundle                    # release path: .aab for Play
flutter build apk --split-per-abi --release  # sideload path: one APK per ABI
```

Both artifacts package `libjeliya_ffi.so` per ABI from the gitignored
`app/android/app/src/main/jniLibs/` — build those first with
`node scripts/build-android-libs.mjs` (prerequisites in `../app/README.md`);
this is the Android analogue of the UI-before-cargo ordering above.

- **Play (release path):** Play requires app bundles and enrolls the app in
  Play App Signing — Google holds the distribution key and our keystore
  becomes the *upload* key. The bundle builds with no extra Gradle flags.
- **Sideload:** `--split-per-abi` produces one APK per supported ABI
  (armeabi-v7a, arm64-v8a, x86_64). The Gradle config skips its
  `ndk.abiFilters` for this build only (AGP forbids `abiFilters` combined
  with ABI splits); the default debug fat APK is unaffected.

### Sizes (measured 2026-07-10, release, tree-shaken icons)

| Artifact | Size |
| --- | --- |
| `app-armeabi-v7a-release.apk` | 30.4 MB |
| `app-arm64-v8a-release.apk` | 39.7 MB |
| `app-x86_64-release.apk` | 43.1 MB |
| `app-release.aab` (all ABIs; Play serves per-device splits) | 77.2 MB |
| fat `app-release.apk` (all ABIs, for reference) | 112.1 MB |
| fat `app-debug.apk` (the on-device dev build) | 222 MB |

Per-ABI APKs run larger than a stock Flutter app (~15–25 MB) because each
carries the bundled Rust core (`libjeliya_ffi.so`: 14–23 MB per ABI).
