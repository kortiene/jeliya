---
type: "Status Report"
title: "Verification evidence"
description: "Revision-bound verification ledger and evidence-recording contract for the v0.5.0 technical preview."
tags: ["evidence", "networking", "release", "testing", "verification"]
timestamp: "2026-07-12T12:21:59Z"
status: "canonical"
implementation_status: "partial"
verification_status: "partial"
release_status: "unreleased"
audience: ["contributors", "maintainers", "operators", "release-engineers"]
---

# Verification evidence

This is the candidate's evidence ledger. `PASS` is permitted only when the
record identifies the exact Jeliya commit, dependency revisions, environment,
timestamp, assertions, and sanitized evidence location. A pass on another
revision is `historical`, not transferable evidence.

## Candidate identity

| Field | Value |
|---|---|
| Milestone | `v0.5.0 — Evidence-Backed Technical Preview` |
| Baseline commit | `1285b42037a3713840955fa518f2b81b19f2929f` |
| Baseline `iroh-rooms` revision | `3cb9bfd1e43eb755c967315c37b6d4fd1c2bf020` |
| Candidate commit | pending; the working tree is not evidence |
| Candidate upstream remediation revision | pending publication and pin |
| Release evidence gate | BLOCKED |
| Evidence window | opened 2026-07-12 UTC |

## Milestone evidence ledger

| Gate | Required evidence | Current status |
|---|---|---|
| Public read-RPC authorization | negative tests for foreign timelines, members, agents, files, local file reads, and pipes; joined-then-left positive archive case | store-seeded never-joined-room regression passed locally; full candidate suite pending |
| Upstream synchronization isolation | malicious `WantEvents` and foreign-parent tests on the exact pinned upstream revision | targeted test passed on local upstream candidate `3702e8cbcd5ac1808791124dd6bc44068be5f822`; publication and Jeliya pin pending |
| Android backup exclusion | manifest and XML rule validation for cloud backup and device transfer | pending final candidate run |
| Agent secret location | platform-default directory outside checkout plus Git ignore/commit-prevention validation | pending final candidate run |
| Rust dependency audit | zero reachable high/critical advisories or approved, expiring exception record | pending final candidate run |
| npm dependency audit | zero reachable high/critical advisories or approved, expiring exception record | pending final candidate run |
| Complete CI | Rust, MSRV, TypeScript, Dart, Flutter, docs, smoke, sidecar, agent, fleet, and protocol gates with no silent skips | pending |
| Repeatability | all required CI gates twice from clean environments | pending |
| Direct different-network P2P | two authorized hosts, distinct networks, exact candidate, settled `direct` path, full assertions | pending |
| Deliberately constrained relay | no firewall or system changes; settled `relay` path with full assertions | pending; requires an application-level safe constraint |
| Join and resynchronization | targeted join, restart/reconnect, complete room resync | pending |
| Messages, files, and pipes | bidirectional messages, byte-exact BLAKE3 file fetch, pipe flow | pending |
| Absent-room public API non-disclosure over network | every room-scoped read plus aggregate reads omit a room ID that was never joined | local harness passed; source-bound remote rerun pending; this is not synchronization-isolation evidence |
| Installer integrity | Unix and PowerShell installers fetch and verify the published checksum before extraction | pending |
| Atomic publication | complete five-archive set validates before the sole write-enabled job publishes | pending; publication itself is not authorized |
| Version consistency | tag, daemon version, changelog, archive names, and checksums agree | pending |
| Documentation | metadata, links, capability claims, and evidence ledger pass the docs gate | local docs gate to be recorded after completion |

## Dependency-risk exception register

The updated candidate lockfiles have **no high or critical vulnerability
exception**. The entries below are `cargo audit` maintenance or yanked-version
warnings, not advisories that identify a security vulnerability in the pinned
code. They remain visible because the affected crates are present in reachable
transitive dependency paths. “Reachable” here means compiled through the named
dependency chain; it does not mean an exploit path has been demonstrated.

| Advisory or warning | Reachable dependency path | Risk assessment | Mitigation | Owner | Review and expiry |
|---|---|---|---|---|---|
| `RUSTSEC-2023-0089` — `atomic-polyfill` unmaintained | `postcard` through `iroh` | Maintenance risk: future defects may not receive fixes; no vulnerability is identified by this advisory | keep the resolved lockfile pinned, fail CI on new vulnerability findings, and monitor `postcard`/`iroh` for removal or replacement | Jeliya maintainers | review and expire 2026-09-30 |
| `RUSTSEC-2024-0436` — `paste` unmaintained | `netwatch` through `iroh` | Maintenance risk in a build-time macro dependency; no vulnerability is identified by this advisory | keep the resolved lockfile pinned, retain the automated audit gate, and monitor `netwatch`/`iroh` migration upstream | Jeliya maintainers | review and expire 2026-09-30 |
| `RUSTSEC-2024-0370` — `proc-macro-error` unmaintained | `genawaiter` through `iroh-blobs` | Maintenance risk in a procedural-macro dependency; no vulnerability is identified by this advisory | keep the resolved lockfile pinned, retain the automated audit gate, and monitor `genawaiter`/`iroh-blobs` remediation upstream | Jeliya maintainers | review and expire 2026-09-30 |
| yanked `num-bigint 0.4.7` | `x509-parser` and `rcgen` through `iroh` | Ecosystem support risk from a yanked version; the audit output does not identify a high or critical vulnerability for this version | keep the exact lockfile, prevent unreviewed resolution drift, run the audit gate on every change, and adopt the upstream `rcgen`/`iroh` resolution when compatible | Jeliya maintainers | review and expire 2026-09-30 |

Expiry means the warning must be removed, renewed with fresh evidence, or made
release-blocking. It is not permanent acceptance. Any new reachable high or
critical vulnerability remains a release blocker unless a separate, explicitly
approved and time-bounded security exception is recorded.

## Local upstream-candidate qualification — non-releaseable

The local Iroh Rooms candidate
`3702e8cbcd5ac1808791124dd6bc44068be5f822` contains the room-scoped event
lookup remediation and the compile-time relay-only verification seam. Targeted
local checks passed:

- `WantEvents` could not serve a known event ID from another room, and that
  foreign row remained missing when cited as a local causal parent;
- Jeliya's store-seeded regression kept a never-joined foreign room out of
  aggregate and room-scoped public reads;
- the normal Jeliya build rejected the hidden relay attestation flag;
- the public `3cb9bfd1e43eb755c967315c37b6d4fd1c2bf020` dependency failed the
  relay verifier build closed;
- an isolated build wired to
  `3702e8cbcd5ac1808791124dd6bc44068be5f822` propagated the relay-only feature and
  printed the exact fixed attestation without creating identity state.

This is not release evidence. The upstream candidate exists only in a local
worktree, Jeliya still pins public revision
`3cb9bfd1e43eb755c967315c37b6d4fd1c2bf020`, and no relay network
run used the candidate. Publication, immutable pinning, clean source-bound
builds, and the remote direct/relay runs remain mandatory.

## Historical evidence that does not certify this candidate

The 2026-07-04 Gate A run recorded a direct path, bidirectional messages, and a
verified file transfer. Its evidence-record commit is
`f2aea0959ee5bf0f91fee030bdd2e2466163671c` and its `iroh-rooms` pin is
`1d2f014e783893ffeaea055c436370179a31110a`. Those revisions differ from the
candidate baseline, and the raw local JSON was not committed. The result is
useful historical evidence only; details are in
[`gate-a-result.md`](gate-a-result.md).

The Android 13 device smoke similarly proves local engine lifecycle, room
operations, pushes, persistence, and UI integration with `loopback: false`.
It did not involve a remote peer or measure a direct/relay path, so it is not
Android real-network evidence.

## Local harness qualification — non-certifying

The replacement network harness completed a three-peer loopback qualification
run before any remote mutation:

| Field | Value |
|---|---|
| Run ID | `20260712T133929Z-be17800a` |
| Mode | local loopback machinery check |
| Source | baseline `1285b42037a3713840955fa518f2b81b19f2929f` with an intentionally dirty hardening worktree |
| `iroh-rooms` revision | `3cb9bfd1e43eb755c967315c37b6d4fd1c2bf020` |
| Local binary | `jeliyad 0.5.0`, SHA-256 `1583a2dca44fcd352135c3f23ffcec7ad402dd1062523d88dce39c22af27db07` |
| Peers | three local macOS x86_64 processes in isolated temporary directories |
| Assertions | 36 passed: targeted joins, three-peer convergence, direct loopback paths, bidirectional messages, BLAKE3 fetch plus byte-identical SHA-256, authorized pipe flow, unauthorized pipe zero-target-connection denial, one offline message resynchronized after reopen, absent-room-ID non-disclosure across read RPCs (including `peers.status`), and aggregate filtering |
| Cleanup | all harness processes stopped and all run-owned temporary directories removed |
| Log evidence | per-role stdout/stderr line and byte counts, raw-stream SHA-256, and bounded redacted excerpts; no raw logs persisted |
| Sanitized local evidence | `.jeliya-gatea/v0.5.0/20260712T133929Z-be17800a.json` (gitignored) |

This run qualifies the orchestration, provenance reporting, assertion, and
cleanup machinery only. It is explicitly `certifiable: false`: the source was
uncommitted, the diagnostic binary was not built by the harness, all peers
shared one machine, and no public relay was involved. It does not satisfy the
direct different-network or forced-relay milestone gates.

## Authorized remote inventory

Read-only SSH inventory found the proposed least-privilege pair below. This is
connectivity and environment inventory, not a P2P test result.

| Host alias | Intended role | Observed platform | Tooling constraint | Test status |
|---|---|---|---|---|
| `user@kilo` | remote peer B | Ubuntu 22.04 x86_64 | no Rust or Node toolchain; receive only the source-built, verified static binary | inventory only |
| `user@stargate-03` | remote peer C | Ubuntu 22.04 x86_64 | no Rust or Node toolchain; receive only the source-built, verified static binary | inventory only |
| `user@zulu` | alternate remote peer | Ubuntu 22.04 x86_64 | use only if one selected host cannot establish distinct public egress or a required path | inventory only |

Before certifying direct connectivity, record only whether all observed public
egress addresses differ, never the addresses themselves. This comparison does
not prove independent VPCs or routing domains, so do not label it as network
topology proof. The relay test must constrain the application transport
without changing host firewalls, routes, services, or system configuration.

## Required record for each remote run

```text
run_id: non-secret unique identifier
started_at_utc / ended_at_utc
jeliya_commit / tag
iroh_rooms_revision and resolved Cargo.lock source
artifact filename / sha256
host aliases and roles
OS / architecture
isolated temporary directories and allocated local control ports
distinct_public_egress: sanitized pairwise comparison and explicit topology caveat
observed peer path: direct | relay
assertions: join, offline-message resync, messages, file hash, pipes, reconnect,
            absent-room-ID non-disclosure
sync isolation: upstream WantEvents/foreign-parent tests plus Jeliya store-seeded test
result: pass | fail | blocked
sanitized logs: per role/stream line count, byte count, SHA-256, bounded redacted excerpt
cleanup: processes stopped and only run-owned temporary artifacts removed
```

Never record invite tickets, bearer tokens, portfile tokens, identity seeds,
private keys, full public addresses, room contents unrelated to the test, or
raw environment dumps. Raw logs are never persisted. Only the structured,
bounded summaries above may become CI or documentation artifacts; their
excerpts redact all secrets held in memory, credential-shaped labels, and long
hex/base64-like values.
