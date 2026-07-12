---
type: "Policy"
title: "Jeliya documentation profile"
description: "Metadata, navigation, linking, and CI rules for the repository's OKF-compatible documentation wiki."
tags: ["documentation", "governance", "okf", "wiki"]
timestamp: "2026-07-12T12:21:59Z"
status: "canonical"
implementation_status: "implemented"
verification_status: "verified"
release_status: "not-applicable"
audience: ["contributors", "documentation-authors", "maintainers"]
---

# Jeliya documentation profile

This document defines the repository's documentation contract. The `docs/`
directory is the canonical, docs-as-code wiki for Jeliya. Documentation changes
travel with code changes, use the same review history, and can later be rendered
by any compatible documentation frontend without creating a second source of
truth.

The profile is based on the [Open Knowledge Format (OKF) v0.1
draft](https://github.com/GoogleCloudPlatform/knowledge-catalog/blob/ee67a5ca27044ebe7c38385f5b6cffc2305a9c1a/okf/SPEC.md).
The commit is pinned because OKF is still a draft and its reference tooling may
change independently. Jeliya deliberately narrows the format where predictable
authoring and CI validation matter more than accepting every YAML or Markdown
variant.

## Scope and source of truth

- Every Markdown file under `docs/` is a Jeliya documentation concept except
  the reserved `index.md` navigation file.
- Root project documents such as `README.md`, `PRODUCT.md`, `DESIGN.md`,
  `CHANGELOG.md`, and `SECURITY.md` remain outside the bundle. The wiki index
  links to them as repository resources instead of duplicating their content.
- Git history is authoritative for authorship and change history. Do not add an
  OKF `log.md`; a manually maintained log would duplicate Git and can drift.
- A document marked `canonical` is the current source of truth for its stated
  scope. This says nothing by itself about whether the subject is implemented,
  verified, or released; those states have separate required fields.

## Required frontmatter

Every concept starts with a YAML frontmatter block containing exactly these
required fields:

```yaml
---
type: "Reference"
title: "Jeliya daemon protocol (v1)"
description: "Normative transport-neutral contract between jeliya-core and every Jeliya client."
tags: ["architecture", "protocol", "daemon"]
timestamp: "2026-07-11T02:40:00Z"
status: "canonical"
implementation_status: "implemented"
verification_status: "verified"
release_status: "released"
audience: ["client-authors", "contributors", "maintainers"]
---
```

The repository profile accepts a safe, deterministic YAML subset:

- keys are unquoted ASCII identifiers;
- string values are double-quoted;
- `tags` and `audience` are non-empty flow-style arrays of double-quoted
  strings;
- nested mappings, custom tags, anchors, aliases, merge keys, block scalars,
  duplicate keys, and implicit scalar typing are not allowed;
- unknown fields are rejected until this profile explicitly defines them.

### Fields

| Field | Contract |
|---|---|
| `type` | One controlled document type from the table below. |
| `title` | Human-readable title. It must match the document's single level-one heading. |
| `description` | One sentence explaining the document's scope and value. |
| `tags` | Lowercase topical tokens used for discovery. |
| `timestamp` | UTC ISO 8601 time of the last meaningful content change. |
| `status` | The document lifecycle only: one value from the lifecycle table below. |
| `implementation_status` | Whether the subject described by the document exists in the current candidate tree. |
| `verification_status` | Strength and currency of evidence for the subject described by the document. |
| `release_status` | Whether the subject described by the document is present in public release artifacts. |
| `audience` | Lowercase tokens naming the intended readers. |

Update `timestamp` when the document's meaning or contract changes. Formatting,
link repair, or metadata-only migration does not require inventing a new content
date; the last meaningful Git change may be used during migration.

## Controlled document types

| Type | Use |
|---|---|
| `Architecture` | System boundaries, component responsibilities, and proposed technical designs. |
| `Reference` | Normative protocols, schemas, contracts, and stable lookup material. |
| `Guide` | Task-oriented instructions for developers or operators. |
| `Runbook` | Repeatable operational, release, or incident procedures. |
| `Decision` | A durable decision and its rationale, evidence, and consequences. |
| `Policy` | Repository governance and rules that contributors must follow. |
| `Research` | Investigation whose findings inform, but do not themselves enact, a decision. |
| `Status Report` | Time-bound evidence, test results, or milestone outcomes. |
| `Glossary` | Controlled terminology and language guidance. |

Do not create a new type for a single document. Extend the vocabulary only when
at least two documents need a distinct handling or discovery behavior.

## Independent status axes

The four status fields answer different questions and must never be collapsed
into one optimistic label:

| Axis | Question |
|---|---|
| `status` | Is this document settled and current? |
| `implementation_status` | Does the described behavior exist in the candidate code? |
| `verification_status` | What evidence supports the described behavior? |
| `release_status` | Can a user obtain the described behavior from a published artifact? |

Use the least favorable truthful value when a document covers multiple
surfaces. Put finer-grained status in a capability matrix rather than inflating
the page-level metadata.

### Document lifecycle (`status`)

| Status | Meaning |
|---|---|
| `draft` | Incomplete working material that must not be treated as settled. |
| `proposal` | A reviewable recommendation or design that is not yet adopted or implemented. |
| `canonical` | The current source of truth for the document's defined scope. |
| `deprecated` | Retained for history or link continuity; readers must follow its replacement. |

The lifecycle describes the document, not a feature's runtime state. A canonical
release plan may truthfully list both implemented and unimplemented work. A
proposed architecture remains `proposal` even if parts of its current-state
analysis are verified.

### Implementation status

| Status | Meaning |
|---|---|
| `not-applicable` | The document is policy, governance, or evidence for which implementation is not a meaningful claim. |
| `planned` | The subject is intended but not implemented in the candidate tree. |
| `partial` | Some required surfaces or acceptance criteria are implemented; material work remains. |
| `implemented` | The described subject exists in the candidate tree. This does not imply verification or release. |

### Verification status

| Status | Meaning |
|---|---|
| `not-applicable` | The subject makes no behavior or outcome claim that requires verification. |
| `unverified` | No acceptable evidence has been recorded for the candidate revision. |
| `partial` | Some assertions passed, but required environments, negative cases, or repetitions remain. |
| `verified` | The documented acceptance evidence passed for the exact recorded revision and environment. |
| `historical` | Evidence is valid for an older revision or dependency set and must not certify the current candidate. |

### Release status

| Status | Meaning |
|---|---|
| `not-applicable` | The subject is not a releasable product capability or artifact. |
| `unreleased` | It is absent from the latest public release artifacts. |
| `partial` | Only some documented surfaces or platforms are publicly released. |
| `released` | It is present in the latest public release artifacts identified by the document. |

`verified` never implies `released`, and `released` never implies the current
candidate has passed its new acceptance gates. Status reports must identify the
tested Git commit, dependency revisions, environment, timestamp, assertions,
and sanitized evidence location. If any of these are missing, use `partial`,
`unverified`, or `historical` as appropriate.

## Navigation and links

- `docs/index.md` is the root of the wiki and intentionally has no
  frontmatter. It is manually curated around reader tasks, not generated as an
  alphabetical file list.
- Every concept must be reachable from `docs/index.md` through local Markdown
  links. Orphaned documents fail CI.
- Use file-relative links (`PROTOCOL.md`, `../README.md`, or
  `signing-notarization.md#acceptance-checklist`). Never use a leading slash for
  a repository document.
- Local paths and heading fragments must resolve. External links must use
  `https://`; the validator checks their shape but does not make network
  requests.
- Moving a document changes its OKF concept ID. Avoid moves without a concrete
  information-architecture benefit, and update every inbound link in the same
  change.

## Markdown and citations

- Author CommonMark-compatible Markdown with GitHub-style tables, task lists,
  and fenced code blocks where they improve execution.
- Each document has exactly one `#` heading, placed immediately after the
  frontmatter. Start its internal structure at `##`.
- Raw HTML elements are prohibited; non-rendered HTML comments are allowed.
  A future renderer must still sanitize Markdown and enforce a no-network
  content security policy.
- Use a final `## Citations` section when external evidence supports material
  factual claims. Code and repository links may stay next to the claim they
  support.
- Keep secrets, invite tickets, daemon tokens, private keys, and unnecessary
  personal data out of documentation and examples.

## CI contract

Run the documentation gate from the repository root:

```sh
node scripts/check-docs.mjs
```

The gate validates frontmatter, all four controlled status axes, timestamps,
unique titles, the single-H1 contract, local paths and fragments, and
reachability from `docs/index.md`. A documentation change is not complete until
the gate passes.

## Non-goals

This profile does not define a product knowledge runtime, retrieval system,
vector index, P2P bundle protocol, or agent trust boundary. It structures the
repository wiki. Jeliya's signed room event log remains the product's source of
operational truth.
