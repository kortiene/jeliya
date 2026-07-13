---
type: "Status Report"
title: "Verification evidence"
description: "Revision-bound verification ledger and evidence-recording contract for the v0.5.0 technical preview."
tags: ["evidence", "networking", "release", "testing", "verification"]
timestamp: "2026-07-12T23:55:23Z"
status: "canonical"
implementation_status: "implemented"
verification_status: "partial"
release_status: "unreleased"
audience: ["contributors", "maintainers", "operators", "release-engineers"]
---

# Verification evidence

This ledger separates a functional test result from release-qualifying
evidence. A result is transferable to a release candidate only when it binds
the exact public Jeliya commit, public immutable dependency revisions,
environment, timestamps, assertions, retained sanitized manifest, and detached
signature. The current schema 2 direct run is valuable functional evidence.
The matching forced-relay build failed closed before remote execution because
the public dependency pin does not contain the reviewed relay-only test seam.
Neither outcome authorizes publication.

## Candidate identity

| Field | Value |
|---|---|
| Milestone | `v0.5.0 — Evidence-Backed Technical Preview` |
| Baseline commit | `1285b42037a3713840955fa518f2b81b19f2929f` |
| Hardened and network-tested commit before final documentation reconciliation | `0f6769a68d783cf6a5feba0e2db6111a212affa1` — clean, local, and unpublished |
| Current public `iroh-rooms` pin | `3cb9bfd1e43eb755c967315c37b6d4fd1c2bf020` — synchronization isolation is not remediated |
| Candidate upstream remediation revision | `3702e8cbcd5ac1808791124dd6bc44068be5f822` — clean and tested locally, but unpublished |
| Retained evidence signatures | absent; the release-evidence public SPKI is not provisioned |
| Release evidence gate | BLOCKED |
| Evidence window | 2026-07-12 UTC |

The current schema 2 direct run resolves the repository's public, unsafe Iroh
Rooms pin. It verifies the hardened public-RPC boundary and direct network
behavior, but explicitly makes no upstream synchronization-isolation claim.
The exact Jeliya commit is unpublished, the evidence is unsigned, and the pin
still permits the synchronization behavior under remediation, so the retained
manifest correctly sets `certifiable: false` and `source.releaseable: false`.

Older schema 1 runs use a local `file://` checkout of the unpublished upstream
remediation. They remain useful historical functional evidence only; they do
not qualify the current implementation or a release.

## Milestone evidence ledger

| Gate | Current evidence | Status |
|---|---|---|
| Public read-RPC authorization | centralized guard and local negative suite cover foreign timelines, members, agents, files, pipes, local-file HTTP, and aggregate projections; the current schema 2 direct run denied all 17 room-scoped RPCs and filtered local-file and aggregate reads | functional PASS at `0f6769a…`; no upstream synchronization-isolation claim; release qualification blocked |
| Accepted-room provenance | create/join failure injection proves provenance is accepted before irreversible event publication; 24 concurrent mutations retain every room; direct reads reuse the authorized snapshot cache; Unix mode is pinned to `0600`; exact `atomicwrites 0.4.4` uses synchronized Unix directory replacement and Windows write-through replacement | local PASS; Windows semantics source-reviewed but not behaviorally executed on `windows-latest` |
| Pre-identity protocol contract | `room.list` returns the successful empty onboarding result `{ rooms: [] }` consistently across the core engine, TypeScript mock, Dart daemon, Dart FFI, Dart mock, and golden-corpus oracles | local PASS |
| Upstream synchronization isolation | malicious `WantEvents`, foreign-parent, and administrative-tip tests passed against local upstream `3702e8cbcd5ac1808791124dd6bc44068be5f822` | local PASS; BLOCKED until upstream publication and Jeliya repin |
| Android backup exclusion | `allowBackup=false`, explicit cloud/device-transfer exclusion rules, and engine state under `noBackupFilesDir`; repository gate and six secret-storage tests pass | local PASS; this is app-private no-backup storage, not Android Keystore wrapping |
| Agent secret location | platform data directory outside the checkout, deny-all state-directory Git guard, repository ignore rules, and commit-prevention validation | local PASS |
| Rust dependency audit | zero vulnerability advisories; three unmaintained-crate warnings and one yanked-version warning remain in the register below | PASS for vulnerability threshold |
| npm dependency audit | zero vulnerabilities | PASS |
| Complete CI definition | Rust, MSRV, TypeScript, Dart, Flutter, docs, smoke, sidecar, agent, fleet, protocol, and dependency gates are configured with required-tool failures; manual dispatch is non-publishing and Gradle is checksum-verified | configured; no two hosted clean runs exist |
| Repeatability | two complete hosted CI executions from clean environments | BLOCKED; repository was not pushed and hosted execution was not authorized |
| Direct different-network P2P | schema 2 run `3c938c66`: three peers, three distinct observed egress values, two ASNs, stable direct paths on roles A/B/C, and 36/36 assertions | current functional PASS; retained, unsigned, and non-certifying |
| Deliberately forced relay | the exact public-pin relay-only source build requested the diagnostic seam, but the pinned dependency does not provide it | BLOCKED; build failed closed before remote execution; no current relay PASS exists |
| Join, reconnect, and resynchronization | current direct run covered targeted joins, three-peer convergence, closed-session message, reopen, resynchronization, and settled direct reconnect | current functional PASS for direct only; relay remains blocked |
| Messages, files, and pipes | current direct run covered bidirectional and three-peer messages, byte-identical engine-verified BLAKE3 file fetch, authorized pipe, and zero-target-connection unauthorized pipe | current functional PASS for direct only; relay remains blocked |
| Installer integrity | Unix behavioral tests verify checksum-before-extraction; Windows jobs execute checksum/tamper behavior, simulate reparse rejection, and smoke the native daemon | Unix PASS; Windows gates configured but no hosted `windows-latest` result exists |
| Complete asset-set visibility | an execution-free read-only job validates and seals the complete set, a separate read-only job performs smoke execution, and the sole writer verifies the receipt without candidate execution before its final token-bearing step; the release stays draft until all uploaded bytes match | contract and negative receipt tests PASS; no five-archive set built and no publication executed; GitHub tag and release operations are not one transaction |
| Version consistency | local source checks bind daemon/UI/lockfile/changelog naming to `0.5.0` | local PASS; public tag and artifacts do not exist |
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

The unpublished Iroh Rooms revision
`3702e8cbcd5ac1808791124dd6bc44068be5f822` contains the room-scoped event
lookup remediation and the compile-time relay-only test seam. Local tests
demonstrated that:

- `WantEvents` cannot serve a known event ID from another room;
- a foreign row remains unavailable when cited as a local causal parent;
- administrative-tip traversal remains room-scoped;
- a normal Jeliya build rejects the hidden relay attestation check; and
- the relay-only feature is compile-time, propagated through Jeliya and the
  dependency, and attested before a forced-relay run starts.

This qualification is not release evidence. The revision must first be
reviewed and published, then pinned through Jeliya's public `Cargo.toml` and
`Cargo.lock` before the network suite is repeated.

## Current schema 2 network evidence

The current direct run used the clean hardened Jeliya source snapshot
`0f6769a68d783cf6a5feba0e2db6111a212affa1` and the exact public Iroh Rooms pin
`3cb9bfd1e43eb755c967315c37b6d4fd1c2bf020`. The operator role ran on macOS
x86_64. Roles B and C ran on `root@demo1` and `root@demo2`, both Ubuntu 22.04.5
x86_64. SSH connected as root, but both remote daemons executed through
`setpriv` as UID/GID `65534`.

| Path | Run and UTC window | Current result | Retained evidence | Manifest SHA-256 |
|---|---|---|---|---|
| direct | `20260712T231015Z-3c938c66`, 23:10:15–23:34:46 | 36/36; A/B/C each remained direct for three consecutive observations | [`preview-direct-schema2.json`](evidence/v0.5.0/preview-direct-schema2.json) | `2ef571f34b2140f033487e019a2746c20ce3265362881fb843eee708e057a6a5` |
| forced relay | no run manifest | BLOCKED; exact public-pin relay-only build failed closed because the dependency does not provide the reviewed compile-time seam; no remote execution occurred | none | not applicable |

The direct run observed three pairwise-distinct public egress values across two
ASNs without retaining any address. It exercised targeted room join,
three-peer convergence, messages, file fetch and hash verification, pipes,
reconnect and resynchronization, all 17 room-scoped read denials, local-file
denial, and aggregate room/agent filtering. Cleanup passed, and independent
read-only checks found no run directory or process remaining on either remote
host.

The manifest is evidence schema 2 and records the isolated source build,
complete Zig archive verification, exact tool identities, binary hashes,
assertions, digest-only log summaries, and cleanup. It is intentionally
unsigned and `certifiable: false`: the Jeliya commit is unpublished, the
evidence public key is absent, and the public dependency pin still lacks the
synchronization-isolation remediation. Its
`synchronization_isolation_claimed: false` field is normative for interpreting
the result. Public-RPC non-disclosure does not prove that foreign-room events
cannot enter the local store through upstream synchronization.

The failed relay build is also evidence of a working safety boundary: the
harness did not silently omit the required seam, substitute a different
dependency, weaken the path assertion, or mutate a remote host. It is not a
relay fallback result and must remain BLOCKED until the reviewed upstream seam
is published and pinned.

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
| direct | `20260712T155534Z-d3d9ff69`, 15:55:34–16:15:24 | 36/36; A/B/C each remained direct for three consecutive observations | [`direct.json`](evidence/v0.5.0/direct.json) | `5b4659cc709148e149ce339c8b70515ddd838b4cc7cf07a96a5982b08a1b2af0` |
| forced relay | `20260712T161837Z-f1d9c149`, 16:18:37–16:38:54 | 36/36; A/B/C each remained relay for three consecutive observations | [`relay.json`](evidence/v0.5.0/relay.json) | `472f71394485e72e1e3c9f791d1d80e1489bdcbd19ec22d15326044efb5049e9` |

Both runs used Jeliya
`fe870c7c5b63f2bf52b031dd1bc8e27e83183be5`, Iroh Rooms
`3702e8cbcd5ac1808791124dd6bc44068be5f822`, Node `22.22.3`, Rust/Cargo
`1.91.0`, Zig `0.15.2`, `cargo-zigbuild 0.23.0`, locked source builds, and
freshly built embedded web UI. The native macOS x86_64 and Linux x86_64 musl
binaries were built from a Git archive of the recorded source with two Cargo
jobs. Each transferred Linux binary was hash- and version-checked on both
remote hosts before execution.

These retained manifests use evidence schema 1 and the unpublished local
upstream remediation rather than the current public dependency pin. Schema 1
predates the isolated source-build environment and complete Zig-installation
binding defined by schema 2. The historical records therefore remain
non-certifying and cannot be promoted by adding a signature or by revalidating
them with the newer checker; new direct and relay runs must emit schema 2 after
the upstream fix is public and immutably pinned.

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

Both historical manifests remain unsigned because the approved
release-evidence Ed25519 public SPKI and its out-of-band private-key custody
have not been established.
The release gate intentionally rejects unsigned evidence. Adding a signature
now would still not qualify these runs because their Jeliya and Iroh Rooms
commits are unpublished.

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

The evidence gate remains **BLOCKED** until all of the following are complete:

1. review and publish upstream remediation
   `3702e8cbcd5ac1808791124dd6bc44068be5f822`, or an equivalent reviewed
   successor;
2. repin Jeliya's public `Cargo.toml` and `Cargo.lock` to that immutable public
   revision and publish the exact Jeliya candidate commit;
3. establish out-of-band Ed25519 private-key custody and commit only the
   canonical public SPKI before the network-qualified commit;
4. repeat direct and forced-relay runs from that public commit and dependency,
   retain the exact sanitized manifests, and attach valid detached signatures;
5. pass every required hosted CI gate twice from clean environments;
6. behaviorally execute the Windows installer integrity gate, build and verify
   the complete five-archive daemon artifact set, and recheck version/tag/name
   consistency; and
7. invoke the publication workflow only after explicit release authority is
   granted, keep the release draft until the complete asset set verifies, and
   use the documented recovery procedure if the non-transactional Git tag and
   release operations are interrupted.

Until then, `v0.5.0` is an unreleased technical-preview candidate, regardless
of the local functional successes recorded above.
