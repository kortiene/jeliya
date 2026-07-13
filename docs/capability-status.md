---
type: "Status Report"
title: "Capability status"
description: "Evidence-aware capability matrix for the v0.5.0 technical-preview candidate and the latest public release."
tags: ["capabilities", "release", "status", "verification"]
timestamp: "2026-07-12T23:55:23Z"
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
| Hardened implementation and network-test snapshot | `0f6769a68d783cf6a5feba0e2db6111a212affa1` on `hardening/v0.5.0-evidence-preview` before final documentation reconciliation |
| Public Jeliya `iroh-rooms` pin | `3cb9bfd1e43eb755c967315c37b6d4fd1c2bf020` (room-isolation defect remains) |
| Local upstream remediation | `3702e8cbcd5ac1808791124dd6bc44068be5f822` (clean and tested, but unpublished) |
| Current network verification | Schema 2 direct run at Jeliya `0f6769a…` with public pin `3cb9bfd…`; forced-relay build blocked because that pin lacks the test seam |
| Historical network verification | Schema 1 direct and relay runs at Jeliya `fe870c7…` with local upstream `3702e8c…`; functional evidence only |
| Latest public release | `v0.4.3` at `9d62c3cd98c7f21d9683815c28278b6ac8c0b97f` |
| Status captured | 2026-07-12 23:55 UTC |

See [Release versus main](release-vs-main.md) for the revision boundaries and
[Verification evidence](verification-evidence.md) for the complete ledger.
The current schema 2 direct run covers the hardened implementation snapshot,
but it uses the unsafe public upstream pin and explicitly makes no
synchronization-isolation claim. The older schema 1 direct and relay runs use
the unpublished local remediation and remain historical functional evidence;
they do not qualify the current implementation or a release.

## Capability matrix

| Capability | Implementation | Verification | Public release | Honest current claim |
|---|---|---|---|---|
| `jeliyad` with embedded React UI | implemented | partial | released in `v0.4.3` | Schema 2 source-bound macOS x86_64 and Linux x86_64 musl builds passed the current direct run. The matching relay-only build failed closed, and a complete five-target `v0.5.0` artifact set does not exist. |
| Identity, room create/join/open, membership, and messages | implemented | current direct functional pass | released in `v0.4.3` | Three-peer join, message convergence, reconnect, and resynchronization passed in the current direct run. Current relay verification is blocked; the evidence is unsigned and non-certifiable. |
| Files and BLAKE3 fetch verification | implemented | current direct functional pass | released in `v0.4.3` | Cross-network transfer, byte equality, and hash verification passed in the current direct run. |
| Pipes | implemented | current direct functional pass | released in `v0.4.3` | Authorized transfer, closure, and zero target bytes from the unauthorized third peer passed in the current direct run. |
| Direct cross-network P2P | implemented | current functional pass, not release-qualifying | released in `v0.4.3` | [Schema 2 direct run `3c938c66`](evidence/v0.5.0/preview-direct-schema2.json) passed 36/36 assertions across three distinct public egresses and two ASNs at `0f6769a…`. It is unsigned, unpublished, and does not claim synchronization isolation. |
| Deliberately forced relay | test seam absent from the current public dependency pin | current verification blocked | runtime relay support released in `v0.4.3` | The exact public-pin relay-only build failed closed before remote execution. [Historical schema 1 run `f1d9c149`](evidence/v0.5.0/relay.json) passed only with the unpublished local upstream seam and does not qualify the current candidate. |
| Public room-scoped RPC isolation | implemented in hardened candidate | verified locally and exercised remotely | unreleased | A centralized guard covers the public room-scoped surface. Seventeen negative RPC checks, local-file denial, and aggregate filtering passed; foreign agent projections were exercised. |
| Room isolation in upstream synchronization | remediated only in local upstream checkout | local malicious-sync tests pass; retained runs do not claim synchronization isolation | unreleased | Jeliya still publicly pins vulnerable `3cb9bfd…`. `3702e8c…` must be reviewed, published, and pinned before qualification. |
| Agent runner and fleet | implemented | local pass | released as source in `v0.4.3` | Agent E2E passes; the earlier fleet stability run passed 5/5. Linux orphan/zombie process-group cleanup was verified on `demo1` under UID `65534`. |
| Android in-process FFI engine | implemented | local device smoke only | unreleased | App-private identity state is excluded from cloud backup and device transfer. It is not Android Keystore-backed, and Android direct, relay, NAT, and cross-network behavior remain unverified. |
| Dependency security | gates implemented | Cargo and npm report zero vulnerabilities | unreleased candidate gates | Four unmaintained/yanked dependency warnings have documented owners, mitigations, and expiry; no reachable unresolved high/critical vulnerability is accepted. |
| CI matrix | implemented | local component gates pass; hosted proof absent | unreleased | Rust, MSRV, TypeScript, Dart, Flutter, docs, smoke, sidecar, agent, fleet, protocol, and security jobs are defined. A manual non-publishing dispatch exists, and Gradle is checksum-verified; two clean hosted CI runs have not occurred because nothing was pushed. |
| Unix installer integrity | implemented | behavioral checks pass | unreleased | Unix installers fetch and verify the matching sidecar before extraction. |
| Windows installer integrity | behavioral gate configured | hosted execution absent | unreleased | The Windows job parses and executes checksum/tamper behavior, simulates reparse-point rejection, and the release matrix runs native `jeliyad.exe --version`; those jobs have not run on `windows-latest` for this candidate. |
| Complete asset-set visibility and version consistency | implemented locally | contract and receipt tests pass; workflow never executed | unreleased | A read-only job validates and seals the untouched complete set, a separate read-only job executes the smoke binary, and the sole writer verifies the receipt without executing candidate bytes. Its GitHub token is exposed only to the final publishing step. GitHub cannot transact the tag and release assets atomically; the finalizer instead keeps the release draft until every uploaded byte matches and requires operator inspection after an interrupted cleanup. No `v0.5.0` release has been built or published, and the absent evidence key keeps publication fail-closed. |
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
