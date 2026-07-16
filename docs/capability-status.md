---
type: "Status Report"
title: "Capability status"
description: "Evidence-aware capability matrix for the v0.5.0 technical-preview candidate and the latest public release."
tags: ["capabilities", "release", "status", "verification"]
timestamp: "2026-07-16T15:30:00Z"
status: "canonical"
implementation_status: "partial"
verification_status: "partial"
release_status: "partial"
audience: ["contributors", "maintainers", "operators", "release-engineers"]
---

# Capability status

This page separates implementation, verification, and public availability.
`v0.5.0` shipped on 2026-07-14 as a daemon-only prerelease backed by signed,
certifying direct and forced-relay evidence. Current `main` is the
`v0.6.0` candidate: it repins `iroh-rooms` to `v0.1.0-rc.3`. The `v0.5.0`
evidence does not transfer to that pin, so fresh certifying direct and
forced-relay runs were executed at the candidate on 2026-07-16; `v0.6.0` is
qualified but not yet published.

## Snapshot boundary

| Field | Value |
|---|---|
| Released milestone | `v0.5.0 — Evidence-Backed Technical Preview`, published 2026-07-14 as a prerelease: five daemon+embedded-UI archives with `.sha256` sidecars |
| Network-qualified commit (`v0.6.0` candidate) | `55024a46b3e112796ba2acf1dc408dab26dbba2e` with `iroh-rooms` pin `71fbb500…` (published tag `v0.1.0-rc.3`) |
| Certified evidence (`v0.6.0` candidate) | signed schema 2 direct (`1ca39cfa`) and forced-relay (`cf28bc63`) runs of 2026-07-16; `certifiable: true`; see [Verification evidence](verification-evidence.md) |
| Candidate `iroh-rooms` pin (`main`) | `71fbb5007bef4ce83631c94762ec68c2beef3d79` — published tag `v0.1.0-rc.3`; adds the join-after-conversation fix, the join-bootstrap capability gate, size-independent membership sync, and deep gap healing |
| Candidate network verification | certified — signed direct and forced-relay runs bind `55024a4` + `71fbb500`. The superseded `v0.5.0` evidence binds `c5f740e` + `d0ceb0b` and does not transfer. Neither run certifies room-scoped synchronization isolation (`synchronization_isolation_claimed: false`); that rests on the upstream suite at the pin |
| Historical network verification | schema 1 runs at Jeliya `fe870c7…` with local upstream `3702e8c…`, and the schema 2 preview run at `0f6769a…` with pre-remediation pin `3cb9bfd…`; functional evidence only |
| Status captured | 2026-07-16 21:00 UTC |

See [Release versus main](release-vs-main.md) for the revision boundaries and
[Verification evidence](verification-evidence.md) for the complete ledger.
The released `v0.5.0` pins `d0ceb0b…`, which predates upstream's
join-after-conversation fix: an invite minted after any non-admin chat cannot
complete `room.join` on `v0.5.0` — the rc.3 repin on `main` fixes this for
the `v0.6.0` candidate. Mixed `v0.5.0`/candidate rooms cannot complete joins in
either direction, so a room's members, especially its admin, must move
together.

## Capability matrix

| Capability | Implementation | Verification | Public release | Honest current claim |
|---|---|---|---|---|
| `jeliyad` with embedded React UI | implemented | certified for `v0.5.0` | released in `v0.5.0` (prerelease) | The complete five-target daemon+embedded-UI archive set with `.sha256` sidecars is published. Signed schema 2 direct and forced-relay runs certified the released revision pair. |
| Identity, room create/join/open, membership, and messages | implemented | certified direct and relay pass for `v0.5.0` | released in `v0.5.0` | Three-peer join, message convergence, reconnect, and resynchronization passed in both certifying runs. Known `v0.5.0` limitation: its pin predates upstream's join-after-conversation fix, so an invite minted after non-admin chat cannot complete `room.join`; the rc.3 candidate on `main` fixes this and has no network run yet. |
| Files and BLAKE3 fetch verification | implemented | certified direct and relay pass for `v0.5.0` | released in `v0.5.0` | Cross-network transfer, byte equality, and hash verification passed in both certifying runs. |
| Pipes | implemented | certified direct and relay pass for `v0.5.0` | released in `v0.5.0` | Authorized transfer, closure, and zero target bytes from the unauthorized third peer passed in both certifying runs. |
| Direct cross-network P2P | implemented | certified for `v0.5.0` and for the `v0.6.0` candidate | released in `v0.5.0` | [Signed schema 2 direct run `1ca39cfa`](evidence/v0.6.0/direct.json) at `55024a4…` + `71fbb500…` passed every recorded assertion across three distinct public egresses and two ASNs; `certifiable: true`. The [`v0.5.0` run `3b86ac67`](evidence/v0.5.0/direct.json) certified the released pin pair `c5f740e…` + `d0ceb0b…`. |
| Deliberately forced relay | published seam pinned; verifier chain forwards through jeliya-core | certified for `v0.5.0` and for the `v0.6.0` candidate | released in `v0.5.0` | [Signed schema 2 relay run `cf28bc63`](evidence/v0.6.0/relay.json) at `55024a4…` + `71fbb500…`: the relay-only build self-attested on the operator host and both remote hosts, then every role held relay; `certifiable: true`. The [`v0.5.0` run `a3c76859`](evidence/v0.5.0/relay.json) certified the released pin pair. |
| Public room-scoped RPC isolation | implemented | verified locally and in both certifying runs | released in `v0.5.0` | A centralized guard covers the public room-scoped surface. Seventeen negative RPC checks, local-file denial, and aggregate filtering passed over the public network in the certifying runs. |
| Room isolation in upstream synchronization | remediated in the published pin certified for `v0.5.0` (`d0ceb0b…`); the rc.3 candidate keeps the fix and adds the join-bootstrap capability gate | certified by the `v0.5.0` runs; at the rc.3 tag the upstream isolation regressions and full suites pass locally (523/523 core, 232/232 net) | released in `v0.5.0` | The `v0.5.0` isolation claim is certified at `d0ceb0b…`. The rc.3 candidate is locally requalified; a fresh signed schema 2 qualification is required before the claim transfers to the next release. |
| Agent runner and fleet | implemented | local pass | released as source through `v0.5.0` | Agent E2E passes; the earlier fleet stability run passed 5/5. Linux orphan/zombie process-group cleanup was verified on `demo1` under UID `65534`. |
| Linux Flutter desktop app | implemented for host-native source builds | Ubuntu 24.04 ARM64 release build, X11/Xvfb lifecycle, sidecar smoke, dependency, archive, and checksum gates pass locally; the equivalent x86_64 hosted gate and Wayland lifecycle have no result yet | unreleased | The GTK app and path-relocatable sidecar bundle are source-supported. The local ARM64 daemon requires GLIBC 2.39, and the tarball lacks a complete Rust dependency license inventory. Linux enables the real network path, but direct, relay, NAT, and cross-network behavior remain unverified. No native Linux app artifact is public. |
| Android in-process FFI engine | implemented | local device smoke only | unreleased | App-private identity state is excluded from cloud backup and device transfer. It is not Android Keystore-backed, and Android direct, relay, NAT, and cross-network behavior remain unverified. |
| Dependency security | gates implemented | Cargo and npm report zero vulnerabilities | release gates passed for `v0.5.0` | Four unmaintained/yanked dependency warnings have documented owners, mitigations, and expiry; no reachable unresolved high/critical vulnerability is accepted. |
| CI matrix | implemented | hosted runs pass on `main` | exercised for `v0.5.0` | Rust, MSRV, TypeScript, Dart, Flutter, Linux native packaging, docs, smoke, sidecar, agent, fleet, protocol, and security jobs are defined; the six pre-existing required jobs pass on hosted `main` runs, including Windows installer integrity. The new `linux-flutter` job (release binaries, integrated bundle under Xvfb, dependency/archive/checksum checks) has no hosted execution yet. |
| Unix installer integrity | implemented | behavioral checks pass | released in `v0.5.0` | Unix installers fetch and verify the matching sidecar before extraction; `v0.5.0` installs via the version-pinned installer path. |
| Windows installer integrity | implemented | hosted `windows-latest` job passes on `main` | released in `v0.5.0` | The Windows job executes checksum/tamper behavior, simulates reparse-point rejection, and runs native `jeliyad.exe --version`; a `v0.5.0` Windows zip and sidecar are published. |
| Complete asset-set visibility and version consistency | implemented | executed for `v0.5.0` | released in `v0.5.0` | The publication workflow validated, sealed, smoked, and receipt-verified the complete five-archive set; the evidence key is provisioned and the signed evidence passed the release gate before publication. |
| WCAG 2.1 AA | partial | targeted checks only | partial | WCAG is a design target, not an enforced or certified conformance claim across React and Flutter. |
| OKF-compatible documentation | implemented | locally checked; reconciled to the released `v0.5.0` and the rc.3 candidate | released posture documented | The profile separates lifecycle, implementation, verification, and release status. |

## Preview publication rule

`v0.5.0` published only `jeliyad` with its embedded web UI, after all required
daemon target gates passed — the rule held. Native Flutter applications, DMG,
Linux app tarballs, APK/AAB, Homebrew app cask, iOS artifacts, and a
separately packaged agent runner remain out of scope until their own platform
gates are satisfied.

No row becomes `verified` because code exists, and no row becomes `released`
because it is on a branch. For the next release the same bar applies to the
rc.3 candidate: fresh signed network evidence at its pin, passing hosted gates
(including the new `linux-flutter` job), a matching tag, a complete verified
artifact set, and explicit release authority. See
[Known gaps and roadmap](known-gaps-roadmap.md).
