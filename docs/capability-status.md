---
type: "Status Report"
title: "Capability status"
description: "Evidence-aware capability matrix for the v0.5.0 technical-preview candidate and the latest public release."
tags: ["capabilities", "release", "status", "verification"]
timestamp: "2026-07-12T12:21:59Z"
status: "canonical"
implementation_status: "partial"
verification_status: "partial"
release_status: "unreleased"
audience: ["contributors", "maintainers", "operators", "release-engineers"]
---

# Capability status

This page is the concise source of truth for what Jeliya implements, what has
evidence, and what users can obtain from a public release. It applies the four
independent status axes defined in [`PROFILE.md`](PROFILE.md).

## Snapshot boundary

| Field | Value |
|---|---|
| Candidate milestone | `v0.5.0 — Evidence-Backed Technical Preview` |
| Baseline Git commit | `1285b42037a3713840955fa518f2b81b19f2929f` |
| Baseline `iroh-rooms` revision | `3cb9bfd1e43eb755c967315c37b6d4fd1c2bf020` |
| Candidate branch | `hardening/v0.5.0-evidence-preview` |
| Latest public release | `v0.4.3` at `9d62c3cd98c7f21d9683815c28278b6ac8c0b97f` |
| Status captured | 2026-07-12 UTC |

The hardening branch contains uncommitted work and is not a verified revision.
An upstream room-scoped synchronization remediation exists only on a local
branch at the time of this snapshot; it is neither published nor pinned by this
repository. See [`verification-evidence.md`](verification-evidence.md) for the
candidate evidence ledger.

## Capability matrix

| Capability | Implementation | Verification | Public release | Honest current claim |
|---|---|---|---|---|
| `jeliyad` with embedded React UI | implemented | partial | released in `v0.4.3` | Five daemon archives exist, but the `v0.5.0` build and two clean verification cycles are pending. |
| Identity, room create/join/open, signed membership, and messages | implemented | partial | released in `v0.4.3` | Local and historical tests exist; the candidate's full negative-authorization and real-network suite is pending. |
| Files and BLAKE3 fetch verification | implemented | partial | released in `v0.4.3` | Local behavior exists; candidate cross-network transfer and hash evidence is pending. |
| Pipes | implemented | partial | released in `v0.4.3` | Local behavior exists; candidate direct, relay, reconnect, and authorization evidence is pending. |
| Direct P2P path | implemented | historical | released in `v0.4.3` | One 2026-07-04 run observed `direct` on older Jeliya and `iroh-rooms` revisions. It does not certify this candidate. |
| Relay fallback | implemented | unverified for this milestone | released in `v0.4.3` | The runtime exposes relay paths, but a deliberately constrained candidate run has not yet been recorded. |
| Cross-room read isolation in public RPCs | partial candidate hardening | unverified | unreleased | Centralized room-access work is in the candidate working tree; all read RPCs and negative E2E assertions must still pass on a committed revision. |
| Cross-room isolation in synchronization | partial local upstream remediation | unverified | unreleased | The pinned upstream revision can serve a foreign-room event by known ID from a shared store. A room-scoped fix is local and cannot certify Jeliya until published and pinned. |
| Agent runner, task claims, fleet reads, and dashboards | implemented | partial | released in `v0.4.3` | The implementation exists; the flaky agent assertion and candidate agent/fleet E2E gates remain to be stabilized and repeated. |
| Android in-process FFI engine | implemented | partial | unreleased | A physical Android 13 smoke proved local lifecycle, room operations, pushes, and persistence with real mode configured. Cross-network P2P, relay, and NAT behavior were not tested. |
| macOS Flutter application and DMG pipeline | implemented | partial | unreleased | Source and packaging work exist, but no native app artifact has been published. It is unconditionally excluded from `v0.5.0`. |
| iOS application | planned | unverified | unreleased | No iOS scaffold or engine build exists. |
| WCAG 2.1 AA behavior | partial | partial | partial | Design rules and targeted tests exist; there is no complete automated and manual conformance record across React and Flutter. |
| Unix and Windows installer integrity verification | implemented in candidate | unverified | unreleased | Candidate installers fail closed unless the exact archive's checksum sidecar validates before extraction; adversarial tests and release artifacts are pending. `v0.4.3` installer code did not enforce this. |
| Atomic, version-consistent release publication | partial candidate hardening | unverified | unreleased | The candidate workflow must build and validate the full artifact set before a single publishing job receives write permission. |
| OKF-compatible documentation wiki | implemented | verified locally | unreleased | Metadata separates document, implementation, verification, and release state; publication with `v0.5.0` is pending. |

## Preview publication rule

For `v0.5.0`, the only intended public product artifact is `jeliyad` with the
embedded web UI for the five currently supported daemon targets. Source code,
test harnesses, and documentation may accompany the tag. The macOS Flutter
application, DMG, Android APK/AAB, Homebrew app cask, and any iOS artifact are
unconditionally outside this milestone and must remain unpublished.

No row may move to `verified` because code exists, and no row may move to
`released` because it is on `main`. A public tag, matching artifact, and
release record are required.
