---
type: "Status Report"
title: "Known gaps and roadmap"
description: "Release blockers, deferred risks, owners, and next actions for the v0.5.0 evidence-backed technical preview."
tags: ["gaps", "release", "risks", "roadmap"]
timestamp: "2026-07-16T15:30:00Z"
status: "canonical"
implementation_status: "partial"
verification_status: "partial"
release_status: "partial"
audience: ["contributors", "maintainers", "product", "release-engineers"]
---

# Known gaps and roadmap

`v0.5.0` shipped on 2026-07-14: the release conditions the `NOW` phase tracked
were met (published safe pin, signed certifying direct and relay evidence,
hosted gates, complete verified artifact set). The table below records that
closure and the gaps that carry forward to the post-release candidate on
`main`, which repins `iroh-rooms` to `v0.1.0-rc.3` and must earn its own
evidence.

## NOW — closure status

| Area | Evidence now available | Remaining condition for the next release | Owner | Status |
|---|---|---|---|---|
| Public room-scoped authorization | centralized guard; 17 negative RPCs, local-file denial, and aggregate filtering passed locally and in both certifying network runs | preserve gates on the next candidate | core maintainer | closed for `v0.5.0` |
| Accepted-room provenance | failure-injected create/join ordering, serialized concurrent updates, cached reads, owner-only Unix state, and durable replacement semantics pass; hosted Windows job passes on `main` | preserve on the next candidate | core maintainer | closed |
| Upstream synchronization isolation | certified for `v0.5.0` at published pin `d0ceb0b…`; the rc.3 candidate keeps the fix, adds the join-bootstrap capability gate, and passes upstream store/engine isolation regressions plus the full Jeliya suites locally at `71fbb50…` | rerun signed network qualification at the rc.3 pin before the next release | upstream and core maintainer | certified for `v0.5.0`; rc.3 locally requalified |
| Android and agent secrets | Android cloud/device-transfer exclusions, app-private no-backup identity storage, external agent data default, ignore and tracked-secret gates pass | keep controls; Keystore wrapping is defense-in-depth, not a current claim | mobile and agent maintainers | closed |
| Dependency security | Cargo and npm report zero vulnerabilities; four unmaintained/yanked warnings have owner, mitigation, and expiry records | rerun against the next candidate's lockfiles | dependency owner | closed |
| CI completeness | all required matrix jobs pass on hosted `main` runs; manual dispatch does not publish; Gradle is checksum-verified | hosted execution of the new `linux-flutter` job; keep two clean runs on the next final commit | CI maintainer | closed for the six pre-existing jobs |
| Agent/fleet reliability | agent E2E passes; fleet stability passed 5/5; Linux orphan/zombie cleanup verified on `demo1` under UID `65534` | repeat in the next candidate's hosted gates | agent maintainer | closed |
| Linux Flutter source app | Ubuntu 24.04 ARM64 release package, X11/Xvfb lifecycle, bundled daemon smoke, dependency, archive, and checksum gates pass locally; a hosted x86_64 `linux-flutter` CI job is now defined | obtain hosted x86_64 and Wayland results; define a compatibility baseline and distribution format; bundle a complete Rust dependency license inventory; establish signing before publication | desktop maintainer | source-supported; unpublished |
| Direct network behavior | signed certifying schema 2 run `3b86ac67` at `c5f740e…` + `d0ceb0b…` | rerun at the rc.3 candidate pin before the next release | verification owner | certified for `v0.5.0` |
| Forced relay behavior | signed certifying schema 2 relay run `a3c76859` with a self-attested relay-only build at the same revision pair; the rc.3 candidate verifier builds and attests locally | rerun at the rc.3 candidate pin before the next release | verification owner | certified for `v0.5.0` |
| Evidence authenticity | detached Ed25519 signatures over both certifying manifests verify against the committed public SPKI; private-key custody is out of band | keep custody; sign the next candidate's runs | release authority | closed |
| Unix installer integrity | behavioral checksum-before-extraction tests pass; `v0.5.0` installs via the version-pinned installer path | rerun against the next artifacts | release maintainer | closed |
| Windows installer integrity | hosted `windows-latest` behavioral job passes on `main`; a `v0.5.0` Windows zip and sidecar are published | rerun against the next artifacts | release maintainer | closed |
| Complete asset-set visibility | the publication workflow executed for `v0.5.0`: validation, sealing, isolated smoke, receipt verification, and draft-until-complete publication | re-execute for the next release under explicit authority | release authority | executed for `v0.5.0` |
| Complete artifact set | `v0.5.0` published all five daemon-plus-embedded-UI archives with sidecars | build and verify the next candidate's set together | release maintainer | closed for `v0.5.0` |
| Documentation alignment | status pages reconciled to the released `v0.5.0`, its certified evidence, and the rc.3 candidate boundary | re-reconcile at the next release cut | documentation owner | current for this snapshot |

No reachable high or critical advisory is currently unresolved. The four
maintenance/yank warnings are tracked with mitigation and an expiry of
2026-09-30; expiry requires reassessment, not silent acceptance.

## Explicit preview limitations

- the macOS Flutter application is unpublished and its bundled sidecar remains
  loopback-only;
- the Linux Flutter application is an unsigned, source-built developer
  package only; no native app archive is public, its x86_64 hosted and Wayland
  results are pending, the local ARM64 daemon requires GLIBC 2.39, the tarball
  lacks a complete Rust dependency license inventory, and direct, relay, NAT,
  and cross-network behavior are unverified;
- Android has local device-smoke evidence, not direct, relay, NAT, reconnect,
  or cross-network evidence; its identity is app-private and backup-excluded,
  not Keystore-backed;
- iOS has no application scaffold or engine build;
- bare daemon binaries are unsigned; macOS notarization and Windows
  Authenticode are inactive;
- WCAG 2.1 AA remains a design target with targeted checks, not enforced or
  certified conformance;
- member removal cannot recall data already copied by an authorized peer;
  revocation semantics require a separate protocol and product decision;
- the pinned upstream `v0.1.0-rc.3` carries documented residuals: while a room
  is accepting joins, an unproven provisionally-admitted dialer no longer
  receives history but still receives live event fan-out until it disconnects
  (upstream issue #121); a store hole left by a swallowed insert error heals
  only from peers that re-serve the region (upstream issue #119); and mixed
  pre/post-repin fleets cannot complete joins, so joiners and admins must
  upgrade together.

## Exit criteria for the next release

`v0.5.0` met its exit criteria and shipped on 2026-07-14. The next release
reaches a release-authority decision only when the same bar is met at the
new candidate:

1. the candidate's published pin (`v0.1.0-rc.3` at `71fbb50…`, or its
   reviewed successor) is carried by the final public commit;
2. signed direct and relay manifests bound to that commit and pin pass the
   release gate with `certifiable: true` (the `v0.5.0` evidence binds
   `c5f740e` + `d0ceb0b` and does not transfer);
3. every required hosted CI gate — now including `linux-flutter` — passes
   twice from clean environments;
4. Windows behavioral checks and the other target-specific gates pass;
5. the complete archive-and-sidecar set is built and verified before
   publication begins;
6. tag, daemon, changelog, and public names agree on the release version;
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
