---
type: "Status Report"
title: "Known gaps and roadmap"
description: "Release blockers, deferred risks, owners, next actions after the v0.6.0 evidence-backed technical preview, and the verification the decided client retirement removes."
tags: ["gaps", "release", "risks", "roadmap"]
timestamp: "2026-07-27T22:58:56Z"
status: "canonical"
implementation_status: "partial"
verification_status: "partial"
release_status: "partial"
audience: ["contributors", "maintainers", "product", "release-engineers"]
---

# Known gaps and roadmap

`v0.6.0` shipped on 2026-07-16 at `2283a441...`: the release conditions were
met for source `55024a4...` + `71fbb500...` (signed certifying direct and relay
evidence, hosted gates, and a complete verified artifact set). The table below
records the gaps that carry forward to v0.6.1, whose candidate pins
`iroh-rooms` to `a5d98b70...`. Exact Jeliya candidate `a1af1cdc...` is now
designated and must earn fresh signed evidence at that pair.

**Amended 2026-07-27 (issue #157).** A clean-slate client architecture is now
decided, and none of it is built: one typed Rust client stack rendering in the
platform's system WebView is to replace the React and Flutter clients, one new
protocol and storage generation is to replace protocol v1, and the Dart
protocol package, the C ABI, and `jeliya-ffi` are to be retired. Read
[Dioxus clean-slate architecture](dioxus-architecture.md) first; it governs
this page's forward-looking sections — how the preview limitations are read,
the verification the retirement removes, NEXT, and LATER. It binds nothing in
the v0.6.1 candidate: the released `v0.6.0` facts, the closure table below, the
candidate's evidence, and its exit criteria stand exactly as recorded, and no
candidate reference moves because of the decision.

## NOW — closure status

| Area | Evidence now available | Remaining condition for the next release | Owner | Status |
|---|---|---|---|---|
| Public room-scoped authorization | centralized guard; 17 negative RPCs, local-file denial, and aggregate filtering passed locally and in both certifying network runs | preserve gates on the next candidate | core maintainer | closed for `v0.6.0` |
| Accepted-room provenance | failure-injected create/join ordering, serialized concurrent updates, cached reads, owner-only Unix state, and durable replacement semantics pass; hosted Windows job passes on `main` | preserve on the next candidate | core maintainer | closed |
| Upstream synchronization, provisional-peer, and store integrity | certified baseline for `v0.5.0` at `d0ceb0b…`; current `a5d98b70…` pin passes targeted fanout, isolation, and store-degradation regressions plus 806 core/net tests and the full Jeliya suites locally | rerun signed direct and relay qualification at `a5d98b70…` before the next release | upstream and core maintainer | current pin locally requalified; network qualification pending |
| Android and agent secrets | Android cloud/device-transfer exclusions, app-private no-backup identity storage, external agent data default, ignore and tracked-secret gates pass | keep controls; Keystore wrapping is defense-in-depth, not a current claim | mobile and agent maintainers | closed |
| Dependency security | Cargo and npm report zero vulnerabilities; four unmaintained/yanked warnings have owner, mitigation, and expiry records | rerun against the next candidate's lockfiles | dependency owner | closed |
| CI completeness | all eight required matrix jobs, including `linux-flutter`, pass on public `main` run `29704754961` at exact candidate `a1af1cdc…`; PR run `29703977510` passed on the identical tree; manual dispatch does not publish; Gradle is checksum-verified | preserve the two clean source-tree matrices through qualification and run the release gates after evidence lands | CI maintainer | candidate source tree clean twice |
| Agent/fleet reliability | agent E2E passes; fleet stability passed 5/5; Linux orphan/zombie cleanup verified on `demo1` under UID `65534` | repeat in the next candidate's hosted gates | agent maintainer | closed |
| Linux Flutter source app | Ubuntu 24.04 ARM64 local qualification and the hosted x86_64 `linux-flutter` job pass; the hosted result binds public `main` at exact candidate `a1af1cdc…` | obtain a Wayland result; define a compatibility baseline and distribution format; bundle a complete Rust dependency license inventory; establish signing before publication | desktop maintainer | source-supported; unpublished |
| Direct network behavior | signed run certifies released `v0.6.0` source `55024a4…` + `71fbb500…` | rerun at `a1af1cdc…` + `a5d98b70…` after both qualification refs exist | verification owner | designated; network run pending |
| Forced relay behavior | signed run certifies released `v0.6.0` source `55024a4…` + `71fbb500…`; the relay-only verifier still builds locally | rerun the source-built relay qualification at `a1af1cdc…` + `a5d98b70…` after the direct run | verification owner | designated; network run pending |
| Evidence authenticity | detached Ed25519 signatures over both v0.6.0 manifests verify against the committed public SPKI; the private key is absent from the checkout, repository history, and audited release archives | confirm approved out-of-band custody and key availability without importing it into the checkout; sign the v0.6.1 runs there | release authority | checkout clean; custody confirmation required |
| Unix installer integrity | behavioral checksum-before-extraction tests pass; `v0.6.0` installs via the version-pinned installer path | rerun against the next artifacts | release maintainer | closed |
| Windows installer integrity | hosted `windows-latest` behavioral job passes on `main`; a `v0.6.0` Windows zip and sidecar are published | rerun against the next artifacts | release maintainer | closed |
| Complete asset-set visibility | the publication workflow executed for `v0.6.0`: validation, sealing, isolated smoke, receipt verification, and draft-until-complete publication | re-execute for v0.6.1 under explicit authority | release authority | executed for `v0.6.0` |
| Complete artifact set | `v0.6.0` published all five daemon-plus-embedded-UI archives with sidecars | build and verify the v0.6.1 set together | release maintainer | closed for `v0.6.0` |
| Documentation alignment | status pages distinguish released `v0.6.0`, its immutable signed evidence, and designated v0.6.1 candidate `a1af1cdc…` | bind fresh signed evidence only under `docs/evidence/v0.6.1/` after the network reruns | documentation owner | current for candidate designation |

No reachable high or critical advisory is currently unresolved. The four
maintenance/yank warnings are tracked with mitigation and an expiry of
2026-09-30; expiry requires reassessment, not silent acceptance.

## Explicit preview limitations

**Amended 2026-07-27 (issue #157).** Every limitation below is a true statement
about the stack that is retiring — the React web client, the Flutter desktop
and Android applications, and their Dart transport — and about what `v0.6.0`
published. They are preserved as released fact and are not restated or softened
by the decision. None of them is *new* work in that stack: the conditions the
closure table already records for the `v0.6.1` candidate stand, and the
decision adds nothing beyond them. Where a limitation names behavior the
replacement stack must also provide, it is re-qualified against the
replacement on a clean install rather than closed here. Nothing is retired
before its replacement is qualified.

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
- the current upstream pin is an immutable but untagged commit. It fixes the
  provisional-peer fanout and store-hole residuals from `v0.1.0-rc.3`, but a
  long-term tagged-release and maintenance path is still required;
- exhausted store retries or queue overflow produce a durable critical
  `store_degraded` decision. Operators still need a documented response to real
  disk failure; and
- mixed pre/post-repin fleets cannot complete joins, so joiners and admins must
  upgrade together.

## Exit criteria for the next release

`v0.5.0` met its exit criteria and shipped on 2026-07-14. The next release
reaches a release-authority decision only when the same bar is met at the
new candidate:

1. the candidate's reviewed public pin (`a5d98b70…`, or a reviewed tagged
   successor carrying the same fixes) is carried by the final public commit;
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

## Verification the retirement removes

Retiring React, Flutter, and the Dart protocol package removes working
enforcement for which no replacement exists yet. The losses are recorded here
because they are invisible in the code that remains: every gate below passes
today and keeps passing right up to the change that deletes the stack it
inspects, so the coverage disappears without a failing check to announce it.

| Enforcement today | What retirement removes | Condition before it may lapse | Owner |
|---|---|---|---|
| the golden protocol conformance corpus, replayed by three independent oracles — the daemon, the TypeScript client, and the Dart/FFI client (see the [Dart protocol package](../dart/jeliya_protocol/README.md)) against the shared `ui/src/lib/conformance/corpus.json` | two of the three oracles; one implementation replaying its own vectors demonstrates self-consistency, not conformance | the replacement corpus is authored independently by hand and never generated from the implementation under test, and a corpus-versus-implementation disagreement is resolved by deciding which one is wrong | core maintainer and verification owner |
| both localization gates — `scripts/i18n-gate.mjs` over the Flutter catalog and `scripts/check-ui-i18n.mjs` over the React catalogs (key parity, empty values, and French values still byte-identical to their English source) | both gates, because each inspects a catalog that retires with its client | an equivalent gate enforces the same properties over the replacement catalog and passes before either is removed. French parity is the property most likely to regress silently, because an untranslated string still renders and still reads as working software | web maintainer |
| the cross-client design-token gate — `scripts/check-design-tokens.mjs` with `app/test/design_tokens_test.dart` over `assets/design-tokens.json`, recorded in [cross-client design tokens](design-tokens.md) | the parity half, which is meaningful only while two clients exist, and the contrast floors enforced alongside it, which have no successor consumer | the contrast floors are enforced against the replacement stack, and the token fixture keeps a consumer that fails when a floor is violated | cross-platform maintainers |
| the [accessibility checklist](accessibility-checklist.md)'s "what CI already covers" table, which points at `ui/e2e/a11y-matrix.spec.ts`, `ui/e2e/a11y.spec.ts`, and the `app/test/a11y_*.dart` suites | every suite the table names; the checklist would otherwise keep instructing reviewers not to re-verify by hand what nothing verifies | equivalent enforced coverage exists on the replacement, and the table is rewritten in the same change that removes the suites | web maintainer and documentation owner |

**No enforcement may lapse before its replacement is qualified.** Removing a
gate together with the code it inspects is permitted; removing the gate first
is not. A retirement change that cannot name the passing replacement gate is
not ready, and an unenforced property is an open gap even when the behavior it
protected is believed intact.

**New qualification results are enforced evidence, not certification.** A
replacement result states what was enforced, on which platform, at which
commit. WCAG 2.1 AA stays a design target with targeted checks until a gate
says otherwise, and no result claims certified conformance.

## NEXT — after the preview

The clean-slate decision re-scopes this list. Apart from key custody,
everything below is qualification work for the replacement stack on a clean
install rather than work on the retiring clients. None of it is started, no
result below exists, and each result is enforced evidence, not certification.

- operate signing, notarization, and evidence keys with documented custody,
  rotation, and incident response. The decision changes nothing here, and it
  applies to every artifact the replacement stack would publish — release
  authority;
- qualify Android network behavior on a clean install of the replacement
  stack — direct, relay, reconnect, background, and NAT across representative
  devices and networks — against the in-process client the decision selects,
  not against the retiring Flutter application — mobile maintainer;
- decide an Android system-WebView floor or evergreen policy. None exists: the
  decision captures a device's WebView version as evidence only and leaves the
  policy open, so no Android result may state a supported WebView range until
  one is decided — mobile maintainer;
- decide identity storage for the new generation, including whether Keystore
  wrapping is adopted, without weakening backup exclusion or recoverability.
  App-private, backup-excluded storage remains the floor, and Keystore wrapping
  remains defense-in-depth rather than a claim — mobile maintainer;
- gate the replacement stack on comprehensive accessibility automation and
  scheduled manual audits. This is no longer an increment on the current
  suites, because those retire with the clients they test — web, desktop, and
  mobile maintainers;
- qualify the packaged system WebView per platform: Linux on WebKitGTK with
  recorded library and glibc floors; Windows on WebView2 only if Windows is
  explicitly included rather than formally deferred, which is itself undecided;
  and macOS on the system WebView, whose floor is not yet recorded. Navigation,
  new-window, download, devtools, and storage policy fail closed, and a missing
  platform gate blocks that platform's publication row alone — there is no
  all-platform release barrier — desktop maintainer;
- carry the localization, design-token, and accessibility enforcement recorded
  above onto the replacement before any of it lapses — web maintainer and
  cross-platform maintainers;
- author the replacement conformance corpus independently, so the new protocol
  generation regains an oracle it does not generate itself — core maintainer
  and verification owner;
- define member removal and key-rotation semantics before promising
  revocation. The new protocol generation is where they would be specified, and
  the decision does not specify them — core maintainer;
- automate privacy-reviewed retained evidence publication after a successful
  release — verification owner.

## LATER — separate product decisions

iOS support, hosted agents, and an agent marketplace remain deferred and
require separate product, security, and architecture decisions. iOS is out of
scope in the clean-slate decision too, and no application scaffold or engine
build exists for it.

A future hosted or delegated browser architecture — a hosted origin, a service
worker, a delegated browser controller, a browser-resident room peer, native
companion pairing, or a browser-owned identity — is deferred on the same terms.
Each requires a new decision record, a new threat model, and a separately
approved backlog before any of it is built; the clean-slate decision authorizes
none of them.

New protocol event types are no longer a separate deferral. They belong to the
new generation's specification, which is open and unwritten, together with the
questions the decision deliberately left to it: the shared-file maximum — the
current 100 MiB limit is a v1 reference, not a decision for the new generation
— and absolute performance budgets. A new user-facing capability still needs a
product decision; it no longer needs a protocol-generation decision beside it.
