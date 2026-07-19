---
type: "Status Report"
title: "Verification evidence"
description: "Revision-bound verification ledger and evidence-recording contract for the v0.6.0 candidate."
tags: ["evidence", "networking", "release", "testing", "verification"]
timestamp: "2026-07-19T19:05:30Z"
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
signature. The retained direct and forced-relay schema 2 runs meet that bar for
the earlier Jeliya `55024a4...` + Iroh Rooms `71fbb500...` snapshot. The current
source candidate repins Iroh Rooms to `a5d98b70...`; those signed manifests do
not transfer to the new dependency revision and do not authorize a `v0.6.0`
release from the current tree.

## Candidate identity

| Field | Value |
|---|---|
| Milestone | `v0.6.0 — Capability-Gated Join Candidate`, not yet published |
| Baseline commit | `045d85cb1d066f16d564b6051363b9328063ee01` — the published `v0.5.0` tag |
| Current source candidate | `105744b6c27633e5ccc576d86f1a15e3fe443b94` |
| Network-qualified commit | `pending — fresh signed direct and relay runs required` |
| Current public `iroh-rooms` pin | `a5d98b70d717f35d3ce60953a88e12e646f2e871` — deliberately untagged first upstream `main` merge carrying the fixes for `kortiene/iroh-room#121` and `kortiene/iroh-room#119` plus the connection-generation follow-ups |
| Candidate upstream remediation revision | `a5d98b70d717f35d3ce60953a88e12e646f2e871` |
| Last network-qualified snapshot | Jeliya `55024a46b3e112796ba2acf1dc408dab26dbba2e` + Iroh Rooms `71fbb5007bef4ce83631c94762ec68c2beef3d79` (tag `v0.1.0-rc.3`) |
| Retained evidence signatures | present and valid for the last network-qualified snapshot; not transferable to the current pin |
| Release evidence gate | BLOCKED |
| Evidence window | local exact-revision qualification on 2026-07-19 UTC; current network evidence pending |

The current pin is the first upstream `main` merge containing both required
fixes. The two commits after it change only `iroh-rooms-cli`, which Jeliya does
not consume, so pinning later would expand the reviewed surface without changing
the SDK behavior. The newest tag remains `v0.1.0-rc.3` at `71fbb500...` and
predates both fixes.

The retained signed direct and forced-relay runs remain valid evidence for
`55024a4...` + `71fbb500...`: they covered three peers across two BGP origin
ASNs, passed every recorded assertion, and set `certifiable: true`. They are
historical for the current source candidate. Fresh manifests must bind
`105744b...` and `a5d98b70...` before the release
evidence gate can return to `READY`.

The current source candidate was recorded as `4261470...` while the repin was in
review. `main` enforces linear history, so the merge rebased that work and
rewrote its commit SHA to `9c71fac...`. The rebased commit has the identical
tree (`53e5ce2c...`) and the identical parent (`a24f2238...`), so it is the same
source in the same position in history and the recorded local qualification
results carry over unchanged. The pre-merge SHA is not reachable from `main` and
must not be used in reproduction steps. This restatement does not alter the
`Network-qualified commit` row, which stays `pending` until fresh signed direct
and forced-relay runs are performed.

Jeliya `105744b6c27633e5ccc576d86f1a15e3fe443b94` advances the source
candidate to include the bounded concurrent path-settlement observer and
sanitized timeout diagnostics from `kortiene/jeliya#133`. Its tree
(`4aeed8ce...`) and parent (`05b5f4e...`) match the reviewed PR head exactly.
All eight hosted checks passed on public `main` run `29699530741` at the exact
candidate commit, including the network evidence harness qualification step.
The evidence gate remains `BLOCKED`: neither the prior signed manifests nor
the local dry runs can qualify this new public commit.

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
| Public read-RPC authorization | centralized guard and local negative suite cover foreign timelines, members, agents, files, pipes, local-file HTTP, and aggregate projections; the retained schema 2 runs denied all 17 room-scoped RPCs and filtered local-file and aggregate reads | certifying PASS at the superseded `55024a4…` + `71fbb500…` snapshot; current-pin network rerun required |
| Accepted-room provenance | create/join failure injection proves provenance is accepted before irreversible event publication; 24 concurrent mutations retain every room; direct reads reuse the authorized snapshot cache; Unix mode is pinned to `0600`; exact `atomicwrites 0.4.4` uses synchronized Unix directory replacement and Windows write-through replacement | local PASS; Windows semantics source-reviewed but not behaviorally executed on `windows-latest` |
| Pre-identity protocol contract | `room.list` returns the successful empty onboarding result `{ rooms: [] }` consistently across the core engine, TypeScript mock, Dart daemon, Dart FFI, Dart mock, and golden-corpus oracles | local PASS |
| Upstream synchronization isolation | malicious `WantEvents`, foreign-parent, and administrative-tip checks pass at `a5d98b70d717f35d3ce60953a88e12e646f2e871`, which carries forward the isolation remediation first published at `d0ceb0b320f1ff3a576b63d8b24aa1bf76a2d3bb` | local exact-revision PASS. NOT network-certified: the retained manifests set `synchronization_isolation_claimed: false` and do not exercise these internals |
| Unproven provisional-peer fanout and teardown | live-fanout denial plus the superseded-link and deauthorization-state generation regressions pass at `a5d98b70...`; Jeliya's 67-assertion loopback suite covers capability-proven join integration | local exact-revision PASS; current-pin direct and relay evidence pending |
| Store insert recovery and degradation | five deterministic upstream tests cover retry recovery, local hole healing under descendants, exhausted-budget durable critical `store_degraded`, queue overflow, and exactly-once peer re-serve | local exact-revision PASS at `a5d98b70...`; disk failure remains possible and must surface operationally |
| Android backup exclusion | `allowBackup=false`, explicit cloud/device-transfer exclusion rules, and engine state under `noBackupFilesDir`; repository gate and six secret-storage tests pass | local PASS; this is app-private no-backup storage, not Android Keystore wrapping |
| Agent secret location | platform data directory outside the checkout, deny-all state-directory Git guard, repository ignore rules, and commit-prevention validation | local PASS |
| Rust dependency audit | zero vulnerability advisories; three unmaintained-crate warnings and one yanked-version warning remain in the register below | PASS for vulnerability threshold |
| npm dependency audit | zero vulnerabilities | PASS |
| Complete CI definition | Rust, MSRV, TypeScript, Dart, Flutter, Linux native packaging, docs, smoke, sidecar, agent, fleet, protocol, and dependency gates are configured with required-tool failures; manual dispatch is non-publishing and Gradle is checksum-verified | all jobs, including `linux-flutter` and Windows installer integrity, passed on public `main` run `29688515781` at `a24f223...`; current-candidate rerun pending |
| Repeatability | two complete hosted CI executions from clean environments | configured in `release.yml`; not executed for the current candidate |
| Direct different-network P2P | retained schema 2 run `1ca39cfa`: three peers, three distinct observed egresses, two ASNs (AS11426 + AS24940), stable direct paths on roles A/B/C, all assertions pass | certifying PASS for `55024a4…` + `71fbb500…`; current-pin rerun required |
| Deliberately forced relay | retained schema 2 run `cf28bc63`: the relay-only source build self-attests on all three hosts and proves relay paths on roles A/B/C | certifying PASS for `55024a4…` + `71fbb500…`; current-pin rerun required |
| Join, reconnect, and resynchronization | the current two-daemon loopback run passes 67/67 assertions; retained network runs cover the same integration boundary at the prior dependency pin | local current-pin PASS; current-pin direct and relay evidence pending |
| Messages, files, and pipes | current loopback covers messages, byte-identical BLAKE3-verified file fetch, authorized pipe, and unauthorized denial; retained network runs cover the prior dependency pin | local current-pin PASS; current-pin direct and relay evidence pending |
| Installer integrity | Unix behavioral tests verify checksum-before-extraction; Windows jobs execute checksum/tamper behavior, simulate reparse rejection, and smoke the native daemon | Unix PASS; hosted `windows-latest` job passes on `main` |
| Complete asset-set visibility | an execution-free read-only job validates and seals the complete set, a separate read-only job performs smoke execution, and the sole writer verifies the receipt without candidate execution before its final token-bearing step; the release stays draft until all uploaded bytes match | executed end to end for `v0.5.0`, which built, verified, and published the five-archive set with sidecars; the same path executes for `v0.6.0` on release dispatch and has not yet run at this candidate. GitHub tag and release operations remain non-transactional |
| Version consistency | local source checks bind daemon/UI/lockfile/changelog naming to `0.6.0` | PASS locally at `105744b`; the public `v0.6.0` tag does not exist yet |
| Documentation | required OKF pages distinguish the current locally qualified candidate, the prior signed schema 2 snapshot, and historical schema 1 local-remediation evidence | local docs and release-contract gates pass on this documentation diff |

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

## Upstream isolation-remediation lineage

The reviewed Iroh Rooms room-scoped event-lookup remediation and compile-time
relay-only test seam were first published as
`d0ceb0b320f1ff3a576b63d8b24aa1bf76a2d3bb` and certified for `v0.5.0`. They
were carried through the prior `71fbb500...` snapshot and remain in the current
pin `a5d98b70d717f35d3ce60953a88e12e646f2e871`. Exact-revision tests demonstrate
that:

- `WantEvents` cannot serve a known event ID from another room;
- a foreign row remains unavailable when cited as a local causal parent;
- administrative-tip traversal remains room-scoped;
- a normal Jeliya build rejects the hidden relay attestation check; and
- the relay-only feature is compile-time, propagated through Jeliya and the
  dependency, and attested before a forced-relay run starts.

The current `Cargo.toml` and `Cargo.lock` resolve `a5d98b70...`, and the local
isolation regression was repeated there. The certifying network evidence below
binds the prior `55024a4...` + `71fbb500...` snapshot; it has not been repeated
at the current pin.

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

The evidence gate is **BLOCKED** for the current `v0.6.0` source candidate.
Completed work:

1. upstream fixes for `kortiene/iroh-room#121`, `kortiene/iroh-room#126`, and
   `kortiene/iroh-room#119` are public
   and present at immutable revision
   `a5d98b70d717f35d3ce60953a88e12e646f2e871`;
2. public Jeliya source candidate
   `105744b6c27633e5ccc576d86f1a15e3fe443b94` resolves that exact revision in
   `Cargo.toml` and `Cargo.lock`; and
3. the targeted fanout, isolation, and store-degradation regressions, the full
   upstream core/net suite, the Jeliya workspace suite, and the loopback E2E
   suite pass at source candidate
   `105744b6c27633e5ccc576d86f1a15e3fe443b94`.

Remaining work before `READY`:

1. run direct and forced-relay qualification from that clean public commit,
   retaining new sanitized manifests bound to `a5d98b70...`;
2. sign the exact manifest bytes with the approved out-of-band Ed25519 key;
3. pass the evidence signature, source ancestry, and docs-only-after-
   qualification checks; and
4. complete the hosted double CI run and remaining release gates under explicit
   release authority.

`v0.5.0` remains released and certified at its own revision pair. The retained
`v0.6.0` manifests remain valid for the older `55024a4...` + `71fbb500...`
snapshot but cannot clear the current candidate's gate.

## Candidate provenance: untagged upstream fixes (2026-07-19)

Jeliya source candidate `105744b6c27633e5ccc576d86f1a15e3fe443b94`
repins the SDK crates to upstream merge
`a5d98b70d717f35d3ce60953a88e12e646f2e871`. This is the first `main` commit
that contains:

- the provisional-peer fanout and deferred-handshake fix from
  `kortiene/iroh-room#121` (merge `58aca4ba...`);
- the connection-generation guard and deauthorization follow-up for
  `kortiene/iroh-room#126`; and
- store-insert retry, local hole healing, and durable fail-loud degradation for
  `kortiene/iroh-room#119` (merge `a5d98b70...`).

The newest tag, `v0.1.0-rc.3` at `71fbb500...`, predates all three. The two
commits after `a5d98b70...` modify only `iroh-rooms-cli/src/audit.rs`, a crate
Jeliya does not consume. The untagged minimum therefore carries the required
library fixes without adding unrelated code to the reviewed dependency surface.

Local qualification used a clean detached upstream checkout on Linux aarch64
with Rust/Cargo 1.97.1:

```sh
# Upstream checkout at a5d98b70d717f35d3ce60953a88e12e646f2e871
cargo test --locked -p iroh-rooms-net --all-features --test join_e2e \
  uninvited_provisional_dialer_receives_no_live_fanout -- --exact
cargo test --locked -p iroh-rooms-core --all-features --test sync_smoke \
  want_events_cannot_serve_an_id_from_another_room_in_the_shared_store -- --exact
cargo test --locked -p iroh-rooms-core --all-features --lib \
  sync::engine_tests::
cargo test --locked -p iroh-rooms-net --all-features --lib \
  transport::tests::superseded_provisional_link_teardown_preserves_the_successors_gate \
  -- --exact
cargo test --locked -p iroh-rooms-net --all-features --lib \
  transport::tests::invalidate_link_makes_a_late_teardown_a_noop_preserving_deauthorized \
  -- --exact
cargo test --locked -p iroh-rooms-core -p iroh-rooms-net \
  --all-targets --all-features
scripts/verify.sh

# Jeliya checkout at 9c71fac2104a74076662177cf4ef74bb5050bae9
DART_SDK_INCLUDE="$HOME/flutter/bin/cache/dart-sdk/include" \
  cargo test --locked --workspace
DART_SDK_INCLUDE="$HOME/flutter/bin/cache/dart-sdk/include" \
  node scripts/e2e.mjs --mode loopback
cargo run --locked -p jeliyad --features relay-only-test -- \
  --verification-relay-only-build
! cargo run --locked -p jeliyad -- --verification-relay-only-build
```

- the exact `kortiene/iroh-room#121` live-fanout regression passed (1/1);
- the malicious `WantEvents` isolation regression passed (1/1), including its
  foreign-parent and administrative-tip oracles;
- both targeted connection-generation teardown regressions passed (2/2),
  covering a superseded provisional link and a late deauthorization teardown;
- all five deterministic store retry/degradation tests passed (5/5);
- `iroh-rooms-core` and `iroh-rooms-net` passed 806 tests across all targets and
  all features, with two intentional ignores;
- upstream's canonical `scripts/verify.sh` passed formatting, clippy, workspace
  tests, doctests, and example builds (1,780 tests passed, 20 ignored);
- Jeliya's locked workspace suite passed 77 tests with one intentional
  performance ignore; and
- the two-daemon loopback suite passed all 67 assertions; and
- the relay-only build emitted `jeliya-relay-only-test-build-v1`, while the
  normal build rejected the hidden attestation flag with exit status 2.

This is exact-revision local evidence. It does not replace the signed
three-egress direct and forced-relay runs required above.

## Superseded candidate provenance: iroh-room v0.1.0-rc.3 (2026-07-16)

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
transfer to this pin. Signed direct and forced-relay runs were executed on
2026-07-16 from public commit `55024a4` carrying the rc.3 pin. Those manifests
remain certifying evidence for that exact snapshot, not for the current
`a5d98b70...` dependency candidate. The rc.3 residuals were live event fanout
to an unproven provisional dialer and store holes from swallowed insert errors;
both are addressed by the 2026-07-19 repin described above.
Mixed-fleet caution: a `v0.5.0`-era joiner cannot bootstrap from an rc.3
admin (it sends no capability proof), and an rc.3 joiner hard-stalls
bootstrapping from a `v0.5.0`-era responder once it holds more than ~1k
events — a room's members, especially its admin, must upgrade together.
