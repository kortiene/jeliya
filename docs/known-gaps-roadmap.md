---
type: "Status Report"
title: "Known gaps and roadmap"
description: "Release blockers, deferred risks, owners, and next actions for the v0.5.0 evidence-backed technical preview."
tags: ["gaps", "release", "risks", "roadmap"]
timestamp: "2026-07-12T12:21:59Z"
status: "canonical"
implementation_status: "partial"
verification_status: "partial"
release_status: "unreleased"
audience: ["contributors", "maintainers", "product", "release-engineers"]
---

# Known gaps and roadmap

The `NOW` phase adds no product capability. It closes trust, evidence, and
release-integrity gaps around the existing engineering alpha. Any new feature
proposal waits until this milestone is either ready or explicitly stopped.

## NOW — `v0.5.0` release blockers

| Gap | Risk | Required closure | Owner | Status |
|---|---|---|---|---|
| Public read RPCs can expose foreign-room projections | cross-room confidentiality breach | centralized guard plus negative tests for timelines, members, agents, files, local-file reads, and pipes | core maintainer | implementation in progress; final evidence pending |
| Pinned upstream synchronization is not room-scoped for known event IDs and parents | malicious peer can extract or contaminate foreign-room state | publish reviewed upstream fix, pin immutable revision, rerun malicious synchronization tests | upstream and core maintainer | local fix only; release blocked |
| Android and agent identity state can escape intended storage boundaries | backup or accidental Git disclosure | explicit backup/data-extraction rules, safer default data directory, ignore and commit-prevention tests | mobile and agent maintainers | implementation present; validation pending |
| Reachable cargo/npm advisories | supply-chain or runtime compromise | resolve high/critical findings or record an approved expiring exception with reachability and mitigation | dependency owner | audits pending final lockfiles |
| CI omits or silently skips required surfaces | false confidence | enforce Rust, MSRV, TypeScript, Dart, Flutter, docs, smoke, sidecar, agent, fleet, and protocol jobs; fail on missing prerequisites | CI maintainer | workflow hardening pending |
| Agent E2E assertion is flaky | nondeterministic release gate | compare post-idle event IDs against a baseline and assert no new intruder-authored execution | agent maintainer | stabilization pending |
| Candidate direct and relay behavior lacks revision-bound evidence | unknown real-network behavior | two-host direct run and safely constrained relay run with full functional and authorization assertions | verification owner | remote inventory complete; runs pending |
| Installers do not guarantee archive integrity before extraction | modified binary execution | fetch exact checksum sidecar, validate format/name/hash, fail closed before extraction on Unix and Windows | release maintainer | implementation present; adversarial tests pending |
| Release jobs can publish incrementally or from mutable tooling | partial or compromised release | immutable action pins, verified Zig, read-only builders, complete artifact validation, one final publisher | release maintainer | workflow hardening pending |
| Tag, daemon, changelog, and filenames can drift | irreproducible or misleading release | one consistency gate over the complete artifact set | release maintainer | pending version bump and gate |
| Evidence and capability documentation can outrun code | governance failure | update all status pages from final evidence, run docs gate, review the diff | documentation owner | initial profile present; final reconciliation pending |

There is no standing exception for a reachable high or critical dependency
advisory. If an upgrade proves unsafe, the exception must identify the advisory,
reachable path, compensating control, owner, approval, and an expiry date no
later than 2026-08-12. Expiry is a reassessment deadline, not permanent
acceptance.

## Preview limitations that must be explicit

These gaps do not expand the `v0.5.0` artifact scope, but documentation and
release notes must not hide them:

- the macOS Flutter application is not a published artifact and its bundled
  sidecar remains loopback-only;
- Android has local device-smoke evidence but no different-network direct,
  relay, or NAT-traversal evidence and no published APK/AAB;
- iOS has no application scaffold or engine build;
- bare daemon binaries are unsigned, and macOS notarization and Windows
  Authenticode are not active;
- WCAG 2.1 AA is a design target with partial automated coverage, not a
  complete conformance certification;
- mobile background availability and native local-file open remain incomplete;
- member removal cannot recall data already copied by a previously authorized
  peer; revocation semantics require a separate product and protocol decision.

## Exit criteria for NOW

`v0.5.0` is ready for a release-authority decision only when:

1. the upstream room-isolation fix is published, pinned, and verified;
2. every required local and CI gate passes twice from clean environments;
3. direct different-network and deliberately constrained relay runs pass on
   the exact candidate and are recorded without secrets;
4. five daemon-plus-embedded-UI archives build, verify, and agree on version;
5. installers verify their matching published checksums before extraction;
6. the release workflow can publish atomically but has not published without
   explicit authorization;
7. [`capability-status.md`](capability-status.md),
   [`platform-matrix.md`](platform-matrix.md),
   [`release-vs-main.md`](release-vs-main.md), and
   [`verification-evidence.md`](verification-evidence.md) match the final
   evidence.

## NEXT — after the preview

Only after NOW exits, prioritize operational hardening that increases trust
without reopening the product surface:

- obtain and operate signing/notarization credentials with a documented
  rotation and incident process;
- add comprehensive accessibility automation and scheduled manual audits;
- verify Android direct, relay, reconnect, and background behavior across
  representative devices and networks;
- define member removal and key-rotation semantics before promising revocation;
- publish retained, privacy-reviewed evidence bundles for each release.

## LATER — separate product decisions

iOS support, hosted agents, an agent marketplace, new protocol event types,
and other user-facing capabilities require their own product, security, and
architecture decisions. They are intentionally outside this milestone.
