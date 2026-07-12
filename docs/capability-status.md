---
type: "Status Report"
title: "Capability status"
description: "Evidence-aware capability matrix for the v0.5.0 technical-preview candidate and the latest public release."
tags: ["capabilities", "release", "status", "verification"]
timestamp: "2026-07-12T16:40:00Z"
status: "canonical"
implementation_status: "partial"
verification_status: "partial"
release_status: "unreleased"
audience: ["contributors", "maintainers", "operators", "release-engineers"]
---

# Capability status

This page separates implementation, verification, and public availability. The
`v0.5.0` candidate is **blocked**: passing local and network tests do not make an
unpublished revision releasable.

## Snapshot boundary

| Field | Value |
|---|---|
| Candidate milestone | `v0.5.0 — Evidence-Backed Technical Preview` |
| Audited baseline | `1285b42037a3713840955fa518f2b81b19f2929f` |
| Hardened implementation snapshot | `8caeaf5f2d067230f66be757c415569e8e5e325f` on `hardening/v0.5.0-evidence-preview` |
| Public Jeliya `iroh-rooms` pin | `3cb9bfd1e43eb755c967315c37b6d4fd1c2bf020` (room-isolation defect remains) |
| Local upstream remediation | `3702e8cbcd5ac1808791124dd6bc44068be5f822` (clean and tested, but unpublished) |
| Network verification snapshot | Jeliya `fe870c7c5b63f2bf52b031dd1bc8e27e83183be5` with local upstream `3702e8c…` (clean, but unpublished) |
| Latest public release | `v0.4.3` at `9d62c3cd98c7f21d9683815c28278b6ac8c0b97f` |
| Status captured | 2026-07-12 16:40 UTC |

See [Release versus main](release-vs-main.md) for the revision boundaries and
[Verification evidence](verification-evidence.md) for the complete ledger.

## Capability matrix

| Capability | Implementation | Verification | Public release | Honest current claim |
|---|---|---|---|---|
| `jeliyad` with embedded React UI | implemented | partial | released in `v0.4.3` | Source-bound `v0.5.0` macOS x86_64 and Linux x86_64 musl builds passed the retained network runs; a complete five-target `v0.5.0` artifact set does not exist. |
| Identity, room create/join/open, membership, and messages | implemented | functional pass on unpublished revisions | released in `v0.4.3` | Three-peer join, message convergence, reconnect, and resynchronization passed in both retained runs. The evidence is unsigned and non-certifiable. |
| Files and BLAKE3 fetch verification | implemented | functional pass on unpublished revisions | released in `v0.4.3` | Cross-network transfer, byte equality, and hash verification passed in both retained runs. |
| Pipes | implemented | functional pass on unpublished revisions | released in `v0.4.3` | Authorized transfer, closure, and zero target bytes from the unauthorized third peer passed in both retained runs. |
| Direct cross-network P2P | implemented | functional pass, not release-qualifying | released in `v0.4.3` | [Direct run `d3d9ff69`](evidence/v0.5.0/direct.json) passed 36/36 assertions across three distinct public egresses and two ASNs. It used unpublished Jeliya and upstream revisions and is unsigned. |
| Deliberately forced relay | implemented behind a test-only verification seam | functional pass, not release-qualifying | runtime relay support released in `v0.4.3` | [Relay run `f1d9c149`](evidence/v0.5.0/relay.json) attested relay-only binaries and passed 36/36 assertions. It has the same publication and signature limitations. |
| Public room-scoped RPC isolation | implemented in hardened candidate | verified locally and exercised remotely | unreleased | A centralized guard covers the public room-scoped surface. Seventeen negative RPC checks, local-file denial, and aggregate filtering passed; foreign agent projections were exercised. |
| Room isolation in upstream synchronization | remediated only in local upstream checkout | local malicious-sync tests pass; retained runs do not claim synchronization isolation | unreleased | Jeliya still publicly pins vulnerable `3cb9bfd…`. `3702e8c…` must be reviewed, published, and pinned before qualification. |
| Agent runner and fleet | implemented | local pass | released as source in `v0.4.3` | Agent E2E passes; the earlier fleet stability run passed 5/5. Linux orphan/zombie process-group cleanup was verified on `demo1` under UID `65534`. |
| Android in-process FFI engine | implemented | local device smoke only | unreleased | App-private identity state is excluded from cloud backup and device transfer. It is not Android Keystore-backed, and Android direct, relay, NAT, and cross-network behavior remain unverified. |
| Dependency security | gates implemented | Cargo and npm report zero vulnerabilities | unreleased candidate gates | Four unmaintained/yanked dependency warnings have documented owners, mitigations, and expiry; no reachable unresolved high/critical vulnerability is accepted. |
| CI matrix | implemented | local component gates pass; hosted proof absent | unreleased | Rust, MSRV, TypeScript, Dart, Flutter, docs, smoke, sidecar, agent, fleet, protocol, and security jobs are defined. A manual non-publishing dispatch exists, and Gradle is checksum-verified; two clean hosted CI runs have not occurred because nothing was pushed. |
| Unix installer integrity | implemented | behavioral checks pass | unreleased | Unix installers fetch and verify the matching sidecar before extraction. |
| Windows installer integrity | implemented structurally | static checks only | unreleased | PowerShell structure is checked; Windows execution and reparse-point behavior still require a Windows runner. |
| Atomic, version-consistent publication | implemented locally | workflow checks pass; workflow never executed | unreleased | Builders are read-only and publication is centralized, but no `v0.5.0` release has been built or published. The evidence public key is intentionally absent, so publication fails closed. |
| WCAG 2.1 AA | partial | targeted checks only | partial | WCAG is a design target, not an enforced or certified conformance claim across React and Flutter. |
| OKF-compatible documentation | implemented | locally checked; final release reconciliation pending | unreleased | The profile separates lifecycle, implementation, verification, and release status. |

## Preview publication rule

`v0.5.0` may publish only `jeliyad` with its embedded web UI, and only after all
required daemon target gates pass. Native Flutter applications, DMG, APK/AAB,
Homebrew app cask, iOS artifacts, and a separately packaged agent runner are out
of scope.

No row becomes `verified` because code exists, and no row becomes `released`
because it is on a branch. A public immutable dependency pin, signed retained
evidence, passing hosted gates, matching tag, complete verified artifact set,
and explicit release authority are still required. See
[Known gaps and roadmap](known-gaps-roadmap.md).
