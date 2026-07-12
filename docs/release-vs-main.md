---
type: "Status Report"
title: "Release versus main"
description: "Exact boundary between the latest published Jeliya artifacts, the audited main baseline, and the v0.5.0 candidate."
tags: ["artifacts", "main", "release", "versions"]
timestamp: "2026-07-12T12:21:59Z"
status: "canonical"
implementation_status: "not-applicable"
verification_status: "verified"
release_status: "not-applicable"
audience: ["contributors", "maintainers", "operators", "release-engineers"]
---

# Release versus main

Git branches, tags, and releases answer different questions. This page prevents
implemented-on-main behavior from being described as shipped.

## Current boundary

| Layer | Exact revision | Version state | Artifact state | Claim allowed |
|---|---|---|---|---|
| Latest public release | tag `v0.4.3` at `9d62c3cd98c7f21d9683815c28278b6ac8c0b97f` | daemon `0.4.3` | published 2026-07-07; five daemon archives and five checksum sidecars | only behavior reachable in those archives is released |
| Audited `main` baseline | `1285b42037a3713840955fa518f2b81b19f2929f` | Cargo manifests still report daemon `0.4.3` | no artifact corresponds to this commit | implemented on main, not released |
| `v0.5.0` hardening candidate | branch `hardening/v0.5.0-evidence-preview`; uncommitted working tree at this snapshot | core and daemon manifests plus changelog name `0.5.0`; built-artifact consistency is not yet established | none | work in progress, not verified, not released |
| Upstream synchronization remediation | local branch in the `iroh-room` checkout | not published or pinned by Jeliya | none | cannot be used as Jeliya release evidence |

The baseline pins `iroh-rooms` revision
`3cb9bfd1e43eb755c967315c37b6d4fd1c2bf020`. The local remediation must receive
an immutable upstream revision and Jeliya must pin that exact revision before
candidate verification can be reproducible.

## Published `v0.4.3` artifact set

- `jeliyad-v0.4.3-aarch64-apple-darwin.tar.gz`
- `jeliyad-v0.4.3-x86_64-apple-darwin.tar.gz`
- `jeliyad-v0.4.3-aarch64-unknown-linux-musl.tar.gz`
- `jeliyad-v0.4.3-x86_64-unknown-linux-musl.tar.gz`
- `jeliyad-v0.4.3-x86_64-pc-windows-msvc.zip`
- one `.sha256` sidecar for each archive

The GitHub release is neither a draft nor a prerelease. No macOS Flutter DMG,
Android APK/AAB, iOS application, or separately packaged agent runner is part
of that release.

## Material changes after `v0.4.3`

The baseline contains substantial unreleased work, including the in-process
FFI engine, Android application path, mobile UI, expanded localization and
accessibility tests, agent orchestration updates, and packaging work. These
changes are visible in source but absent from `v0.4.3` binaries. The candidate
adds security, dependency, CI, release, and evidence hardening without adding
new product features.

## Version-consistency gate

Before a `v0.5.0` tag can be published, one final job must prove all of the
following against the same commit:

1. the tag is exactly `v0.5.0`;
2. `jeliyad --version` reports `0.5.0` for every built target;
3. the changelog contains the matching `0.5.0` release entry;
4. every public archive and checksum filename contains `v0.5.0` and the exact
   target triple;
5. the complete expected artifact set exists and verifies before any asset is
   published;
6. only the final publishing job has repository write permission.

Until all six assertions pass, documentation must refer to a candidate, not a
release.

## Evidence source

The release tag, publication timestamp, draft/prerelease flags, and asset names
were read from the GitHub Releases API on 2026-07-12 UTC. Repository revisions,
manifest versions, and dependency pins were read from the local Git history at
the baseline above. No mutable `latest` URL is used as revision evidence.
