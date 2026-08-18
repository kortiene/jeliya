# Jeliya documentation

This directory is Jeliya's canonical docs-as-code wiki. Start with the project
foundations, then follow the section that matches the work you are doing. The
[documentation profile](PROFILE.md) defines metadata, lifecycle, linking, and
CI rules for every page in this wiki.

## Project foundations

- [README](../README.md) - Product overview, installation, first room, and contributor entry points.
- [Product](../PRODUCT.md) - Users, product purpose, principles, and accessibility commitments.
- [Design system](../DESIGN.md) - Visual language, components, responsive behavior, and interaction contracts.
- [Contributing](../CONTRIBUTING.md) - Contribution requirements, repository conventions, and required verification.
- [Security](../SECURITY.md) - Vulnerability reporting, threat-model boundaries, and current security posture.
- [Changelog](../CHANGELOG.md) - Shipped changes by release.

## Current status and evidence

- [Capability status](capability-status.md) - What is implemented, verified, and publicly released as of v0.6.0 and the v0.6.1 preparation line.
- [Platform matrix](platform-matrix.md) - Runtime, packaging, verification, and release status by operating system and artifact.
- [Release versus main](release-vs-main.md) - Exact boundary between released v0.6.0, its certified evidence, and the v0.6.1 preparation line on main.
- [Verification evidence](verification-evidence.md) - Revision-bound milestone ledger, remote-test record, and evidence-sanitization contract.
- [Known gaps and roadmap](known-gaps-roadmap.md) - Release blockers, deferred risks, owners, and the NOW/NEXT/LATER boundary.

## Architecture and protocols

- [Dioxus clean-slate architecture](dioxus-architecture.md) - Decision record for the clean-slate typed Rust client stack on Dioxus system-WebView rendering, its protocol and storage generation, the single embedded artifact, and the retirement of React, Flutter, the Dart protocol, and the C ABI. Decided; M1 typed-API slices, the M2 runtime (#167/#168/#169/#174), the M3 web foundation (#176/#177/#178), and the M4 supervisor (#170) are implemented; M1 remainders (#165/#166/#233) and the full program are in progress.
- [First-release distribution boundary](first-release-distribution.md) - Decision record for how the first release is delivered: one content-addressed artifact served by the local daemon and embedded in packaged desktop targets, the trust boundary each surface sits on, the operator-pasted pairing code that authenticates an ordinary browser, and the hosted-origin work deferred behind a new decision. Decided and not yet built.
- [Dioxus web build and reproducibility](dioxus-web-build.md) - The reproducible-build contract for the shared `jeliya-ui` crate (#176): the pinned toolchain, the deterministic wasm recipe, the single canonical artifact the daemon embeds, the build-time guard that rejects React output, and the development and production commands. Implemented; the release-line cutover (#200) and the sealed manifest (#183) remain open.
- [Daemon protocol](PROTOCOL.md) - Normative transport-neutral contract between `jeliya-core` and every Jeliya client, and the contract every released daemon speaks.
- [Protocol v2](protocol-v2.md) - Draft clean-slate contract for the Dioxus client stack: the three-layer handshake and generation gate, the 33 approved operations, the sequenced push stream with gap detection and authoritative resync, and the conformance corpus. Substance settled; per-operation wire schemas, the complete error taxonomy, and the fixture DSL are still open.
- [Shared-file size policy](shared-file-size.md) - Decision record retaining 104,857,600 bytes as the protocol-v2 maximum shared-file size, the served preflight and distinctive over-limit error it requires, and the provisional resource budgets and falsifiers that bound it.
- [How the embedded UI artifact is compressed on the wire](ui-artifact-wire-encoding.md) - Decision record sealing Brotli and gzip variants of the embedded UI artifact in its content-addressed manifest, served by static content negotiation with the canonical digest kept over the uncompressed bytes, applied identically to embedded and `--ui-dir` sources, and failing closed on a corrupt or missing variant. Decided and not yet built.
- [Product behavior contract](product-behavior-contract.md) - The required clean-slate cross-platform product behaviors the Dioxus client stack must satisfy: destinations, routes, shells, truthful states, retained invariants, EN/FR, accessibility, per-platform rows, and evidence ownership. Recorded against fresh state; the M3 web shell, routing, and preference schema (#178) are the first implementing slices.
- [Room Workbench](room-workbench.md) - Decision record for the global-versus-room hierarchy, canonical routes, responsive shells, and status vocabulary.
- [Room attention](room-attention.md) - Decision record for evidence-backed room recency, device-local unread, and actionable attention, and the evidence rule each displayed field must satisfy.
- [Device-local self label](self-label.md) - Decision record for the editable, device-local self display name reusing the alias store keyed by the self identity id, its fallback, validation, migration, and privacy rules.
- [Cross-client design tokens](design-tokens.md) - Mapping from every design-token concept to its React custom property and its Flutter getter, the shared fixture, and the two gates that enforce it.
- [Agent orchestration](agent-orchestration.md) - Normative contract for agent liveness, task claims, fleet reads, and UI projections.
- [Security and threat model](security-threat-model.md) - Assets, trust boundaries, threats, controls, and residual risks for the technical preview.

## Agents

- [Run the Jeliya agent](agent-guide.md) - Operational and security guide for the room-driven agent runner.

## Proposals

- [Agent marketplace architecture](agent-marketplace.md) - Proposed, not-yet-implemented hosted-agent marketplace architecture, trust model, product flow, and delivery plan.

## Superseded records

The proposal below is superseded by
[Dioxus clean-slate architecture](dioxus-architecture.md). Its review is
retained as a valid, dated evidence record about the revision it examined.

- [Production deployment architecture](production-deployment.md) - Superseded pre-Dioxus proposal for a hosted PWA and native companion at app.jeliya.ai; never accepted, replaced under #113 by the [Dioxus clean-slate architecture](dioxus-architecture.md) and the [first-release distribution boundary](first-release-distribution.md), and excluded from the first release.
- [Production deployment architecture review](production-deployment-review.md) - Adversarially verified findings against that proposal, bound to the revision it reviewed at `043bd1e`.

## Operations and release evidence

- [Diagnostics and logging](diagnostics-logging.md) - Enable, locate, follow, rotate, and safely share `jeliyad` diagnostic logs on the released daemon and packaged desktop, and the boundary that keeps signed room event logs out of troubleshooting.
- [Accessibility release checklist](accessibility-checklist.md) - The screen-reader and keyboard behaviours automated checks cannot prove, verified by hand before a release.
- [Real-network NAT runbook](realnet-runbook.md) - Procedure for proving direct or relayed connectivity across two networks.
- [v0.6.1 evidence boundary](evidence/v0.6.1/index.md) - Empty qualification boundary reserved for fresh, signed v0.6.1 manifests after candidate designation.
- [Historical Gate A result](gate-a-result.md) - Older direct-connectivity evidence that does not certify the v0.5.0 candidate.
- [Signing and notarization](signing-notarization.md) - Release-security plan for macOS and Windows artifacts.
- [Native update, signing, and anti-rollback policy](native-update-policy.md) - Decision record for whether each published native channel has an updater, how update artifacts and metadata are authenticated, and the single-generation anti-rollback, fail-closed UX, recovery, diagnostics, and evidence contract. Decided and not yet built.

## Language, identity, and governance

- [Internationalization](i18n.md) - Language roadmap and engineering rules for maintainable localization.
- [French glossary](glossary-fr.md) - Canonical French terminology and localization decisions.
- [Naming decision](naming.md) - Decision record and trademark research supporting the rename to Jeliya.
- [Documentation profile](PROFILE.md) - Metadata, navigation, linking, and CI rules for this wiki.
