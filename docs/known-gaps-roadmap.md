---
type: "Status Report"
title: "Known gaps and roadmap"
description: "Release blockers, deferred risks, owners, and next actions for the v0.5.0 evidence-backed technical preview."
tags: ["gaps", "release", "risks", "roadmap"]
timestamp: "2026-07-12T23:55:23Z"
status: "canonical"
implementation_status: "partial"
verification_status: "partial"
release_status: "unreleased"
audience: ["contributors", "maintainers", "product", "release-engineers"]
---

# Known gaps and roadmap

The `NOW` phase adds no product capability. It hardens the existing engineering
alpha and remains **blocked for release** despite substantial local progress.

## NOW — closure status

| Area | Evidence now available | Remaining release condition | Owner | Status |
|---|---|---|---|---|
| Public room-scoped authorization | centralized guard; 17 negative RPCs, local-file denial, and aggregate filtering pass; foreign agent projection exercised | preserve gates on the final public candidate | core maintainer | locally closed |
| Accepted-room provenance | failure-injected create/join ordering, serialized concurrent updates, cached reads, owner-only Unix state, and durable Unix/Windows replacement semantics pass locally | execute the configured Windows job and preserve these tests on the final public candidate | core maintainer | locally closed; hosted Windows proof pending |
| Upstream synchronization isolation | local `3702e8c…` remediation is clean and its malicious-sync tests pass | review and publish upstream fix, pin it immutably in Jeliya, then rerun qualification | upstream and core maintainer | **blocked** |
| Android and agent secrets | Android cloud/device-transfer exclusions, app-private no-backup identity storage, external agent data default, ignore and tracked-secret gates pass | keep controls in final candidate; Keystore wrapping is defense-in-depth, not a current claim | mobile and agent maintainers | locally closed |
| Dependency security | Cargo and npm report zero vulnerabilities; four unmaintained/yanked warnings have owner, mitigation, and expiry records | rerun against final lockfiles; no high/critical exception may be implicit | dependency owner | locally closed |
| CI completeness | all required matrix jobs and fail-closed prerequisites are defined; manual dispatch does not publish; Gradle is checksum-verified before execution | push authority and two clean hosted runs on the final public commit | CI maintainer | **blocked** |
| Agent/fleet reliability | agent E2E passes; fleet stability passed 5/5; Linux orphan/zombie cleanup verified on `demo1` under UID `65534` | repeat in final hosted gates | agent maintainer | locally closed |
| Direct network behavior | schema 2 run `3c938c66` at `0f6769a…`: three peers, distinct egress, two ASNs, 36/36 and cleanup pass; no synchronization-isolation claim | publish and pin the safe upstream revision, then rerun from the public Jeliya commit with a valid retained-evidence signature | verification owner | current functional pass; **not certifiable** |
| Forced relay behavior | exact public-pin relay-only build failed closed before remote execution because `3cb9bfd…` lacks the reviewed seam; older schema 1 `f1d9c149` is historical local-remediation evidence only | publish and pin the reviewed seam, then obtain a current schema 2 36/36 signed relay run paired with direct evidence from the same commit and toolchain | verification owner | **blocked; no current relay pass** |
| Evidence authenticity | release gate validates detached Ed25519 evidence signatures | authorize key custody and commit only the canonical public key before the qualifying run; never commit the private key | release authority | **blocked, fails closed** |
| Unix installer integrity | behavioral checksum-before-extraction tests pass | rerun against final artifacts | release maintainer | locally closed |
| Windows installer integrity | behavioral checksum/tamper, simulated reparse rejection, and native daemon smoke jobs are configured | obtain a passing hosted Windows result on the final candidate | release maintainer | **blocked; configured, unexecuted** |
| Complete asset-set visibility | immutable actions, verified build tools, execution-free validation and receipt sealing, isolated read-only smoke, receipt-only writer verification, draft-until-complete publication, and final-step token isolation are implemented | execute only after all gates pass and explicit release authority is granted; inspect and recover manually if cleanup is interrupted because GitHub cannot transact the tag and release assets atomically | release authority | implemented, never executed |
| Complete artifact set | `v0.4.3` has five published archives | build and verify all five `v0.5.0` daemon-plus-embedded-UI archives and sidecars together | release maintainer | **blocked; complete set absent** |
| Documentation alignment | status pages distinguish current schema 2 direct evidence, the failed-closed current relay build, and historical schema 1 local-remediation evidence | final reconciliation after public pins, hosted runs, signatures, and artifact verification | documentation owner | current for this snapshot; release remains blocked |

No reachable high or critical advisory is currently unresolved. The four
maintenance/yank warnings are tracked with mitigation and an expiry of
2026-09-30; expiry requires reassessment, not silent acceptance.

## Explicit preview limitations

- the macOS Flutter application is unpublished and its bundled sidecar remains
  loopback-only;
- Android has local device-smoke evidence, not direct, relay, NAT, reconnect,
  or cross-network evidence; its identity is app-private and backup-excluded,
  not Keystore-backed;
- iOS has no application scaffold or engine build;
- macOS arm64, Linux arm64 musl, and Windows have no complete `v0.5.0`
  candidate artifact evidence;
- bare daemon binaries are unsigned; macOS notarization and Windows
  Authenticode are inactive;
- WCAG 2.1 AA remains a design target with targeted checks, not enforced or
  certified conformance;
- member removal cannot recall data already copied by an authorized peer;
  revocation semantics require a separate protocol and product decision.

## Exit criteria for NOW

`v0.5.0` reaches a release-authority decision only when:

1. upstream `3702e8c…` or its reviewed successor is published and pinned by the
   final public Jeliya commit;
2. the approved evidence public key predates qualification, and signed direct
   and relay manifests pass the release gate with `certifiable: true`;
3. every required hosted CI gate passes twice from clean environments;
4. Windows behavioral checks and the other target-specific gates pass;
5. all five daemon-plus-embedded-UI archives and sidecars are built and
   verified before publication begins;
6. tag, daemon, changelog, and public names agree on `v0.5.0`;
7. [Capability status](capability-status.md),
   [Platform matrix](platform-matrix.md),
   [Release versus main](release-vs-main.md), and
   [Verification evidence](verification-evidence.md) match that final commit;
8. explicit release authority is granted to the sole publishing job.

## NEXT — after the preview

- operate signing, notarization, and evidence keys with documented custody,
  rotation, and incident response;
- add comprehensive accessibility automation and scheduled manual audits;
- verify Android direct, relay, reconnect, background, and NAT behavior across
  representative devices and networks;
- evaluate Android Keystore-backed identity wrapping without weakening backup
  exclusions or recoverability;
- define member removal and key-rotation semantics before promising revocation;
- automate privacy-reviewed retained evidence publication after a successful
  release.

## LATER — separate product decisions

iOS support, hosted agents, an agent marketplace, new protocol event types,
and other user-facing capabilities require separate product, security, and
architecture decisions. They remain outside this milestone.
