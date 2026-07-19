---
type: "Status Report"
title: "Release versus main"
description: "Exact boundary between released v0.6.0 artifacts, their qualified source, and the designated v0.6.1 candidate."
tags: ["artifacts", "main", "release", "versions"]
timestamp: "2026-07-19T21:49:56Z"
status: "canonical"
implementation_status: "not-applicable"
verification_status: "partial"
release_status: "not-applicable"
audience: ["contributors", "maintainers", "operators", "release-engineers"]
---

# Release versus main

Git branches, test revisions, tags, and release assets answer different
questions. `v0.6.0` shipped on 2026-07-16 as a daemon-only prerelease at
`2283a441...`. Jeliya `a1af1cdc974bc307317779afa0765c3988cb871f`
is the designated v0.6.1 source candidate; it is not network-qualified or a
release.

## Current boundary

| Layer | Exact revision | Dependency state | Artifact/evidence state | Claim allowed |
|---|---|---|---|---|
| Latest public release | lightweight tag `v0.6.0` at `2283a441220031485a7a212dc585772231d0f428` (prerelease) | release source pins published `iroh-rooms` `71fbb5007bef4ce83631c94762ec68c2beef3d79` (`v0.1.0-rc.3`) | five published daemon archives and five checksum sidecars; signed certifying direct (`1ca39cfa`) and relay (`cf28bc63`) manifests bind the qualified source commit | behavior in those archives is released; the tag adds only the reviewed evidence documentation to the qualified source |
| Current `v0.6.1` source candidate | `a1af1cdc974bc307317779afa0765c3988cb871f` | pins untagged public Iroh Rooms `a5d98b70d717f35d3ce60953a88e12e646f2e871`, the first merge carrying `kortiene/iroh-room#121` and `kortiene/iroh-room#119` fixes plus the `kortiene/iroh-room#126` follow-ups | all eight hosted jobs passed on public `main` run `29704754961`; fresh signed direct/relay evidence pending | corrective source only; designated but not network-qualified or published |
| Released `v0.6.0` qualification source | `55024a46b3e112796ba2acf1dc408dab26dbba2e` | pins `v0.1.0-rc.3` at `71fbb5007bef4ce83631c94762ec68c2beef3d79` | signed certifying direct (`1ca39cfa`) and relay (`cf28bc63`) manifests bind this exact pair | evidence authorized `v0.6.0`; it does not transfer to v0.6.1 |
| Superseded `v0.5.0` network-qualified commit | `c5f740e67d043a1153cf285691e3bc5b2b9a7203` | pins `d0ceb0b…` | both `v0.5.0` certifying schema 2 runs bind this commit | the certified evidence speaks for that revision pair only; it does not transfer to the rc.3 pin |
| Audited baseline | `1285b42037a3713840955fa518f2b81b19f2929f` | pins vulnerable `iroh-rooms` `3cb9bfd…` | no artifact for this commit | baseline source behavior only |
| Initial hardening checkpoint | `4d0807a42ad79f7eb1b44edab48a62bf8813dd9c` | pinned `3cb9bfd…` at that checkpoint | historical checkpoint before provenance, cache, and protocol-contract follow-ups | historical only |
| Pre-certification network snapshot | `0f6769a68d783cf6a5feba0e2db6111a212affa1` on `hardening/v0.5.0-evidence-preview` | pinned then-unsafe `3cb9bfd…` | schema 2 direct 36/36 functional pass ([preview manifest](evidence/v0.5.0/preview-direct-schema2.json), unsigned); its relay-only build failed closed for lack of the seam | historical functional evidence only |
| Historical local-remediation network snapshot | Jeliya `fe870c7c5b63f2bf52b031dd1bc8e27e83183be5` | local Git dependency `3702e8c…` | schema 1 direct and relay functional pass; manifests retained unsigned as `historical-schema1-{direct,relay}.json` | historical functional evidence only |

The certifying [direct](evidence/v0.6.0/direct.json) and
[relay](evidence/v0.6.0/relay.json) schema 2 manifests bind the
network-qualified commit `55024a4…` and published pin `71fbb500…`, carry
detached Ed25519 signatures, and set `certifiable: true` — they qualify that
released `v0.6.0` source. They do not qualify `a1af1cdc…` + `a5d98b70…`.
The `v0.5.0` manifests
([direct](evidence/v0.5.0/direct.json), [relay](evidence/v0.5.0/relay.json))
bind `c5f740e…` + `d0ceb0b…` and authorized that prerelease; they do not
transfer to another pin. Neither generation certifies room-scoped
synchronization isolation — every manifest sets
`synchronization_isolation_claimed: false`, so that control rests on the
upstream suite at the pinned revision, not on network evidence.

## Published `v0.6.0` artifact set

- `jeliyad-v0.6.0-aarch64-apple-darwin.tar.gz`
- `jeliyad-v0.6.0-x86_64-apple-darwin.tar.gz`
- `jeliyad-v0.6.0-aarch64-unknown-linux-musl.tar.gz`
- `jeliyad-v0.6.0-x86_64-unknown-linux-musl.tar.gz`
- `jeliyad-v0.6.0-x86_64-pc-windows-msvc.zip`
- one `.sha256` sidecar for each archive

No DMG, Linux native-app tarball, APK/AAB, iOS application, or separately
packaged agent runner is in `v0.6.0`; it is a daemon-plus-embedded-UI
prerelease only.

## Candidate changes are not released capabilities

The v0.6.1 candidate `a1af1cdc...` pins `iroh-rooms` to untagged
`a5d98b70...`.
Alongside the rc.3 join capability, bounded membership sync, and gap healing,
this adds provisional-peer fanout/handshake gating, connection-generation
teardown guards, and bounded store-insert recovery with durable critical
degradation reporting. It also adds a source-supported Linux
Flutter app with its packaging gate and `linux-flutter` CI job. The Linux app
is an unsigned, unpublished developer package; it adds no released feature.
Local tests and upstream regressions demonstrate implementation progress;
they do not alter the release boundary — `v0.6.0` behavior is exactly what
its archives contain.

## Publication gate

`v0.6.0` met this gate and published. Before the v0.6.1 release tag can be
published, the same public immutable commit must prove all of the following:

1. the reviewed upstream pin (`a5d98b70…` or a reviewed tagged successor
   carrying the same fixes) is public and exactly pinned;
2. the approved evidence Ed25519 public key predates the qualifying network
   runs, and both retained manifests have valid detached signatures;
3. direct and relay evidence is certifiable against the candidate's published
   revisions (the `v0.6.0` evidence binds `55024a4` + `71fbb500` and does not
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

This snapshot records the released `v0.6.0` boundary (tag at `2283a441…`,
certifying signed direct/relay manifests bound to `55024a4…` + `71fbb500…`),
the designated v0.6.1 candidate and its untagged dependency, the superseded v0.5.0 pair,
and the retained historical
manifests (the unsigned schema 2 preview run at `0f6769a…` and the schema 1
local-remediation runs). Neither tickets, tokens, identity material, nor
public IP addresses are retained. See
[Platform matrix](platform-matrix.md) and
[Known gaps and roadmap](known-gaps-roadmap.md).
