---
type: "Status Report"
title: "Capability status"
description: "Evidence-aware capability matrix for the v0.5.0 technical-preview candidate and the latest public release."
tags: ["capabilities", "release", "status", "verification"]
timestamp: "2026-07-19T15:15:00Z"
status: "canonical"
implementation_status: "partial"
verification_status: "partial"
release_status: "partial"
audience: ["contributors", "maintainers", "operators", "release-engineers"]
---

# Capability status

This page separates implementation, verification, and public availability.
`v0.5.0` shipped on 2026-07-14 as a daemon-only prerelease backed by signed,
certifying direct and forced-relay evidence. The current `v0.6.0` source
candidate repins `iroh-rooms` to untagged upstream revision `a5d98b70...`, the
first `main` merge carrying the provisional-peer and store-degradation fixes.
It is locally qualified but not network-qualified or published. Signed runs at
the earlier `55024a4...` + `71fbb500...` snapshot remain valid for that exact
pair and do not transfer to the current dependency.

## Snapshot boundary

| Field | Value |
|---|---|
| Released milestone | `v0.5.0 — Evidence-Backed Technical Preview`, published 2026-07-14 as a prerelease: five daemon+embedded-UI archives with `.sha256` sidecars |
| Current source candidate | `105744b6c27633e5ccc576d86f1a15e3fe443b94` with Iroh Rooms `a5d98b70d717f35d3ce60953a88e12e646f2e871` |
| Last network-qualified `v0.6.0` snapshot | Jeliya `55024a46b3e112796ba2acf1dc408dab26dbba2e` with Iroh Rooms `71fbb5007bef4ce83631c94762ec68c2beef3d79` (tag `v0.1.0-rc.3`) |
| Retained certified evidence | signed schema 2 direct (`1ca39cfa`) and forced-relay (`cf28bc63`) runs of 2026-07-16; valid for the last network-qualified snapshot only |
| Candidate `iroh-rooms` pin | `a5d98b70d717f35d3ce60953a88e12e646f2e871` — deliberately untagged first merge carrying the `kortiene/iroh-room#121` and `kortiene/iroh-room#119` fixes plus `kortiene/iroh-room#126` follow-ups; later `main` changes only an unconsumed CLI crate |
| Candidate verification | exact-revision upstream fanout, isolation, and store-degradation tests pass; full Jeliya workspace and 67-assertion loopback suites pass. Fresh signed direct and forced-relay evidence is required |
| Historical network verification | schema 1 runs at Jeliya `fe870c7…` with local upstream `3702e8c…`, and the schema 2 preview run at `0f6769a…` with pre-remediation pin `3cb9bfd…`; functional evidence only |
| Status captured | 2026-07-19 15:15 UTC |

See [Release versus main](release-vs-main.md) for the revision boundaries and
[Verification evidence](verification-evidence.md) for the complete ledger.
The released `v0.5.0` pins `d0ceb0b…`, which predates upstream's
join-after-conversation fix: an invite minted after any non-admin chat cannot
complete `room.join` on `v0.5.0` — the current repin carries that fix plus the
later provisional-peer and store-degradation fixes. Mixed `v0.5.0`/candidate
rooms cannot complete joins in either direction, so a room's members,
especially its admin, must move together.

## Capability matrix

| Capability | Implementation | Verification | Public release | Honest current claim |
|---|---|---|---|---|
| `jeliyad` with embedded React UI | implemented | certified for `v0.5.0` | released in `v0.5.0` (prerelease) | The complete five-target daemon+embedded-UI archive set with `.sha256` sidecars is published. Signed schema 2 direct and forced-relay runs certified the released revision pair. |
| Identity, room create/join/open, membership, and messages | implemented | certified direct and relay pass for `v0.5.0`; current pin passes local integration | released in `v0.5.0` | Known `v0.5.0` limitation: an invite minted after non-admin chat cannot complete `room.join`. The current candidate fixes this and passes all 67 loopback assertions; fresh current-pin network runs remain required. |
| Files and BLAKE3 fetch verification | implemented | certified direct and relay pass for `v0.5.0` | released in `v0.5.0` | Cross-network transfer, byte equality, and hash verification passed in both certifying runs. |
| Pipes | implemented | certified direct and relay pass for `v0.5.0` | released in `v0.5.0` | Authorized transfer, closure, and zero target bytes from the unauthorized third peer passed in both certifying runs. |
| Direct cross-network P2P | implemented | certified for `v0.5.0` and the prior `v0.6.0` snapshot; current candidate pending | released in `v0.5.0` | [Signed schema 2 direct run `1ca39cfa`](evidence/v0.6.0/direct.json) certifies `55024a4…` + `71fbb500…`; it does not transfer to `105744b…` + `a5d98b70…`. |
| Deliberately forced relay | published seam pinned; verifier chain forwards through jeliya-core | certified for `v0.5.0` and the prior `v0.6.0` snapshot; current candidate pending | released in `v0.5.0` | [Signed schema 2 relay run `cf28bc63`](evidence/v0.6.0/relay.json) certifies `55024a4…` + `71fbb500…`; a new relay-only source build and signed run are required. |
| Public room-scoped RPC isolation | implemented | verified locally and in both certifying runs | released in `v0.5.0` | A centralized guard covers the public room-scoped surface. Seventeen negative RPC checks, local-file denial, and aggregate filtering passed over the public network in the certifying runs. |
| Upstream synchronization and provisional-peer isolation | remediated at current pin `a5d98b70…` | targeted isolation, provisional-fanout, and store-degradation regressions pass; core/net all-targets suite passes 806 tests with two ignores | released baseline in `v0.5.0`; new fixes unreleased | Local exact-revision evidence covers the upstream internals. Fresh signed network evidence is still required for Jeliya integration at the new pin. |
| Agent runner and fleet | implemented | local pass | released as source through `v0.5.0` | Agent E2E passes; the earlier fleet stability run passed 5/5. Linux orphan/zombie process-group cleanup was verified on `demo1` under UID `65534`. |
| Linux Flutter desktop app | implemented for host-native source builds | Ubuntu 24.04 ARM64 local qualification passes; the x86_64 hosted gate passed on public `main` at `a24f223…`; current-candidate rerun and Wayland lifecycle remain pending | unreleased | The GTK app and path-relocatable sidecar bundle are source-supported. The local ARM64 daemon requires GLIBC 2.39, and the tarball lacks a complete Rust dependency license inventory. Linux enables the real network path, but direct, relay, NAT, and cross-network behavior remain unverified. No native Linux app artifact is public. |
| Android in-process FFI engine | implemented | local device smoke only | unreleased | App-private identity state is excluded from cloud backup and device transfer. It is not Android Keystore-backed, and Android direct, relay, NAT, and cross-network behavior remain unverified. |
| Dependency security | gates implemented | Cargo and npm report zero vulnerabilities | release gates passed for `v0.5.0` | Four unmaintained/yanked dependency warnings have documented owners, mitigations, and expiry; no reachable unresolved high/critical vulnerability is accepted. |
| CI matrix | implemented | all jobs passed on public `main` run `29688515781` at `a24f223…`; current-candidate run pending | exercised for `v0.5.0` | Rust, MSRV, TypeScript, Dart, Flutter, Linux native packaging, docs, smoke, sidecar, agent, fleet, protocol, and security jobs are defined. The hosted run includes Windows installer integrity and the `linux-flutter` release-build, Xvfb, dependency, archive, and checksum checks. |
| Unix installer integrity | implemented | behavioral checks pass | released in `v0.5.0` | Unix installers fetch and verify the matching sidecar before extraction; `v0.5.0` installs via the version-pinned installer path. |
| Windows installer integrity | implemented | hosted `windows-latest` job passes on `main` | released in `v0.5.0` | The Windows job executes checksum/tamper behavior, simulates reparse-point rejection, and runs native `jeliyad.exe --version`; a `v0.5.0` Windows zip and sidecar are published. |
| Complete asset-set visibility and version consistency | implemented | executed for `v0.5.0` | released in `v0.5.0` | The publication workflow validated, sealed, smoked, and receipt-verified the complete five-archive set; the evidence key is provisioned and the signed evidence passed the release gate before publication. |
| WCAG 2.1 AA | partial | automated gate on every pull request; manual checklist per release | partial | Enforced, not certified. CI rejects any critical or serious axe violation across every destination at 1440x900, 920x800, 390x844 and 320x568, and fails on clipped layout at 100/200/320% text in English and French; `docs/accessibility-checklist.md` covers the screen-reader and keyboard behaviours automation cannot decide. Not a conformance claim: the web client is English-only until localization lands, and `ui-e2e` is not yet a required status check, so the gate runs on every pull request without blocking merge. |
| OKF-compatible documentation | implemented | locally checked; reconciled to the released `v0.5.0`, prior signed snapshot, and current untagged candidate | released posture documented | The profile separates lifecycle, implementation, verification, and release status. |

## Preview publication rule

`v0.5.0` published only `jeliyad` with its embedded web UI, after all required
daemon target gates passed — the rule held. Native Flutter applications, DMG,
Linux app tarballs, APK/AAB, Homebrew app cask, iOS artifacts, and a
separately packaged agent runner remain out of scope until their own platform
gates are satisfied.

No row becomes `verified` because code exists, and no row becomes `released`
because it is on a branch. For the next release the same bar applies to the
current candidate: fresh signed network evidence at `a5d98b70...`, passing
hosted gates (including the new `linux-flutter` job), a matching tag, a complete
verified artifact set, and explicit release authority. See
[Known gaps and roadmap](known-gaps-roadmap.md).
