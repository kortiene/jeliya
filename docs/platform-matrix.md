---
type: "Status Report"
title: "Platform matrix"
description: "Implementation, verification, packaging, and release status for every Jeliya runtime and target platform."
tags: ["packaging", "platforms", "release", "verification"]
timestamp: "2026-07-12T12:21:59Z"
status: "canonical"
implementation_status: "partial"
verification_status: "partial"
release_status: "partial"
audience: ["contributors", "maintainers", "operators", "release-engineers"]
---

# Platform matrix

This matrix distinguishes code that can be built from code that has been
verified and from artifacts that have actually been published. The latest
public release is `v0.4.3`; the `v0.5.0` candidate remains unreleased.

## Daemon and embedded web UI

| Target | Implementation | Candidate verification | Latest public artifact | `v0.5.0` scope |
|---|---|---|---|---|
| macOS arm64 (`aarch64-apple-darwin`) | implemented | local build, smoke, and installer verification pending | `v0.4.3` daemon archive plus checksum sidecar | included |
| macOS x86_64 (`x86_64-apple-darwin`) | implemented | local build, smoke, and installer verification pending | `v0.4.3` daemon archive plus checksum sidecar | included |
| Linux arm64 musl (`aarch64-unknown-linux-musl`) | implemented | cross-build, checksum, and smoke pending | `v0.4.3` daemon archive plus checksum sidecar | included |
| Linux x86_64 musl (`x86_64-unknown-linux-musl`) | implemented | cross-build, remote smoke, checksum, and installer verification pending | `v0.4.3` daemon archive plus checksum sidecar | included |
| Windows x86_64 MSVC (`x86_64-pc-windows-msvc`) | implemented | build, PowerShell installer, checksum, and smoke pending | `v0.4.3` daemon archive plus checksum sidecar | included |

The embedded React UI is built before Rust and compiled into each daemon
archive. There is no separately released web bundle. `v0.5.0` must not publish
a daemon archive unless the embedded UI and its provenance match the candidate
commit.

## Native applications and source-only tools

| Surface | Implementation | Verification evidence | Release status | `v0.5.0` decision |
|---|---|---|---|---|
| macOS Flutter app | feature implementation and DMG pipeline exist | Flutter tests and packaging checks are incomplete for the candidate; Developer ID and notarization are not active | no DMG or app release has been published | excluded |
| Android Flutter app with in-process Rust engine | app and three ABI library build path exist | one Android 13 local engine/UI smoke; no different-network peer, direct path, relay path, or NAT evidence | no APK or AAB published | excluded |
| iOS app | no scaffold or engine build | none | none | excluded |
| Agent runner and fleet launcher | JavaScript source scripts exist | candidate agent E2E and fleet E2E pending; no separate package conformance gate | distributed as source only | no separate artifact |
| Dart protocol package | source package exists | unit, FFI host replay, and Flutter integration gates pending for candidate | not published as a package artifact | source only |

## Network claims by platform

| Platform/runtime | Local protocol | Cross-network direct | Forced relay | Reconnect/resync |
|---|---|---|---|---|
| `jeliyad` on desktop/server | implemented; candidate rerun pending | historical evidence only; candidate pending | candidate pending | candidate pending |
| Android in-process engine | device-smoke evidence for local operations | unverified | unverified | app-resume resync implementation exists; cross-peer evidence pending |
| macOS Flutter sidecar | local loopback mode | not supported by the current app configuration | not supported by the current app configuration | local sidecar lifecycle tests only |

The phrase “real networking enabled” means only that the engine was configured
with `loopback: false`. It is not evidence of a successful remote peer path.

## Packaging trust status

`v0.4.3` publishes five unsigned daemon archives and one SHA-256 sidecar per
archive. Those sidecars enable manual verification, but installer code at the
`v0.4.3` tag does not verify them automatically before extraction. The
candidate installers now fail closed on a missing, malformed, mismatched, or
incorrect sidecar before extraction; adversarial tests and final release
artifacts remain pending. Immutable workflow dependencies, verified downloaded
build tools, atomic publication, and tag/version/name consistency are mandatory
candidate gates, not properties attributed retroactively to `v0.4.3`.
