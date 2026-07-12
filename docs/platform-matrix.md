---
type: "Status Report"
title: "Platform matrix"
description: "Implementation, verification, packaging, and release status for every Jeliya runtime and target platform."
tags: ["packaging", "platforms", "release", "verification"]
timestamp: "2026-07-12T22:00:46Z"
status: "canonical"
implementation_status: "partial"
verification_status: "partial"
release_status: "partial"
audience: ["contributors", "maintainers", "operators", "release-engineers"]
---

# Platform matrix

The latest public release is `v0.4.3`. The `v0.5.0` candidate remains blocked
and has no public artifacts. A source build or passing test is not a release.

## Daemon and embedded web UI

| Target | Implementation | `v0.5.0` evidence | Latest public artifact | Preview status |
|---|---|---|---|---|
| macOS arm64 (`aarch64-apple-darwin`) | implemented | no candidate archive or platform run | `v0.4.3` archive and sidecar | required, pending |
| macOS x86_64 (`x86_64-apple-darwin`) | implemented | embedded-UI source build and direct/relay operator run pass; installer behavior passes | `v0.4.3` archive and sidecar | functional evidence only |
| Linux arm64 musl (`aarch64-unknown-linux-musl`) | implemented | no candidate archive or platform run | `v0.4.3` archive and sidecar | required, pending |
| Linux x86_64 musl (`x86_64-unknown-linux-musl`) | implemented | embedded-UI source build; direct and relay runs pass on Ubuntu 22.04 x86_64 under UID `65534`; installer behavior passes | `v0.4.3` archive and sidecar | functional evidence only |
| Windows x86_64 MSVC (`x86_64-pc-windows-msvc`) | implemented | behavioral installer/checksum/tamper and simulated reparse gates plus native daemon smoke are configured; no hosted candidate result | `v0.4.3` archive and sidecar | required, pending execution |

The two retained schema 1 network manifests bind builds to Jeliya `fe870c7…`,
local upstream `3702e8c…`, Rust `1.91.0`, Node `22.22.3`, an independently
digest-checked Zig `0.15.2` executable, and the embedded UI. They predate the
schema 2 isolated build environment and complete Zig archive verification,
cover only macOS x86_64 and Linux x86_64 musl, are unsigned, and set
`certifiable: false`. See
[Verification evidence](verification-evidence.md).

## Native applications and source-only tools

| Surface | Implementation | Verification evidence | Release status | `v0.5.0` decision |
|---|---|---|---|---|
| macOS Flutter app | application and DMG pipeline exist | local tests only; current app sidecar is loopback-only; signing/notarization inactive | no DMG published | excluded |
| Android Flutter app with in-process Rust engine | application and three-ABI build path exist | Android 13 local lifecycle/FFI smoke only; no cross-network, NAT, direct, or relay evidence | no APK/AAB published | excluded |
| Android identity storage | app-private no-backup storage with cloud and device-transfer exclusions | rules and validation pass | unreleased | included security control; not Keystore-backed |
| iOS app | no scaffold or engine build | none | none | excluded |
| Agent runner and fleet launcher | JavaScript scripts exist | agent E2E pass; fleet stability 5/5; Linux orphan/zombie cleanup verified remotely | source only | no separate artifact |
| Dart protocol package | source package exists | candidate unit, replay, and integration gates are implemented locally; hosted result pending | not published separately | source only |

## Network claims by runtime

| Runtime | Local protocol | Cross-network direct | Forced relay | Reconnect/resync |
|---|---|---|---|---|
| `jeliyad` on macOS x86_64 and Linux x86_64 | implemented | 36/36 functional pass across three distinct egresses and two ASNs | 36/36 functional pass with relay-only attestation | pass in both retained runs |
| Other daemon targets | implemented | no candidate evidence | no candidate evidence | no candidate evidence |
| Android in-process engine | local device-smoke evidence | unverified | unverified | local lifecycle only; cross-peer unverified |
| macOS Flutter sidecar | loopback-only configuration | unsupported by current app configuration | unsupported by current app configuration | local sidecar lifecycle only |

The daemon runs are not release-qualifying because their Jeliya and upstream
revisions are unpublished and their manifests are unsigned. “Real networking
enabled” on Android means only `loopback: false`; it is not evidence of a peer
path.

## Packaging trust status

`v0.4.3` contains five daemon archives and a SHA-256 sidecar for each. Its
installers do not automatically enforce those sidecars before extraction. The
candidate Unix installer now passes behavioral fail-closed tests for sidecar
verification. Windows jobs now exercise the PowerShell installer, tamper
rejection, a simulated reparse-point payload, and native daemon startup, but
they have not run in a hosted Windows environment for this candidate.

The candidate workflow pins third-party actions, verifies downloaded Zig,
keeps build jobs read-only, validates and seals the complete set without
executing it, and runs smoke execution in a separate read-only job. The sole
writer verifies the sealed receipt without executing candidate bytes and
exposes its token only to the final publishing step. It has never been executed
to publish `v0.5.0`, and no complete five-target candidate set has been built.
See [Release versus main](release-vs-main.md).
