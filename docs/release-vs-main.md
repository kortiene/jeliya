---
type: "Status Report"
title: "Release versus main"
description: "Exact boundary between the latest published Jeliya artifacts, the audited baseline, and the v0.5.0 candidate."
tags: ["artifacts", "main", "release", "versions"]
timestamp: "2026-07-19T15:15:00Z"
status: "canonical"
implementation_status: "not-applicable"
verification_status: "partial"
release_status: "not-applicable"
audience: ["contributors", "maintainers", "operators", "release-engineers"]
---

# Release versus main

Git branches, test revisions, tags, and release assets answer different
questions. `v0.5.0` shipped on 2026-07-14 as a daemon-only prerelease. The
post-release source line and the current branch candidate must not be described
as released.

## Current boundary

| Layer | Exact revision | Dependency state | Artifact/evidence state | Claim allowed |
|---|---|---|---|---|
| Latest public release | tag `v0.5.0` at `045d85cb1d066f16d564b6051363b9328063ee01` (prerelease) | pins published `iroh-rooms` `d0ceb0b…` (rc.2-era remediation) | five published daemon archives and five checksum sidecars; signed certifying direct (`3b86ac67`) and relay (`a3c76859`) manifests | behavior in those archives is released; known limitation: joins from invites minted after non-admin chat fail at this pin |
| Current `v0.6.0` source candidate | `105744b6c27633e5ccc576d86f1a15e3fe443b94` | pins untagged public Iroh Rooms `a5d98b70d717f35d3ce60953a88e12e646f2e871`, the first merge carrying `kortiene/iroh-room#121` and `kortiene/iroh-room#119` fixes plus the `kortiene/iroh-room#126` follow-ups | exact-revision upstream, workspace, and 67-assertion loopback suites pass; fresh signed direct/relay evidence pending | locally qualified; not network-qualified or published |
| Prior `v0.6.0` network-qualified snapshot | `55024a46b3e112796ba2acf1dc408dab26dbba2e` | pins `v0.1.0-rc.3` at `71fbb5007bef4ce83631c94762ec68c2beef3d79` | signed certifying direct (`1ca39cfa`) and relay (`cf28bc63`) manifests bind this exact pair | evidence remains valid for the snapshot but does not transfer to the current candidate |
| Superseded `v0.5.0` network-qualified commit | `c5f740e67d043a1153cf285691e3bc5b2b9a7203` | pins `d0ceb0b…` | both `v0.5.0` certifying schema 2 runs bind this commit | the certified evidence speaks for that revision pair only; it does not transfer to the rc.3 pin |
| Audited baseline | `1285b42037a3713840955fa518f2b81b19f2929f` | pins vulnerable `iroh-rooms` `3cb9bfd…` | no artifact for this commit | baseline source behavior only |
| Initial hardening checkpoint | `4d0807a42ad79f7eb1b44edab48a62bf8813dd9c` | pinned `3cb9bfd…` at that checkpoint | historical checkpoint before provenance, cache, and protocol-contract follow-ups | historical only |
| Pre-certification network snapshot | `0f6769a68d783cf6a5feba0e2db6111a212affa1` on `hardening/v0.5.0-evidence-preview` | pinned then-unsafe `3cb9bfd…` | schema 2 direct 36/36 functional pass ([preview manifest](evidence/v0.5.0/preview-direct-schema2.json), unsigned); its relay-only build failed closed for lack of the seam | historical functional evidence only |
| Historical local-remediation network snapshot | Jeliya `fe870c7c5b63f2bf52b031dd1bc8e27e83183be5` | local Git dependency `3702e8c…` | schema 1 direct and relay functional pass; manifests retained unsigned as `historical-schema1-{direct,relay}.json` | historical functional evidence only |

The certifying [direct](evidence/v0.6.0/direct.json) and
[relay](evidence/v0.6.0/relay.json) schema 2 manifests bind the
network-qualified commit `55024a4…` and published pin `71fbb500…`, carry
detached Ed25519 signatures, and set `certifiable: true` — they qualify that
prior `v0.6.0` snapshot. They do not qualify the current `105744b…` +
`a5d98b70…` source candidate. The `v0.5.0` manifests
([direct](evidence/v0.5.0/direct.json), [relay](evidence/v0.5.0/relay.json))
bind `c5f740e…` + `d0ceb0b…` and authorized that prerelease; they do not
transfer to another pin. Neither generation certifies room-scoped
synchronization isolation — every manifest sets
`synchronization_isolation_claimed: false`, so that control rests on the
upstream suite at the pinned revision, not on network evidence.

## Published `v0.5.0` artifact set

- `jeliyad-v0.5.0-aarch64-apple-darwin.tar.gz`
- `jeliyad-v0.5.0-x86_64-apple-darwin.tar.gz`
- `jeliyad-v0.5.0-aarch64-unknown-linux-musl.tar.gz`
- `jeliyad-v0.5.0-x86_64-unknown-linux-musl.tar.gz`
- `jeliyad-v0.5.0-x86_64-pc-windows-msvc.zip`
- one `.sha256` sidecar for each archive

No DMG, Linux native-app tarball, APK/AAB, iOS application, or separately
packaged agent runner is in `v0.5.0`; it is a daemon-plus-embedded-UI
prerelease only.

## Candidate changes are not released capabilities

The post-release candidate repins `iroh-rooms` to untagged `a5d98b70...`.
Alongside the rc.3 join capability, bounded membership sync, and gap healing,
this adds provisional-peer fanout/handshake gating, connection-generation
teardown guards, and bounded store-insert recovery with durable critical
degradation reporting. It also adds a source-supported Linux
Flutter app with its packaging gate and `linux-flutter` CI job. The Linux app
is an unsigned, unpublished developer package; it adds no released feature.
Local tests and upstream regressions demonstrate implementation progress;
they do not alter the release boundary — `v0.5.0` behavior is exactly what
its archives contain, including its known join-after-chat limitation.

## Publication gate

`v0.5.0` met this gate and published. Before the next release tag can be
published, the same public immutable commit must prove all of the following:

1. the reviewed upstream pin (`a5d98b70…` or a reviewed tagged successor
   carrying the same fixes) is public and exactly pinned;
2. the approved evidence Ed25519 public key predates the qualifying network
   runs, and both retained manifests have valid detached signatures;
3. direct and relay evidence is certifiable against the candidate's published
   revisions (the `v0.5.0` evidence binds `c5f740e` + `d0ceb0b` and does not
   transfer);
4. all required hosted CI gates — now including `linux-flutter` — pass twice
   from clean environments;
5. the complete archive-and-sidecar set exists and verifies, including
   Windows behavioral gates;
6. tag, daemon version, changelog, and artifact names agree;
7. only the final publishing job can write; it verifies the sealed receipt
   without executing candidate bytes, and only its final step receives the
   token after explicit release authority.

GitHub does not provide one transaction spanning the Git tag and release
assets. The workflow guarantees complete asset-set visibility by retaining
a draft until all uploaded bytes verify, but an interrupted cleanup between
the ref and release operations requires operator inspection before retry.

## Evidence provenance

This snapshot records the released `v0.5.0` boundary (tag at `045d85c…`,
certifying signed direct/relay manifests bound to `c5f740e…` + `d0ceb0b…`),
the post-release untagged dependency candidate, the prior signed rc.3 snapshot,
and the retained historical
manifests (the unsigned schema 2 preview run at `0f6769a…` and the schema 1
local-remediation runs). Neither tickets, tokens, identity material, nor
public IP addresses are retained. See
[Platform matrix](platform-matrix.md) and
[Known gaps and roadmap](known-gaps-roadmap.md).
