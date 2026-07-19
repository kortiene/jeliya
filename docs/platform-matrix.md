---
type: "Status Report"
title: "Platform matrix"
description: "Implementation, verification, packaging, and release status for every Jeliya runtime and target platform."
tags: ["packaging", "platforms", "release", "verification"]
timestamp: "2026-07-19T21:49:56Z"
status: "canonical"
implementation_status: "partial"
verification_status: "partial"
release_status: "partial"
audience: ["contributors", "maintainers", "operators", "release-engineers"]
---

# Platform matrix

The latest public release is `v0.6.0` (2026-07-16, daemon-only prerelease at
`2283a441...` with certified network evidence). Designated v0.6.1 source
candidate `a1af1cdc...` pins `iroh-rooms` to untagged `a5d98b70...`; local
exact-revision, loopback, and hosted qualification pass, but signed
direct/relay reruns are pending. The retained
2026-07-16 runs certify released v0.6.0 source `55024a4...` + `71fbb500...`
only. A source build or passing test is not a release.

## Daemon and embedded web UI

| Target | Implementation | Network/release evidence | Latest public artifact | Preview status |
|---|---|---|---|---|
| macOS arm64 (`aarch64-apple-darwin`) | implemented | archive built and verified by the v0.6.0 release workflow; no platform-specific network run | `v0.6.0` archive and sidecar | released; platform network run still absent |
| macOS x86_64 (`x86_64-apple-darwin`) | implemented | certifying signed schema 2 direct and relay runs pass (operator role); installer behavior passes | `v0.6.0` archive and sidecar | certified and released for `v0.6.0`; v0.6.1 candidate pending |
| Linux arm64 musl (`aarch64-unknown-linux-musl`) | implemented | archive built and verified by the v0.6.0 release workflow; no platform-specific network run | `v0.6.0` archive and sidecar | released; platform network run still absent |
| Linux x86_64 musl (`x86_64-unknown-linux-musl`) | implemented | certifying signed schema 2 direct and relay runs pass on Ubuntu x86_64 under UID `65534`; installer behavior passes | `v0.6.0` archive and sidecar | certified and released for `v0.6.0`; v0.6.1 candidate pending |
| Windows x86_64 MSVC (`x86_64-pc-windows-msvc`) | implemented | hosted behavioral installer/checksum/tamper, simulated reparse, and native daemon smoke jobs pass on `main` | `v0.6.0` archive and sidecar | released; no platform network run |

The certifying [direct](evidence/v0.6.0/direct.json) and
[relay](evidence/v0.6.0/relay.json) schema 2 manifests bind macOS x86_64 and
Linux x86_64 musl builds to Jeliya `55024a4…`, published Iroh Rooms pin
`71fbb500…`, and the verified toolchain; both are signed and set
`certifiable: true` for released v0.6.0. They do not transfer to
`a1af1cdc…` + `a5d98b70…`. The `v0.5.0` manifests
([direct](evidence/v0.5.0/direct.json), [relay](evidence/v0.5.0/relay.json))
bind the released pair `c5f740e…` + `d0ceb0b…` and do not transfer to another
pin. The earlier unsigned
[preview manifest](evidence/v0.5.0/preview-direct-schema2.json) at `0f6769a…`
with pre-remediation pin `3cb9bfd…` remains historical.

The older schema 1
[direct](evidence/v0.5.0/historical-schema1-direct.json) and
[relay](evidence/v0.5.0/historical-schema1-relay.json) manifests use Jeliya
`fe870c7…` and local upstream `3702e8c…`. They remain historical
local-remediation evidence only. See
[Verification evidence](verification-evidence.md).

## Native applications and source-only tools

| Surface | Implementation | Verification evidence | Release status | `v0.6.0` decision |
|---|---|---|---|---|
| macOS Flutter app | application and DMG pipeline exist | local tests only; current app sidecar is loopback-only; signing/notarization inactive | no DMG published | excluded |
| Linux Flutter app | GTK application, XDG storage policy, bundled-sidecar CMake contract, and host-architecture source packaging exist | Ubuntu 24.04 ARM64 local qualification passes; the x86_64 hosted gate passed on public `main` run `29704754961` at `a1af1cdc…`; Wayland remains pending | no native Linux app archive published; the local ARM64 daemon requires GLIBC 2.39 and the tarball lacks a complete Rust dependency license inventory | excluded |
| Android Flutter app with in-process Rust engine | application and three-ABI build path exist | Android 13 local lifecycle/FFI smoke only; no cross-network, NAT, direct, or relay evidence | no APK/AAB published | excluded |
| Android identity storage | app-private no-backup storage with cloud and device-transfer exclusions | rules and validation pass | unreleased | included security control; not Keystore-backed |
| iOS app | no scaffold or engine build | none | none | excluded |
| Agent runner and fleet launcher | JavaScript scripts exist | agent E2E pass; fleet stability 5/5; Linux orphan/zombie cleanup verified remotely | source only | no separate artifact |
| Dart protocol package | source package exists | candidate unit, replay, and integration gates pass locally and on public `main` run `29704754961` | not published separately | source only |

## Network claims by runtime

| Runtime | Local protocol | Cross-network direct | Forced relay | Reconnect/resync |
|---|---|---|---|---|
| `jeliyad` on macOS x86_64 and Linux x86_64 | implemented | signed direct pass for released v0.6.0 source `55024a4…` + `71fbb500…`; designated v0.6.1 candidate `a1af1cdc…` pending rerun | signed relay pass with self-attestation for that released pair; designated v0.6.1 candidate pending rerun | local current-pin loopback passed at `a1af1cdc…`; signed v0.6.1 reconnect/resync pending |
| Other daemon targets | implemented | no candidate evidence | no candidate evidence | no candidate evidence |
| Android in-process engine | local device-smoke evidence | unverified | unverified | local lifecycle only; cross-peer unverified |
| macOS Flutter sidecar | loopback-only configuration | unsupported by current app configuration | unsupported by current app configuration | local sidecar lifecycle only |
| Linux Flutter sidecar | real networking configured through the bundled daemon | unverified | unverified | local ARM64 source-package lifecycle gate passes; cross-peer unverified |

The certifying runs qualify their recorded revision pairs exactly; they do not
transfer to the current candidate, whose pin differs. “Real networking enabled”
on Android means only `loopback: false`; it is not evidence of a peer path.

## Packaging trust status

`v0.6.0` contains five daemon archives and a SHA-256 sidecar for each. Its
Unix installers pass behavioral fail-closed tests for sidecar verification
before extraction, and the hosted Windows job exercises the PowerShell
installer, tamper rejection, a simulated reparse-point payload, and native
daemon startup.

The release workflow pins third-party actions, verifies downloaded Zig,
keeps build jobs read-only, validates and seals the complete set without
executing it, and runs smoke execution in a separate read-only job. The sole
writer verifies the sealed receipt without executing candidate bytes and
exposes its token only to the final publishing step. It executed exactly this
way to publish `v0.6.0`'s five-target set.
See [Release versus main](release-vs-main.md).
