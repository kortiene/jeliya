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

- [Dioxus clean-slate architecture](dioxus-architecture.md) - Decision record for the clean-slate typed Rust client stack on Dioxus system-WebView rendering, its protocol and storage generation, the single embedded artifact, and the retirement of React, Flutter, the Dart protocol, and the C ABI. Decided and not yet built.
- [Daemon protocol](PROTOCOL.md) - Normative transport-neutral contract between `jeliya-core` and every Jeliya client, and the contract every released daemon speaks.
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

- [Production deployment architecture](production-deployment.md) - Superseded pre-Dioxus proposal for a hosted PWA and native companion at app.jeliya.ai; never accepted, replaced under #113 by the [Dioxus clean-slate architecture](dioxus-architecture.md), and excluded from the first release.
- [Production deployment architecture review](production-deployment-review.md) - Adversarially verified findings against that proposal, bound to the revision it reviewed at `043bd1e`.

## Operations and release evidence

- [Accessibility release checklist](accessibility-checklist.md) - The screen-reader and keyboard behaviours automated checks cannot prove, verified by hand before a release.
- [Real-network NAT runbook](realnet-runbook.md) - Procedure for proving direct or relayed connectivity across two networks.
- [v0.6.1 evidence boundary](evidence/v0.6.1/index.md) - Empty qualification boundary reserved for fresh, signed v0.6.1 manifests after candidate designation.
- [Historical Gate A result](gate-a-result.md) - Older direct-connectivity evidence that does not certify the v0.5.0 candidate.
- [Signing and notarization](signing-notarization.md) - Release-security plan for macOS and Windows artifacts.

## Language, identity, and governance

- [Internationalization](i18n.md) - Language roadmap and engineering rules for maintainable localization.
- [French glossary](glossary-fr.md) - Canonical French terminology and localization decisions.
- [Naming decision](naming.md) - Decision record and trademark research supporting the rename to Jeliya.
- [Documentation profile](PROFILE.md) - Metadata, navigation, linking, and CI rules for this wiki.
