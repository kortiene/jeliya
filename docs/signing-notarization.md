# Signing and notarization (Phase 2)

Release binaries to date are unsigned (`v0.1.0`/`v0.2.0` were
released under the project's former name Bantaba — see `docs/naming.md`).
The `curl | sh` and
Homebrew paths install cleanly because they do not set the macOS quarantine bit,
but browser downloads can still trip Gatekeeper on macOS and SmartScreen on
Windows. This document tracks the work needed to ship signed desktop binaries.

Status:

- **Implemented** — the `macos-app` job in `.github/workflows/release.yml`
  builds the `Jeliya.app` DMG and Developer-ID-signs + notarizes it
  automatically when the six secrets below exist; without them it falls back
  to an ad-hoc signature. The secrets are not set yet (Apple Developer
  enrollment is pending), so no Developer-ID-signed artifact has shipped —
  every release to date carries only the unsigned daemon archives.
- **Not implemented** — the five per-target `jeliyad` daemon archives are
  uploaded unsigned even when the secrets exist (the macOS archives are
  issue #1's remaining scope), and Windows Authenticode signing has not
  started (issue #2).
- **Out of scope: Android** — the Flutter app's Android release signing is
  separate machinery (an optional, gitignored `app/android/key.properties`
  with a debug-keystore fallback), documented in
  [`packaging/README.md`](../packaging/README.md#android-release-builds).
  The wiring exists; no production keystore does, and no Android artifact
  has ever been released. This document stays about macOS and Windows.

## Goals

- Keep the daemon local-only and reproducible while adding platform trust
  signatures to release artifacts.
- Preserve the current `v*` tag-driven release workflow.
- Never commit private signing material. All credentials live in GitHub Actions
  repository or environment secrets.

## macOS Developer ID + notarization

Required Apple assets:

- Apple Developer Program membership.
- Developer ID Application certificate exported as a password-protected `.p12`.
- Apple ID app-specific password for notarization (`xcrun notarytool` — the
  workflow uses this route, not an App Store Connect API key).
- Team ID and certificate password as GitHub secrets.

### Implemented: the `Jeliya.app` DMG (`macos-app` job)

The `macos-app` job in `.github/workflows/release.yml` plus
`scripts/package-macos.mjs` already carry the full signing + notarization
path. It activates automatically from GitHub repository secrets — no workflow
change is needed once Apple Developer enrollment completes and the secrets
are set:

| Secret | Purpose |
| --- | --- |
| `MACOS_CERT_P12` | Base64-encoded Developer ID Application `.p12`. |
| `MACOS_CERT_PASSWORD` | Password for the `.p12`. |
| `MACOS_SIGN_IDENTITY` | Full codesign identity string (`Developer ID Application: …`). |
| `NOTARY_APPLE_ID` | Apple ID that owns the app-specific password. |
| `NOTARY_TEAM_ID` | Developer Team ID. |
| `NOTARY_PASSWORD` | App-specific password for that Apple ID. |

Activation logic (see the job's `env` block):

1. Signing turns on when `MACOS_CERT_P12` and `MACOS_SIGN_IDENTITY` are both
   non-empty: the job imports the certificate (unlocked with
   `MACOS_CERT_PASSWORD`) into a throwaway keychain and passes the identity
   to `scripts/package-macos.mjs` as `JELIYA_SIGN_IDENTITY`.
2. Notarization additionally requires all three `NOTARY_*` secrets: the job
   stores them as a `notarytool` keychain profile
   (`xcrun notarytool store-credentials jeliya-notary`), and the packaging
   script submits the DMG with `xcrun notarytool submit --wait`, then staples
   the ticket.
3. When any of these are missing the job still runs and uploads a DMG signed
   with the ad-hoc identity (`-`) — same Gatekeeper caveat as the bare daemon
   archives.

The `.sha256` sidecar is generated after signing/notarization, over the final
DMG bytes.

### Not implemented: signing the bare `jeliyad` archives

The five per-target daemon archives from the `build` matrix are uploaded
unsigned even when the secrets above exist — this is the remaining scope of
issue #1. Outline:

1. Import the Developer ID certificate into a temporary keychain on the macOS
   build jobs (reuse the `macos-app` import step).
2. Build `jeliyad` with `embed-ui` as today.
3. `codesign --timestamp --options runtime --sign "$MACOS_SIGN_IDENTITY" jeliyad`.
4. Package the signed binary into the `.tar.gz` asset.
5. Submit the archive or a zipped staging bundle with `xcrun notarytool submit --wait`.
6. Keep `.sha256` sidecars over the final signed/notarized asset bytes.

Notes:

- Notarization is most valuable for browser-downloaded macOS artifacts. Homebrew
  and `curl | sh` are less affected, but signed artifacts still improve trust.
- The DMG path staples its notarization ticket to the disk image. A bare CLI
  daemon archive has no app bundle to staple; Gatekeeper checks the ticket
  online.

## Windows Authenticode signing

Not implemented — nothing in `release.yml` signs `jeliyad.exe` today, and none
of the secrets below exist. This section is the plan.

Required Windows assets:

- Code-signing certificate from a trusted CA, ideally EV if SmartScreen
  reputation is a launch concern.
- Signing key available to GitHub Actions via one of:
  - Azure Trusted Signing / cloud HSM,
  - a CA-backed remote signing service,
  - or a password-protected `.pfx` secret (least preferred operationally).

Suggested GitHub secrets for `.pfx` mode:

| Secret | Purpose |
| --- | --- |
| `WINDOWS_CERTIFICATE_PFX_BASE64` | Base64-encoded Authenticode `.pfx`. |
| `WINDOWS_CERTIFICATE_PASSWORD` | Password for the `.pfx`. |

Workflow outline:

1. Import the certificate in the Windows release job.
2. Build `jeliyad.exe` with `embed-ui` as today.
3. Sign with `signtool sign /fd SHA256 /tr <timestamp-url> /td SHA256 ... jeliyad.exe`.
4. Verify with `signtool verify /pa /v jeliyad.exe`.
5. Zip the signed executable and generate `.sha256` from final bytes.

## Acceptance checklist

- macOS `spctl --assess` / `codesign --verify --deep --strict` passes for signed artifacts.
- Windows `signtool verify /pa /v` passes for `jeliyad.exe`.
- Release artifacts remain named exactly as installers expect.
- Installer smoke tests still pass for macOS/Linux and PowerShell.
- Signing failures fail closed: no unsigned artifact is uploaded from a signing-enabled job.
