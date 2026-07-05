# Signing and notarization plan (Phase 2)

Release binaries to date are intentionally unsigned (`v0.1.0`/`v0.2.0` were
released under the project's former name Bantaba — see `docs/naming.md`).
The `curl | sh` and
Homebrew paths install cleanly because they do not set the macOS quarantine bit,
but browser downloads can still trip Gatekeeper on macOS and SmartScreen on
Windows. This document tracks the work needed to ship signed desktop binaries.

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
- App Store Connect API key or Apple ID app-specific password for notarization.
- Team ID, key ID, issuer ID, and certificate password as GitHub secrets.

Suggested GitHub secrets:

| Secret | Purpose |
| --- | --- |
| `APPLE_CERTIFICATE_P12_BASE64` | Base64-encoded Developer ID `.p12`. |
| `APPLE_CERTIFICATE_PASSWORD` | Password for the `.p12`. |
| `APPLE_TEAM_ID` | Developer Team ID. |
| `APPLE_NOTARY_KEY_ID` | App Store Connect API key ID. |
| `APPLE_NOTARY_ISSUER_ID` | App Store Connect issuer ID. |
| `APPLE_NOTARY_KEY_BASE64` | Base64-encoded `.p8` API key. |

Workflow outline:

1. Import the Developer ID certificate into a temporary keychain on macOS jobs.
2. Build `jeliyad` with `embed-ui` as today.
3. `codesign --timestamp --options runtime --sign "Developer ID Application: ..." jeliyad`.
4. Package the signed binary into the `.tar.gz` asset.
5. Submit the archive or a zipped staging bundle with `xcrun notarytool submit --wait`.
6. Keep `.sha256` sidecars over the final signed/notarized asset bytes.

Notes:

- Notarization is most valuable for browser-downloaded macOS artifacts. Homebrew
  and `curl | sh` are less affected, but signed artifacts still improve trust.
- If the product later ships a `.app` wrapper, staple the notarization ticket to
  the app bundle/DMG. A bare CLI daemon archive has no app bundle to staple.

## Windows Authenticode signing

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
