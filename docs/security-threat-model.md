---
type: "Architecture"
title: "Security and threat model"
description: "Trust boundaries, assets, threats, controls, and residual risks for the v0.5.0 Jeliya technical preview."
tags: ["authorization", "privacy", "security", "threat-model"]
timestamp: "2026-07-12T22:00:46Z"
status: "canonical"
implementation_status: "partial"
verification_status: "partial"
release_status: "unreleased"
audience: ["contributors", "maintainers", "operators", "security-reviewers"]
---

# Security and threat model

Jeliya is a local daemon or in-process mobile engine that stores identity keys,
room state, files, and agent events while communicating with untrusted network
peers. The `v0.5.0` target is a trustworthy technical preview, not a claim of
complete security. The hardening implementation and functional verification
are substantially complete, but the release remains blocked because the
room-scoped upstream remediation and network-test source are unpublished and
the retained network evidence is unsigned.

## Candidate boundary

Security conclusions must name the source being evaluated:

| Surface | Revision | Security meaning |
|---|---|---|
| Public Jeliya dependency | Iroh Rooms `3cb9bfd1e43eb755c967315c37b6d4fd1c2bf020` | still permits the synchronization behavior under remediation; unsafe for `v0.5.0` publication |
| Hardening implementation | Jeliya `689f1fdd47ef2e32986a4fbd10e35196f8c6ab8b` before final documentation reconciliation | implements local RPC, durable provenance, secret, CI, evidence, and release controls |
| Functional network verification | Jeliya `fe870c7c5b63f2bf52b031dd1bc8e27e83183be5` plus local Iroh Rooms `3702e8cbcd5ac1808791124dd6bc44068be5f822` | direct and forced-relay functional checks pass, but both revisions are local/unpublished and cannot certify a release |

No result from the local verification checkout should be projected onto the
current public dependency pin. Publication, immutable repinning, and a fresh
signed evidence run are security requirements, not release administration.
The retained network checkout also predates the provenance, read-cache, and
cross-runtime contract changes in the hardened implementation, so it cannot
qualify those changes independently of the unpublished dependency blocker.

## Assets

- identity private keys and persisted engine state;
- room membership, event history, files, pipes, and agent activity;
- invite tickets and per-start daemon bearer tokens;
- local files and workspaces available to an explicitly enabled agent runner;
- release artifacts, checksums, CI credentials, and evidence-signing material;
- verification evidence, which must be attributable without containing
  secrets.

## Trust boundaries

| Boundary | Trusted side | Untrusted side | Required control |
|---|---|---|---|
| Browser or desktop client to loopback daemon | local authorized client holding the per-start token | other local processes, hostile web origins, DNS rebinding | loopback bind, host validation, bearer token, origin restrictions, bounded inputs |
| Engine to P2P room network | local identity and accepted room membership | peers, relays, malformed events, malicious room members | signatures, room validation, room-scoped synchronization, authorization before state access |
| Shared event store to public RPC | rooms the local identity has accepted | foreign or invite-only rooms that exist in storage | centralized accepted-room guard before fold, materialization, or return; aggregate filtering |
| Flutter app to Android storage and backup | app-private `noBackupFilesDir` state | cloud backup, device transfer, debug extraction, repository checkout | disabled backup plus explicit cloud/device-transfer exclusions and fail-closed migration |
| Agent runner to host | operator-approved sender, worker, workspace, and room | room messages, generated tasks, subprocess output | explicit opt-in, sender allowlist, least-privilege process, isolated state/workspace, no ambient secret logging |
| Operator environment to certifying source build | exact public commit, pinned lockfiles, explicitly allowed network and CA settings, independently verified complete Zig archive | checkout-local Git attributes/configuration, ambient build controls or credentials, path substitution, Python `ziglang`, unbound build tools | isolated bare Git archive; run-owned HOME/Cargo/npm/Git/temp; controlled path; exact Node/npm/Cargo/cargo-zigbuild/Zig bindings; verified Zig installation root and library directory |
| CI build to public release | reviewed immutable source and complete verified artifacts | third-party actions/tools, compromised downloads, partial jobs, candidate binary attempting to alter release inputs | immutable action pins, verified tool downloads, execution-free validation and sealing, isolated read-only smoke, receipt verification without candidate execution, token only in final step |
| Retained evidence to release decision | exact sanitized manifest signed by the approved evidence key | edited, fabricated, stale, or secret-bearing evidence | pinned public SPKI, detached Ed25519 signature, exact source/dependency checks, ancestry restriction |

Android currently relies on app-private no-backup storage and explicit backup
rules. It does **not** wrap the identity with Android Keystore. Keystore-backed
wrapping remains defense in depth; documentation must not describe it as an
implemented control.

## Primary threats and current status

| Threat | Impact | Control | Evidence and remaining risk |
|---|---|---|---|
| Foreign-room data returned through a public RPC | cross-room confidentiality breach | one accepted-room preflight guard before any room-derived read or fold; `agents.fleet` and other aggregates enumerate/filter accepted rooms | local regressions pass; direct and relay runs deny 17 room-scoped RPCs, local-file access, and aggregate foreign-room/agent projections on the unpublished verification revisions |
| Foreign-room events served or admitted during synchronization | remote extraction or local-store contamination | room-scope `get`, `contains`, `WantEvents`, missing-parent traversal, and administrative tips; reject foreign envelopes and parents | malicious local upstream tests pass at `3702e8cb...`; public `3cb9bfd...` remains unsafe and is a release blocker |
| Invite possession treated as accepted membership | pre-join data exposure | accepted-room index is authoritative; invite-only/never-joined rooms fail closed; joined-then-left archive behavior is explicit | negative never-joined cases and positive archive behavior pass locally |
| Android identity copied through backup or device transfer | long-lived identity disclosure outside the device | `allowBackup=false`, explicit backup/data-extraction exclusions, `noBackupFilesDir`, fail-closed migration | repository validation passes; no Keystore protection and no claim against a rooted or compromised device |
| Agent identity or state committed from a checkout | public secret disclosure and identity reuse | platform data directory outside the checkout, per-directory deny-all `.gitignore`, repository ignore rules, tracked-secret gate | six secret-storage tests plus repository validation pass locally |
| Reachable vulnerable dependency | code execution, compromise, or denial of service | automated cargo/npm audits; high/critical findings block; explicit owned/expiring exception only when unavoidable | zero cargo/npm vulnerabilities; three maintenance warnings and one yanked version expire 2026-09-30 |
| Compromised action or downloaded build tool | release supply-chain compromise | third-party Actions pinned to immutable revisions; Zig and Gradle distributions verified before execution; certifying network builds use the official complete Zig archive and exact tool bindings; least-privilege jobs | workflow and local contract tests pass; only the complete Zig archive is independently verified by schema 2, while other recorded tool digests are execution identities; no hosted double run has occurred |
| Partial, mismatched, or post-validation-modified release | incomplete, stale, mislabeled, or candidate-mutated binaries | validate and seal all five private archives in a no-execution job; smoke the immutable artifact separately; verify the receipt without execution before tag/release creation; expose the write token only to the final step | workflow and receipt negative tests pass locally, but no complete five-archive `v0.5.0` set or publication rehearsal exists |
| Installer extracts modified bytes | local code execution | fetch the matching published checksum, validate filename/format, verify SHA-256, then extract | Unix behavior passes; Windows behavioral and simulated-reparse gates are configured but have not executed on a hosted Windows runner |
| Forged or edited verification record | false release confidence | retained exact manifest, canonical public key, detached Ed25519 signature, source/publication/ancestry checks | manifests are retained but unsigned; the approved public SPKI is absent, so the release gate fails closed |
| Secrets copied into logs or evidence | credential or identity disclosure | transient logs confined to run-owned data directories, no address retention, and digest-only retained summaries | successful cleanup removed transient logs in both retained runs; any cleanup failure makes a run fail qualification. Manifests keep only line/byte counts and stream SHA-256 digests and contain no tickets, tokens, seeds, private keys, excerpts, or IP addresses |

## Authorization invariant

A caller-supplied room, invite, event, file, pipe, or agent identifier is
untrusted. Identifier possession is not authorization. Before a public RPC
touches room-derived state, the engine must establish that the local identity
accepted membership in that room. Filtering only after materialization is too
late because names, counts, timing, or errors may already disclose foreign
state.

The accepted-room index is therefore the first guard. A snapshot-level check
is a second defense, not a substitute. Aggregate surfaces must begin with
accepted rooms rather than enumerate the shared store and remove foreign rows
afterward. A rejected request must not mutate room-open state or create a
side-channel through partial work.

Room departure currently preserves access to the local archive for an identity
that previously joined. An invite that has not been accepted must not grant the
same access. Negative never-joined cases and the joined-then-left positive case
pin this security-sensitive product decision.

## Synchronization invariant

RPC guards prevent disclosure from the local API, but they do not make a
foreign event safe to store or serve. Every synchronization session and event
lookup must remain scoped to its room. Known event IDs, causal parents,
administrative tips, and missing-event requests must never become cross-room
read primitives.

The local upstream remediation enforces that invariant and passes malicious
`WantEvents`, foreign-parent, and administrative-tip tests. Because the public
Jeliya lockfile does not yet resolve that code, upstream publication and Jeliya
repinning are mandatory before release qualification.

## Secret-storage boundaries

Android engine state lives under the application's no-backup directory, with
legacy and current backup/data-extraction configurations excluding all relevant
domains. Migration fails closed instead of silently reusing state from a
backup-eligible location. This reduces accidental cloud and device-transfer
copies; it does not protect an unlocked compromised endpoint while the identity
is usable.

The agent runner defaults to the OS platform data directory rather than the
repository. Explicit state directories receive a deny-all Git marker, and
unsafe existing markers fail closed. Repository-level ignore and tracked-file
validation provide independent defense against accidental commits. Operators
must still avoid placing production credentials in the agent environment or
workspace.

## Agent boundary

The runner is a deliberate local code-execution surface. The daemon and browser
do not enable it automatically. The operator selects a worker, room, trigger,
allowed senders, data directory, and workspace. The sender allowlist limits who
can trigger work; it does not sandbox an allowed sender's task or make
model-generated commands safe. Run agents with the least-privileged OS account,
isolated state, a minimal environment, and no production credentials unless the
task explicitly requires them.

## Network evidence boundary

The recorded three-peer direct run demonstrates direct connectivity across the
observed operator/demo topology. The compile-time relay-only run demonstrates
that messages, files, pipes, reconnect, and isolation remain functional when
all tested paths are forced through the relay. It proves relay fallback behavior
under a deliberate application-level constraint; it does not prove a natural
hole-punch failure by ordinary direct-capable binaries.

Both runs used unpublished Jeliya and Iroh Rooms revisions and are unsigned.
They also use historical schema 1, before the isolated source-build and
complete Zig-installation controls in schema 2. They are functional
investigation evidence only and cannot become certifying records retroactively.
See
[`verification-evidence.md`](verification-evidence.md#retained-three-peer-network-evidence)
for the exact revisions, environments, assertions, hashes, and cleanup record.

## Release boundary

Build jobs must remain read-only. A manual promotion binds an exact version and
public default-branch commit, then requires two independent complete CI runs.
An execution-free read-only job validates the five daemon archives, embedded
UI, filenames, checksums, versions, commit, changelog, signed network evidence,
and source ancestry, then seals exact bytes and provenance in a receipt. A
separate read-only job executes the immutable smoke artifact. The sole writer
fetches the public verification source without credentials and verifies the
receipt without executing candidate bytes; its GitHub token exists only in the
final publishing step.

The finalizer rejects an existing tag or release, keeps the release draft until
uploaded bytes compare exactly, and attempts scoped cleanup of only its own
draft and unchanged run-owned tag on failure. GitHub does not provide a single
transaction across a Git ref and release assets, so any interrupted cleanup
requires operator inspection before retry. Native application artifacts, DMGs,
APK/AAB files, and app-cask publication are outside `v0.5.0` unless their
platform gates are separately satisfied.

No release action may proceed while the evidence public key is absent, the
network manifests are unsigned, the source/dependency revisions are
unpublished, the two hosted CI passes are missing, or the complete artifact set
has not been verified.

## Residual risks after the release blockers are cleared

- An authorized room member can copy data already shared with that member;
  removal cannot recall it.
- Relay operators can observe transport metadata even though the room content
  is protected by the underlying protocol.
- Endpoint compromise defeats application-level key protection while an
  identity is in use.
- Android has not been verified with a remote peer across different networks,
  and its identity is not Keystore-wrapped.
- Windows installer reparse-point behavior has not been exercised in the local
  evidence window.
- Comprehensive accessibility conformance, native signing/notarization, iOS,
  and mobile background availability are not preview security guarantees.

See [`known-gaps-roadmap.md`](known-gaps-roadmap.md) for ownership and release
blocking status, the
[`dependency-risk exception register`](verification-evidence.md#dependency-risk-exception-register)
for current maintenance warnings, and [`SECURITY.md`](../SECURITY.md) for
private reporting.
