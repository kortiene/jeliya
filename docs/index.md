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

- [Capability status](capability-status.md) - What is implemented, verified, and publicly released as of v0.5.0 and the post-release candidate.
- [Platform matrix](platform-matrix.md) - Runtime, packaging, verification, and release status by operating system and artifact.
- [Release versus main](release-vs-main.md) - Exact boundary between released v0.5.0, its certified evidence, and the post-release candidate on main.
- [Verification evidence](verification-evidence.md) - Revision-bound milestone ledger, remote-test record, and evidence-sanitization contract.
- [Known gaps and roadmap](known-gaps-roadmap.md) - Release blockers, deferred risks, owners, and the NOW/NEXT/LATER boundary.

## Architecture and protocols

- [Daemon protocol](PROTOCOL.md) - Normative transport-neutral contract between `jeliya-core` and every Jeliya client.
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

## Operations and release evidence

- [Real-network NAT runbook](realnet-runbook.md) - Procedure for proving direct or relayed connectivity across two networks.
- [Historical Gate A result](gate-a-result.md) - Older direct-connectivity evidence that does not certify the v0.5.0 candidate.
- [Signing and notarization](signing-notarization.md) - Release-security plan for macOS and Windows artifacts.

## Language, identity, and governance

- [Internationalization](i18n.md) - Language roadmap and engineering rules for maintainable localization.
- [French glossary](glossary-fr.md) - Canonical French terminology and localization decisions.
- [Naming decision](naming.md) - Decision record and trademark research supporting the rename to Jeliya.
- [Documentation profile](PROFILE.md) - Metadata, navigation, linking, and CI rules for this wiki.
