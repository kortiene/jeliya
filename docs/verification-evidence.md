---
type: "Status Report"
title: "Verification evidence"
description: "Revision-bound verification ledger and evidence-recording contract for the v0.6.0 candidate."
tags: ["evidence", "networking", "release", "testing", "verification"]
timestamp: "2026-07-16T21:00:00Z"
status: "canonical"
implementation_status: "implemented"
verification_status: "verified"
release_status: "unreleased"
audience: ["contributors", "maintainers", "operators", "release-engineers"]
---

# Verification evidence

This ledger separates a functional test result from release-qualifying
evidence. A result is transferable to a release candidate only when it binds
the exact public Jeliya commit, public immutable dependency revisions,
environment, timestamps, assertions, retained sanitized manifest, and detached
signature. Both the direct and the forced-relay schema 2 runs now meet that
bar: each binds the published network-qualified Jeliya commit and the published,
remediated iroh-rooms revision, is retained as a sanitized manifest, and carries
a detached Ed25519 signature verified against the pinned release-evidence key.
Both set `certifiable: true` and `source.releaseable: true`; the evidence
authorizes the v0.6.0 daemon prerelease.

## Candidate identity

| Field | Value |
|---|---|
| Milestone | `v0.6.0 — Capability-Gated Join Candidate`, not yet published |
| Baseline commit | `045d85cb1d066f16d564b6051363b9328063ee01` — the published `v0.5.0` tag |
| Network-qualified commit | `55024a46b3e112796ba2acf1dc408dab26dbba2e` |
| Current public `iroh-rooms` pin | `71fbb5007bef4ce83631c94762ec68c2beef3d79` — join-after-conversation deadlock fixed and the join bootstrap gated on the invite capability proof (iroh-room tag v0.1.0-rc.3) |
| Candidate upstream remediation revision | `71fbb5007bef4ce83631c94762ec68c2beef3d79` |
| Retained evidence signatures | present — detached Ed25519 over `direct.json` and `relay.json`, verified against the pinned public SPKI |
| Release evidence gate | READY |
| Evidence window | 2026-07-16 UTC |

Both certifying schema 2 runs resolve the published iroh-rooms revision
`71fbb500` (iroh-room tag `v0.1.0-rc.3`), which carries the reviewed cross-room
event-lookup isolation fix and the compile-time relay-only test seam already
certified for `v0.5.0`, and adds the join-bootstrap capability gate. The direct
run verifies the hardened public-RPC boundary and stable direct paths; the
forced-relay run attests a relay-only source build and proves the same behavior
over relay. Both bind the published network-qualified commit `55024a4`, run over
three peers spanning two BGP origin ASNs (AS11426 + AS24940) with three distinct
observed egresses, pass every recorded assertion, and set `certifiable: true`
and `source.releaseable: true`.

The `v0.5.0`-certified pin remains `d0ceb0b320f1ff3a576b63d8b24aa1bf76a2d3bb`.
That revision is still publicly fetchable by commit SHA, but it is no longer
named by the `v0.1.0-rc.2` tag: the tag was re-created upstream after the
`v0.5.0` release and now resolves to a different commit. `v0.5.0`'s evidence
binds the commit SHA rather than the tag, so its certification and
reproducibility are unaffected; earlier statements in this ledger that equated
tag `v0.1.0-rc.2` with `d0ceb0b` have been corrected to cite the SHA alone.

Older schema 1 runs (retained as `historical-schema1-direct.json` and
`historical-schema1-relay.json`), and the earlier `preview-direct-schema2.json`
recorded against an unpublished pin, remain historical functional evidence only;
they do not qualify the release. The retained `direct.json` and `relay.json`
manifests are the certifying set.

## Milestone evidence ledger

| Gate | Current evidence | Status |
|---|---|---|
| Public read-RPC authorization | centralized guard and local negative suite cover foreign timelines, members, agents, files, pipes, local-file HTTP, and aggregate projections; the current schema 2 direct run denied all 17 room-scoped RPCs and filtered local-file and aggregate reads | certifying PASS at `55024a4…`; the direct run denied all room-scoped RPCs and filtered local-file and aggregate reads over the public network |
| Accepted-room provenance | create/join failure injection proves provenance is accepted before irreversible event publication; 24 concurrent mutations retain every room; direct reads reuse the authorized snapshot cache; Unix mode is pinned to `0600`; exact `atomicwrites 0.4.4` uses synchronized Unix directory replacement and Windows write-through replacement | local PASS; Windows semantics source-reviewed but not behaviorally executed on `windows-latest` |
| Pre-identity protocol contract | `room.list` returns the successful empty onboarding result `{ rooms: [] }` consistently across the core engine, TypeScript mock, Dart daemon, Dart FFI, Dart mock, and golden-corpus oracles | local PASS |
| Upstream synchronization isolation | malicious `WantEvents`, foreign-parent, and administrative-tip tests pass against the published upstream `71fbb5007bef4ce83631c94762ec68c2beef3d79` (iroh-room tag v0.1.0-rc.3), which carries forward the isolation remediation first published at `d0ceb0b320f1ff3a576b63d8b24aa1bf76a2d3bb` | local PASS at the pinned revision; upstream remediation published and Jeliya repinned. NOT network-certified: both certifying manifests set `synchronization_isolation_claimed: false`, so neither run exercises `WantEvents`, foreign-parent, or administrative-tip traversal over the network |
| Android backup exclusion | `allowBackup=false`, explicit cloud/device-transfer exclusion rules, and engine state under `noBackupFilesDir`; repository gate and six secret-storage tests pass | local PASS; this is app-private no-backup storage, not Android Keystore wrapping |
| Agent secret location | platform data directory outside the checkout, deny-all state-directory Git guard, repository ignore rules, and commit-prevention validation | local PASS |
| Rust dependency audit | zero vulnerability advisories; three unmaintained-crate warnings and one yanked-version warning remain in the register below | PASS for vulnerability threshold |
| npm dependency audit | zero vulnerabilities | PASS |
| Complete CI definition | Rust, MSRV, TypeScript, Dart, Flutter, Linux native packaging, docs, smoke, sidecar, agent, fleet, protocol, and dependency gates are configured with required-tool failures; manual dispatch is non-publishing and Gradle is checksum-verified | the six pre-existing required jobs pass on hosted `main` runs; the new `linux-flutter` job has no hosted execution yet |
| Repeatability | two complete hosted CI executions from clean environments | satisfied by `release.yml`, which runs the complete CI gate twice on the network-qualified commit before any release build |
| Direct different-network P2P | schema 2 run `1ca39cfa`: three peers, three distinct observed egresses, two ASNs (AS11426 + AS24940), stable direct paths on roles A/B/C, all assertions pass | certifying PASS; signed and retained as `direct.json` |
| Deliberately forced relay | schema 2 run `cf28bc63`: the relay-only source build compiles against the published seam and self-attests on the operator host and both remote hosts, then proves forced-relay paths on roles A/B/C with all assertions passing | certifying PASS; signed and retained as `relay.json` |
| Join, reconnect, and resynchronization | current direct run covered targeted joins, three-peer convergence, closed-session message, reopen, resynchronization, and settled direct reconnect | certifying PASS; direct and forced-relay both certified |
| Messages, files, and pipes | current direct run covered bidirectional and three-peer messages, byte-identical engine-verified BLAKE3 file fetch, authorized pipe, and zero-target-connection unauthorized pipe | certifying PASS; direct and forced-relay both certified |
| Installer integrity | Unix behavioral tests verify checksum-before-extraction; Windows jobs execute checksum/tamper behavior, simulate reparse rejection, and smoke the native daemon | Unix PASS; hosted `windows-latest` job passes on `main` |
| Complete asset-set visibility | an execution-free read-only job validates and seals the complete set, a separate read-only job performs smoke execution, and the sole writer verifies the receipt without candidate execution before its final token-bearing step; the release stays draft until all uploaded bytes match | executed end to end for `v0.5.0`, which built, verified, and published the five-archive set with sidecars; the same path executes for `v0.6.0` on release dispatch and has not yet run at this candidate. GitHub tag and release operations remain non-transactional |
| Version consistency | local source checks bind daemon/UI/lockfile/changelog naming to `0.6.0` | PASS locally at `55024a4`; the public `v0.6.0` tag does not exist yet, so tag and artifact-name agreement is asserted by `release.yml` on dispatch, not here |
| Documentation | required OKF pages distinguish current schema 2 direct evidence, the failed-closed current relay attempt, and historical schema 1 local-remediation evidence | rerun the local docs gate on the final documentation-only commit |

## Dependency-risk exception register

The current `cargo audit` and `npm audit` results contain no vulnerability
finding. The Rust entries below are maintenance or ecosystem-support warnings,
not approved high/critical vulnerability exceptions. “Reachable” means the
crate is compiled through the listed dependency path; no exploitability claim
is inferred from compilation alone.

| Advisory or warning | Reachable dependency path | Risk assessment | Mitigation | Owner | Review and expiry |
|---|---|---|---|---|---|
| `RUSTSEC-2023-0089` — `atomic-polyfill` unmaintained | `postcard` through `iroh` | future defects may not receive fixes; no vulnerability is identified | retain the exact lockfile, fail CI on vulnerability findings, and monitor `postcard`/`iroh` remediation | Jeliya maintainers | 2026-09-30 |
| `RUSTSEC-2024-0436` — `paste` unmaintained | `netwatch` through `iroh` | maintenance risk in a build-time macro dependency | retain the audit gate and monitor `netwatch`/`iroh` migration | Jeliya maintainers | 2026-09-30 |
| `RUSTSEC-2024-0370` — `proc-macro-error` unmaintained | `genawaiter` through `iroh-blobs` | maintenance risk in a procedural-macro dependency | retain the audit gate and monitor `genawaiter`/`iroh-blobs` remediation | Jeliya maintainers | 2026-09-30 |
| yanked `num-bigint 0.4.7` | `x509-parser` and `rcgen` through `iroh` | ecosystem-support risk; the audit identifies no high/critical vulnerability for this version | keep resolution pinned and adopt the compatible upstream resolution when available | Jeliya maintainers | 2026-09-30 |

At expiry, each entry must be removed, renewed with fresh evidence, or made
release-blocking. A new reachable high or critical vulnerability is a release
blocker unless maintainers explicitly approve a separate, scoped, owned, and
time-bounded exception.

## Evidence schema 2 source-build contract

A qualifying direct or relay run must emit evidence schema 2. The source build
starts from an isolated `git clone --bare --no-local`, archives the exact
recorded commit, and does not consume checkout-local Git configuration or
attributes. Build subprocesses receive run-owned `HOME`, Cargo, npm, Git, and
temporary state plus a controlled path and a documented ambient allowlist;
ambient build controls are rejected and unlisted ambient variables are not
forwarded.

The x86_64 macOS operator must supply the official Zig `0.15.2` x86_64-macos
archive and the independently established SHA-256
`375b6909fc1495d16fc2c7db9538f707456bfc3373b14ee83fdd3e22b3d43f7f`.
The harness verifies the copied archive before extraction, validates its member
layout, and binds both the executed Zig binary and its library directory to the
verified run-owned installation root.

The harness executes selected tools through resolved absolute paths. Schema 2
records their filenames, versions, and observed SHA-256 digests for Rust,
Cargo, rustup, Node, npm, Zig, `cargo-zigbuild`, Git, and tar without retaining
operator-local filesystem paths.
Only the complete Zig installation archive is independently verified. The
other observed tool digests establish execution identity within the run; they
are not independent supply-chain attestations. npm executes under the exact
recorded Node binary. `cargo-zigbuild` executes directly with the recorded
Cargo and Zig paths, while Python `ziglang` discovery is disabled. Direct and
relay manifests must record an identical toolchain.

This contract does not repair stale provenance. A schema 2 record is still
non-certifying unless it binds public immutable Jeliya and Iroh Rooms
revisions, a clean source tree, qualifying topology, successful cleanup, and a
valid detached signature from the pre-authorized evidence key.

## Local upstream remediation qualification

The reviewed Iroh Rooms remediation — the room-scoped event-lookup fix and the
compile-time relay-only test seam — was first published as
`d0ceb0b320f1ff3a576b63d8b24aa1bf76a2d3bb` and certified for `v0.5.0`; it is
carried forward unchanged in this candidate's pin
`71fbb5007bef4ce83631c94762ec68c2beef3d79` (iroh-room tag `v0.1.0-rc.3`). Its
tests demonstrate that:

- `WantEvents` cannot serve a known event ID from another room;
- a foreign row remains unavailable when cited as a local causal parent;
- administrative-tip traversal remains room-scoped;
- a normal Jeliya build rejects the hidden relay attestation check; and
- the relay-only feature is compile-time, propagated through Jeliya and the
  dependency, and attested before a forced-relay run starts.

This remediation is now pinned through Jeliya's public `Cargo.toml` and
`Cargo.lock` at the network-qualified commit, and the network suite was repeated
against it — see the certifying evidence below.

## Certifying network evidence

Both certifying runs used the published network-qualified Jeliya commit
`55024a46b3e112796ba2acf1dc408dab26dbba2e` and the exact public Iroh Rooms pin
`71fbb5007bef4ce83631c94762ec68c2beef3d79`. The operator role ran on macOS
x86_64. Roles B and C ran on `root@demo1` and `root@demo2`, both Ubuntu x86_64.
SSH connected as root, but both remote daemons executed through `setpriv` as
UID/GID `65534`.

| Path | Run and UTC window | Result | Manifest | Signature |
|---|---|---|---|---|
| direct | `20260716T201318Z-1ca39cfa`, 20:13:18–20:34:25 | certifiable; 36/36 assertions; A/B/C each remained direct for three consecutive observations | [`direct.json`](evidence/v0.6.0/direct.json) | [`direct.json.sig`](evidence/v0.6.0/direct.json.sig) |
| forced relay | `20260716T203450Z-cf28bc63`, 20:34:50–20:56:31 | certifiable; 36/36 assertions; the relay-only source build self-attested on the operator host and both remote hosts, then A/B/C each remained relay for three consecutive observations | [`relay.json`](evidence/v0.6.0/relay.json) | [`relay.json.sig`](evidence/v0.6.0/relay.json.sig) |

Each run observed three pairwise-distinct public egress values across two BGP
origin ASNs (`AS11426` for the operator, `AS24940` for both remote roles)
without retaining any address, and exercised targeted room join, three-peer
convergence, messages, file fetch and BLAKE3 verification, authorized and
unauthorized pipes, closed-session/reopen/resynchronization/reconnect, all
room-scoped read denials, local-file denial, and aggregate room/agent filtering.
Cleanup passed, and independent read-only checks found no run directory or
process remaining on either remote host.

Both manifests are evidence schema 2 and record the isolated source build,
complete Zig archive verification, exact tool identities, binary hashes,
assertions, digest-only log summaries, and cleanup. Each binds the published
Jeliya commit and the published, remediated Iroh Rooms revision, sets
`certifiable: true` and `source.releaseable: true`, and carries a detached
Ed25519 signature (`.sig`) that verifies against the pinned release-evidence
public SPKI. The forced-relay run is the reviewed safety boundary exercised end
to end: the harness required the compile-time seam, the binary attested itself
as a relay-only build, and the path assertions held over relay.

## Historical schema 1 local-remediation evidence

For the historical runs, the operator role ran on macOS x86_64. Roles B and C
ran on `root@demo1` and `root@demo2`, both Ubuntu 22.04.5 x86_64. SSH connected
as root, but the remote daemons executed through `setpriv` as UID/GID `65534`.
No host firewall, route, account, package repository, SSH configuration, or
persistent service was changed.

The three observed public egress values were pairwise different and were not
persisted. The sanitized ASN result was `AS11426` for the operator and
`AS24940` for both remote roles, satisfying the harness's two-origin-ASN
topology condition. `user@kilo` was rejected during inventory because it shared
the operator's observed egress and therefore could not satisfy the three-role
topology gate.

| Path | Run and UTC window | Historical result | Retained evidence | Manifest SHA-256 |
|---|---|---|---|---|
| direct | `20260712T155534Z-d3d9ff69`, 15:55:34–16:15:24 | 36/36; A/B/C each remained direct for three consecutive observations | [`historical-schema1-direct.json`](evidence/v0.5.0/historical-schema1-direct.json) | `5b4659cc709148e149ce339c8b70515ddd838b4cc7cf07a96a5982b08a1b2af0` |
| forced relay | `20260712T161837Z-f1d9c149`, 16:18:37–16:38:54 | 36/36; A/B/C each remained relay for three consecutive observations | [`historical-schema1-relay.json`](evidence/v0.5.0/historical-schema1-relay.json) | `472f71394485e72e1e3c9f791d1d80e1489bdcbd19ec22d15326044efb5049e9` |

Both runs used Jeliya
`fe870c7c5b63f2bf52b031dd1bc8e27e83183be5`, Iroh Rooms
`3702e8cbcd5ac1808791124dd6bc44068be5f822`, Node `22.22.3`, Rust/Cargo
`1.91.0`, Zig `0.15.2`, `cargo-zigbuild 0.23.0`, locked source builds, and
freshly built embedded web UI. The native macOS x86_64 and Linux x86_64 musl
binaries were built from a Git archive of the recorded source with two Cargo
jobs. Each transferred Linux binary was hash- and version-checked on both
remote hosts before execution.

These retained `historical-schema1-{direct,relay}.json` manifests use evidence
schema 1 and the unpublished local upstream remediation rather than a public
dependency pin, and predate the isolated source-build environment and
Zig-installation binding defined by schema 2. They were non-certifying and have
now been superseded by the certifying schema 2 direct and forced-relay runs
recorded above, which emit against the public commit and the immutable public
pin.

Each run covered targeted room join; three-peer membership and message
convergence; messages in both directions; file listing, fetch, engine BLAKE3
verification, and byte equality; authorized and unauthorized pipes; closed
session, offline message, reopen, resynchronization, and reconnect; all 17
room-scoped RPC denials; local-file denial; and foreign room/agent filtering
from aggregates. The foreign fixture join required two attempts in each
successful run because the first 15-second bootstrap window ended with the
transient `peer_unreachable` condition. The bounded retry did not weaken
authorization or retry credential failures.

Cleanup passed in both runs: all run-owned processes stopped and all exact
run-owned temporary directories were removed. The retained manifests omit log
excerpts and raw logs; they keep only per-role stream line counts, byte counts,
and SHA-256 digests. They contain no invite tickets, bearer tokens, portfile
tokens, identity seeds, private keys, or public IP addresses.

The run-era collector finalized those stream summaries after process exit but
did not separately prove stdout/stderr closure. The functional assertions do
not depend on the summaries, but their tail completeness is therefore not a
release claim. The hardened harness now waits for both streams with a bounded,
fail-closed timeout before digest finalization; a fresh certifying run must use
that implementation.

### Historical qualification limits

The historical direct result demonstrates cross-network direct connectivity
for that older source/dependency pair. The historical forced-relay result
demonstrates that the same workflows operate when a compile-time diagnostic
build disables direct transport. It does not show that ordinary direct-capable
binaries naturally failed hole punching.

The historical schema 1 runs were never signed and bound unpublished Jeliya and
Iroh Rooms commits, so they could not qualify a release. The release-evidence
Ed25519 key custody and the public commit/pin that they lacked are now
established for the certifying schema 2 set above.

## Failed runs retained as investigation history

| Run suffix | Outcome | Cleanup |
|---|---|---|
| `46658a13` | topology rejected before functional assertions because the proposed remote shared the operator egress; 0 assertions | passed |
| `262fe069` | 33 assertions passed, then the isolated foreign-room join exhausted its single bootstrap attempt | passed |
| `a75f8796` | 32 assertions passed, then the same transient join bootstrap condition recurred | passed |

These attempts do not count toward the milestone. They explain the host change
and the bounded retry used by the successful runs; they were not converted
into optimistic pass records.

## Historical evidence

The 2026-07-04 Gate A result used evidence-record commit
`f2aea0959ee5bf0f91fee030bdd2e2466163671c` and Iroh Rooms revision
`1d2f014e783893ffeaea055c436370179a31110a`. It proves behavior only for those
older revisions and remains historical. See
[`gate-a-result.md`](gate-a-result.md).

The Android 13 smoke proves local engine lifecycle, room operations, pushes,
persistence, and UI integration with `loopback: false`. It did not communicate
with a remote peer or measure a direct/relay path, so it is not Android
real-network evidence.

## Exact release blockers

The evidence gate is **READY** for `v0.6.0`. Each prior blocker has been
cleared:

1. the upstream remediation is reviewed and published as iroh-rooms
   `71fbb5007bef4ce83631c94762ec68c2beef3d79` (iroh-room tag `v0.1.0-rc.3`);
2. Jeliya's public `Cargo.toml` and `Cargo.lock` are repinned to that immutable
   public revision, and the exact candidate commit
   `55024a46b3e112796ba2acf1dc408dab26dbba2e` is published on `main`;
3. out-of-band Ed25519 private-key custody is established and only the canonical
   public SPKI is committed;
4. the direct and forced-relay runs were repeated from that public commit and
   dependency, retained as the exact sanitized `direct.json`/`relay.json`
   manifests, and carry valid detached signatures; and
5. the remaining gates — two clean hosted CI runs, the Windows installer
   integrity gate, the five-archive daemon artifact set, version/tag/name
   consistency, and the draft-until-verified publication — are executed by
   `release.yml` on release dispatch under explicit release authority.

`v0.5.0` published through that same path on 2026-07-14 with the complete
five-archive set and sidecars. `v0.6.0` has not been dispatched yet; the gates
in item 5 remain unexecuted at this candidate.

## Candidate provenance: repin to iroh-room v0.1.0-rc.3 (2026-07-16)

After the `v0.5.0` release, `main` repinned `iroh-rooms` from certified
`d0ceb0b…` to the published `v0.1.0-rc.3` tag
(`71fbb5007bef4ce83631c94762ec68c2beef3d79`). On top of the certified pin's
isolation fix and relay seam, rc.3 carries the join-after-conversation
deadlock fix (upstream PR #111 — at `d0ceb0b`, and therefore in released
`v0.5.0`, an invite minted after any non-admin chat cannot complete
`room.join`), the join-bootstrap capability gate (PRs #117/#120, with
`room.join` now presenting the invite's `BootstrapProof`), size-independent
membership reconciliation (PR #118), and deep pure-chat gap healing
(PR #116).

Local re-qualification at the pinned tag on 2026-07-16, which preceded and
motivated the certifying network runs recorded above (Linux aarch64
workstation, clean checkouts of the exact tag commit):

- upstream `iroh-rooms-core --all-features`: 523/523 pass, including the
  named isolation regressions
  (`room_scoped_point_reads_never_match_a_foreign_room`,
  `want_events_cannot_serve_an_id_from_another_room_in_the_shared_store`) and
  the v1 wire/store compatibility fixtures;
- upstream `iroh-rooms-net`: 232/232 pass, including the capability-proof
  join end-to-end suite;
- Jeliya at the repin: 63/63 `jeliya-core` and 8/8 `jeliyad` tests pass, and
  the two-daemon loopback end-to-end suite passes 67/67, covering the
  proof-presenting `room.join`;
- the relay verifier builds against the pinned seam, prints the compile-time
  attestation `jeliya-relay-only-test-build-v1`, and exits before touching a
  data dir; the default build rejects the hidden flag.

The certified `v0.5.0` evidence binds `c5f740e` + `d0ceb0b` and does not
transfer to this pin. The fresh signed direct and forced-relay runs it required
were executed on 2026-07-16 from the public commit `55024a4` carrying the rc.3
pin, and are the certifying set recorded above; the local qualification in this
section is corroborating evidence, not the release qualification. Upstream
residuals at rc.3 are recorded in the
[threat model](security-threat-model.md): live event fan-out to a
still-connected unproven provisional dialer while joins are being accepted
(issue #121) and store holes from swallowed insert errors (issue #119).
Mixed-fleet caution: a `v0.5.0`-era joiner cannot bootstrap from an rc.3
admin (it sends no capability proof), and an rc.3 joiner hard-stalls
bootstrapping from a `v0.5.0`-era responder once it holds more than ~1k
events — a room's members, especially its admin, must upgrade together.
