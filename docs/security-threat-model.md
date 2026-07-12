---
type: "Architecture"
title: "Security and threat model"
description: "Trust boundaries, assets, threats, controls, and residual risks for the v0.5.0 Jeliya technical preview."
tags: ["authorization", "privacy", "security", "threat-model"]
timestamp: "2026-07-12T12:21:59Z"
status: "canonical"
implementation_status: "partial"
verification_status: "partial"
release_status: "unreleased"
audience: ["contributors", "maintainers", "operators", "security-reviewers"]
---

# Security and threat model

Jeliya is a local daemon or in-process mobile engine that stores identity keys,
room state, files, and agent events while communicating with untrusted network
peers. The `v0.5.0` goal is a trustworthy preview, not a claim of complete
security. This model records the boundaries that must hold before publication.

## Assets

- identity private keys and persisted engine state;
- room membership, event history, files, pipes, and agent activity;
- invite tickets and per-start daemon bearer tokens;
- local files and workspaces available to an explicitly enabled agent runner;
- release artifacts, checksums, CI credentials, and signing material;
- verification evidence, which must remain useful without containing secrets.

## Trust boundaries

| Boundary | Trusted side | Untrusted side | Required control |
|---|---|---|---|
| Browser or desktop client to loopback daemon | local authorized client holding the per-start token | other local processes, hostile web origins, DNS rebinding | loopback bind, host validation, bearer token, origin restrictions, bounded inputs |
| Engine to P2P room network | local identity and room membership proof | peers, relays, malformed events, malicious room members | signature and room validation, room-scoped synchronization, authorization before every read |
| Shared local event store to public RPC | rooms the local identity has joined | foreign or invite-only rooms present in storage | one centralized room-access guard before materialization or return |
| Flutter app to Android storage and backup | app-private runtime and OS-backed secrets | cloud backup, device transfer, debug extraction, repository checkout | explicit backup exclusions, minimized secret persistence, platform-backed protection where practical |
| Agent runner to host | operator-approved sender, worker, workspace, and room | room messages, generated tasks, subprocess output | explicit opt-in, sender allowlist, isolated workspace/state, no ambient secret logging |
| CI build to public release | reviewed immutable source and verified artifacts | third-party actions/tools, compromised downloads, partial jobs | immutable action pins, verified tool downloads, least privilege, atomic final publication |

## Primary threats and candidate status

| Threat | Impact | Required or current mitigation | Candidate evidence status |
|---|---|---|---|
| Foreign-room data returned through a read RPC | confidentiality breach across rooms | apply one room-access guard to timelines, members, agents, files, pipes, and every future public read before reading or returning data | implementation in working tree; final negative tests pending |
| Foreign-room events served or admitted during synchronization | remote extraction or local-store contamination across rooms | room-scope `get`, `contains`, missing-parent traversal, and `WantEvents`; reject any envelope/parent whose room does not match the session | local upstream remediation exists; immutable upstream revision and Jeliya pin pending |
| Invite-only identity treated as a joined member | read access before membership is accepted | require evidence that the local member has actually joined; keep joined-then-left archive behavior explicit and tested | targeted negative and archive-positive evidence pending final run |
| Android identity copied into cloud or device-transfer backup | long-lived identity disclosure outside the device boundary | candidate stores the engine under `noBackupFilesDir`, disables backup, and excludes private domains from cloud backup and device transfer; Android Keystore wrapping remains a future defense in depth | configuration implemented; validation pending final run |
| Agent identity or local state committed from a checkout | public secret disclosure and identity reuse | candidate defaults to the platform data directory, writes a deny-all `.gitignore` inside every agent data directory, and adds repository ignore plus commit-prevention validation | implementation present; validation pending final run |
| Reachable vulnerable dependency | code execution, data compromise, or denial of service | cargo and npm audit gates; upgrade high/critical findings; time-bound exception with reachability, mitigation, owner, and expiry only when unavoidable | updated lockfiles have no high/critical exception; final clean-environment gate pending |
| Compromised CI action or downloaded build tool | release supply-chain compromise | pin actions to immutable commits; verify Zig and other downloaded tools before execution | candidate workflow verification pending |
| Partial or mismatched release | users receive incomplete, stale, or wrongly versioned binaries | manually promote an exact `main` revision, require two clean CI runs, build and validate all private artifacts first, then create and expose tag plus release only from the write-enabled final job | candidate workflow verification pending |
| Installer extracts modified bytes | local code execution | candidate downloads the exact matching checksum, validates its format and filename, and verifies SHA-256 before extracting; failures stop installation | implementation present; adversarial Unix and Windows tests pending |
| Secrets copied into logs or evidence | credential or identity disclosure | structured sanitization, minimal environment capture, no tickets/tokens/seeds/private keys in artifacts | policy defined; run evidence pending review |

## Authorization invariant

A room identifier supplied by a caller is untrusted. Possession of an ID,
invite, event ID, file ID, or agent ID is not authorization. Before any public
read touches room-derived state, the engine must establish that the local
identity has joined that room. Filtering after materialization is insufficient
because foreign names, counts, or timing can already leak. The same invariant
must be enforced in upstream synchronization so an unauthorized peer cannot
use a known event ID or cross-room parent link as a read primitive.

Room departure currently preserves local archive access for an identity that
previously joined; invite-only state must not grant it. That product decision
is security-sensitive and pinned by negative and positive tests rather than
left implicit.

## Agent boundary

The runner is deliberately a local code-execution surface. It is not enabled by
the daemon or browser automatically. The operator chooses a worker, room,
trigger, allowed senders, data directory, and workspace. The sender allowlist
reduces accidental or unauthorized triggering; it does not sandbox a trusted
sender's requested work or make model-generated commands safe. Run agents with
the least-privileged OS account, isolated state, a minimal environment, and no
production credentials unless the task explicitly requires them.

## Release boundary

No build matrix job may publish directly. A manual promotion identifies the
exact version and current `main` commit before two independent clean CI runs.
Read-only matrix jobs then upload private workflow artifacts. One final job
revalidates the expected five daemon archives, embedded UI, filenames,
checksums, versions, commit, and changelog before it receives write permission.
It refuses an existing tag or release, keeps the release draft until uploaded
bytes compare exactly, and attempts scoped cleanup of only its own draft and
unchanged run-owned tag on failure. GitHub does not provide a single transaction
covering a Git ref and release assets, so an interrupted cleanup remains an
operator-inspection gate before retry. Native application artifacts, DMGs,
APK/AAB files, and app-cask publication are unconditionally outside `v0.5.0`.

## Residual risks

- A malicious authorized room member can read room data already shared with
  that member; removal cannot recall copied data.
- Relay operators observe transport metadata even though room content is
  protected by the underlying protocol.
- Endpoint compromise defeats application-level key protection while the
  identity is in use.
- The Android engine has not yet been verified across different networks.
- Comprehensive accessibility conformance, native signing/notarization, iOS,
  and mobile background availability are not preview security guarantees.

See [`known-gaps-roadmap.md`](known-gaps-roadmap.md) for ownership and release
blocking status, the
[`dependency-risk exception register`](verification-evidence.md#dependency-risk-exception-register)
for current maintenance warnings, and [`SECURITY.md`](../SECURITY.md) for
private reporting.
