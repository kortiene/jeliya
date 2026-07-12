---
type: "Status Report"
title: "Release versus main"
description: "Exact boundary between the latest published Jeliya artifacts, the audited baseline, and the v0.5.0 candidate."
tags: ["artifacts", "main", "release", "versions"]
timestamp: "2026-07-12T22:00:46Z"
status: "canonical"
implementation_status: "not-applicable"
verification_status: "verified"
release_status: "not-applicable"
audience: ["contributors", "maintainers", "operators", "release-engineers"]
---

# Release versus main

Git branches, test revisions, tags, and release assets answer different
questions. The `v0.5.0` candidate is **blocked** and must not be described as
shipped.

## Current boundary

| Layer | Exact revision | Dependency state | Artifact/evidence state | Claim allowed |
|---|---|---|---|---|
| Latest public release | tag `v0.4.3` at `9d62c3cd98c7f21d9683815c28278b6ac8c0b97f` | release lockfile | five published daemon archives and five checksum sidecars | only behavior in those archives is released |
| Audited baseline | `1285b42037a3713840955fa518f2b81b19f2929f` | pins vulnerable `iroh-rooms` `3cb9bfd…` | no artifact for this commit | baseline source behavior only |
| Initial hardening checkpoint | `4d0807a42ad79f7eb1b44edab48a62bf8813dd9c` | public repository pin remains `3cb9bfd…` | historical checkpoint before provenance, cache, and protocol-contract follow-ups | historical only; not the current candidate boundary |
| Hardened candidate implementation | `689f1fdd47ef2e32986a4fbd10e35196f8c6ab8b` on `hardening/v0.5.0-evidence-preview` before final documentation reconciliation | public repository pin remains `3cb9bfd…`; exact `atomicwrites 0.4.4` supplies durable state replacement semantics | local security, correctness, CI, release, evidence, and cross-runtime contract hardening; no public artifacts | implemented and locally tested, not release-ready |
| Network verification branch | Jeliya `fe870c7c5b63f2bf52b031dd1bc8e27e83183be5` | local Git dependency `3702e8c…` | direct and relay functional pass; manifests retained, unsigned, `certifiable: false` | functional evidence only; cannot certify a release |
| Upstream synchronization remediation | local `iroh-room` `3702e8cbcd5ac1808791124dd6bc44068be5f822` | clean and tested, but unpublished | no immutable public dependency revision | cannot support a Jeliya release claim |

The hardened implementation and network branch intentionally differ. The
network branch exists to test the local upstream remediation; it is not a
publicly reproducible candidate. The retained manifests are
[direct](evidence/v0.5.0/direct.json) and
[relay](evidence/v0.5.0/relay.json).
They also predate the runtime fixes between `fe870c7…` and `689f1fd…`; a fresh
network run is required even after the upstream revision becomes public.

## Published `v0.4.3` artifact set

- `jeliyad-v0.4.3-aarch64-apple-darwin.tar.gz`
- `jeliyad-v0.4.3-x86_64-apple-darwin.tar.gz`
- `jeliyad-v0.4.3-aarch64-unknown-linux-musl.tar.gz`
- `jeliyad-v0.4.3-x86_64-unknown-linux-musl.tar.gz`
- `jeliyad-v0.4.3-x86_64-pc-windows-msvc.zip`
- one `.sha256` sidecar for each archive

No DMG, APK/AAB, iOS application, or separately packaged agent runner is in
`v0.4.3`. No complete `v0.5.0` five-archive set has been built or published.

## Candidate changes are not released capabilities

The candidate adds room-access guards, secret and backup protections,
dependency gates, complete CI job definitions, safer E2E process ownership,
installer integrity checks, complete asset-set visibility controls, and
evidence-aware documentation. It adds no product feature. Local tests and
retained network runs demonstrate implementation progress; they do not alter
the release boundary.

## Publication gate

Before a `v0.5.0` tag can be published, the same public immutable commit must
prove all of the following:

1. a reviewed upstream room-isolation fix is public and exactly pinned;
2. an approved evidence Ed25519 public key was committed before the qualifying
   network run, and both retained manifests have valid detached signatures;
3. direct and relay evidence is certifiable against published revisions;
4. all required hosted CI gates pass twice from clean environments;
5. all five daemon-plus-embedded-UI archives and sidecars exist and verify,
   including Windows behavioral gates;
6. tag, daemon version, changelog, and artifact names all say `v0.5.0`;
7. only the final publishing job can write; it verifies the sealed receipt
   without executing candidate bytes, and only its final step receives the
   token after explicit release authority.

The evidence key is intentionally absent today. The release check therefore
fails closed; an unsigned local PASS cannot open publication. The publication
workflow is implemented locally but has never been used to publish `v0.5.0`.
GitHub does not provide one transaction spanning the Git tag and release
assets. The workflow can guarantee complete asset-set visibility by retaining
a draft until all uploaded bytes verify, but an interrupted cleanup between
the ref and release operations requires operator inspection before retry.

## Evidence provenance

This snapshot records repository and release inventory established on
2026-07-12, the hardened implementation at `689f1fd…`, and the retained direct
and relay manifests produced between 15:55 and 16:39 UTC. Neither tickets,
tokens, identity material, nor public IP addresses are retained. See
[Platform matrix](platform-matrix.md) and
[Known gaps and roadmap](known-gaps-roadmap.md).
