---
type: "Runbook"
title: "Signing and notarization (Phase 2)"
description: "Release-security plan and procedure for signing and notarizing Jeliya artifacts on macOS and Windows."
tags: ["macos", "release", "security", "signing", "windows"]
timestamp: "2026-07-11T21:27:07Z"
status: "canonical"
implementation_status: "partial"
verification_status: "partial"
release_status: "unreleased"
audience: ["contributors", "maintainers", "release-engineers"]
---

# Signing and notarization (Phase 2)

Release binaries to date are unsigned (`v0.1.0`/`v0.2.0` were
released under the project's former name Bantaba — see `docs/naming.md`).
The `curl | sh` and
Homebrew paths install cleanly because they do not set the macOS quarantine bit,
but browser downloads can still trip Gatekeeper on macOS and SmartScreen on
Windows. This document tracks the work needed to ship signed desktop binaries.

Current status:

- The `v0.5.0` workflow publishes only five unsigned `jeliyad` archives with
  their checksum sidecars. It contains no `macos-app` job, DMG upload,
  Developer ID signing, notarization, or Authenticode step.
- Source-level macOS packaging scripts and unsigned development builds exist,
  but they are not public release artifacts and do not satisfy a platform gate.
- Android signing is separate future distribution work. `v0.5.0` publishes no
  APK or AAB, and a debug-keystore fallback must never be treated as a
  distributable build. See
  [`packaging/README.md`](../packaging/README.md#android-release-builds).

## Goals

- Keep the daemon local-only and reproducible while adding platform trust
  signatures to release artifacts.
- Preserve the manual, exact-version promotion workflow and its private
  artifact staging, two clean CI runs, and sole write-enabled final job.
- Never commit private signing material. All credentials live in GitHub Actions
  repository or environment secrets.

## macOS Developer ID + notarization

Required Apple assets:

- Apple Developer Program membership.
- Developer ID Application certificate exported as a password-protected `.p12`.
- Apple ID app-specific password for notarization (`xcrun notarytool` — the
  workflow uses this route, not an App Store Connect API key).
- Team ID and certificate password as GitHub secrets.

### Planned: a signed and notarized `Jeliya.app` DMG

`scripts/package-macos.mjs` is development packaging machinery, not a current
release job. A future reviewed workflow may use the following credentials:

| Secret | Purpose |
| --- | --- |
| `MACOS_CERT_P12` | Base64-encoded Developer ID Application `.p12`. |
| `MACOS_CERT_PASSWORD` | Password for the `.p12`. |
| `MACOS_SIGN_IDENTITY` | Full codesign identity string (`Developer ID Application: …`). |
| `NOTARY_APPLE_ID` | Apple ID that owns the app-specific password. |
| `NOTARY_TEAM_ID` | Developer Team ID. |
| `NOTARY_PASSWORD` | App-specific password for that Apple ID. |

Required future controls:

1. Import the certificate into a throwaway keychain with logs that never expose
   credential values.
2. Sign the app and embedded daemon with hardened runtime enabled.
3. Verify the signature locally before notarization.
4. Submit with `notarytool`, wait for success, staple the ticket, and re-verify.
5. Generate the checksum only over the final signed and notarized bytes.
6. Fail closed if any selected signing or notarization step fails. Never fall
   back to an ad-hoc or unsigned artifact in a signing-enabled release.

### Not implemented: signing the bare `jeliyad` archives

The five per-target daemon archives from the `build` matrix are unsigned. A
future signing change requires a separate platform review. Outline:

1. Import the Developer ID certificate into a temporary keychain on the macOS
   build jobs.
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
