# Agent marketplace architecture

> **Status:** proposed design. Nothing in this document is implemented unless
> it is explicitly marked **Verified current behavior**. It is not a security
> guarantee for a marketplace listing or a hosted provider.

## Executive recommendation

Ship a **hosted-agent-only MVP**, with the marketplace catalog and installation
orchestration in `jeliyad`, not in either client. A hosted agent remains an
ordinary cryptographic P2P room participant. The public catalog is never a
source of room state and does not receive room content merely because a user
opens Marketplace.

This is the shortest path that preserves Jeliya's honesty model:

- Jeliya already has identity-bound, single-use `agent` invitations; signed
  membership; peer connectivity; and signed `agent_status` evidence.
- A provider response proves only that provisioning was accepted. Installation
  becomes ready only when Jeliya observes the exact provisioned identity as an
  active room member plus connectivity or a real status event.
- The existing Node runner and fleet script are not a package manager or a
  sandbox. Running marketplace packages locally now would weaken the deliberate
  browser and native execution boundary.

Two prerequisites block a truthful hosted release:

1. An owner must be able to remove an agent through a signed `member.removed`
   event. The SDK supports it, but Jeliya exposes no owner removal write path
   and its materializer currently drops the event.
2. The launch surface must be explicit: web/default `jeliyad` has real
   networking, Flutter macOS currently starts the sidecar in loopback mode, and
   Android runs the in-process Rust engine via `FfiClient` with real
   networking enabled (`loopback: false`) — host-conformance-verified against
   the golden corpus, but not yet proven on devices. iOS does not run at all
   yet: no platform scaffold or engine build exists (the `FfiClient` code path
   is shared, but the staticlib wiring is a tracked follow-up). The MVP is web
   first unless native real networking is completed and proven.

Keep the current manual **Bring your own agent** (BYOA) flow. Marketplace
agents add a managed installation record; BYOA agents remain normal, unmanaged
room members.

## Scope and non-goals

The MVP lets an owner browse a trusted catalog, inspect a manifest, select one
or more owned rooms, grant the actual role access, provision a hosted agent,
observe its join truthfully, and later remove it. It does not make a hosted
provider trustworthy, restrict the agent to selected message/file categories,
or run a package on the user's machine.

The MVP does not include local package installation, arbitrary browser process
spawning, mobile production transport, marketplace metadata in room events,
automatic upgrades, marketplace-triggered removal, or claims that a provider's
runtime is the source/version it advertises.

## Current-state architecture

### Verified current behavior

Jeliya reconstructs room state from signed Iroh Rooms events. The SDK is
intentionally isolated to [`jeliya-core`](../crates/jeliya-core); the
[`RoomSupervisor`](../crates/jeliya-core/src/supervisor.rs) validates, folds,
persists, and publishes events. [`jeliyad`](../crates/jeliyad/src/rpc.rs)
exposes a local WebSocket API to the clients. The protocol contract is
[`docs/PROTOCOL.md`](PROTOCOL.md).

The top-level Agents navigation opens a cross-room fleet dashboard, while a
room's right panel has a separate Agents tab. React implements both in
[`ui/src/components/FleetDashboard.tsx`](../ui/src/components/FleetDashboard.tsx)
and [`ui/src/components/RightPanel.tsx`](../ui/src/components/RightPanel.tsx).
Flutter implements equivalents in
[`app/lib/src/screens/fleet_dashboard.dart`](../app/lib/src/screens/fleet_dashboard.dart)
and [`app/lib/src/screens/right_panel.dart`](../app/lib/src/screens/right_panel.dart).

The current Add Agent modal is an intentional human-controlled boundary:

```text
Room owner                         Jeliya client / daemon                Agent host
──────────                         ─────────────────────                 ──────────
open Agents → Add Agent
paste agent identity id ───────► room.open(room)
                                  invite.create(role=agent)
                                  render ticket + peer address + command
                                                                     human copies/runs
                                                               ───► jeliya-agent.mjs
                                                                       room.join(ticket)
                                                                       room.open(room)
                                                                       status.post(online)
                                                                            │
                                                     signed room events ◄────┘
                                                           + peer evidence
```

The browser and native clients do not start the runner. React does this in
[`FleetDashboard.tsx`](../ui/src/components/FleetDashboard.tsx); Flutter does
the same in [`add_agent.dart`](../app/lib/src/screens/modals/add_agent.dart).
The generic Invite UI is a second manual route and must remain available.

An identity has separate Ed25519 identity and device keys
([`identity.rs`](../crates/jeliya-core/src/identity.rs)). `invite.create` is
owner-only, uses random capability material, creates an identity-bound invite,
and returns the secret ticket. `room.join` checks that the local identity is
the ticket invitee before joining and checks ticket expiry when supplied. The
resulting `member.joined` is signed and folds into membership. These are the
primitives a hosted installer reuses.

The runner ([`scripts/jeliya-agent.mjs`](../scripts/jeliya-agent.mjs)) creates
or reuses one identity per data directory, joins one room per process, and
posts real `agent_status` events. Its default `echo` worker is deterministic;
the `claude` worker is explicit opt-in and spawns a local subprocess. The
fleet script ([`scripts/jeliya-fleet.mjs`](../scripts/jeliya-fleet.mjs)) can
restart runners, but stores tickets in JSON and has no signed packages,
sandbox, resource isolation, rollback, or secure secret broker.

Fleet liveness is derived, not asserted: peer connectivity for a bound device
plus the newest signed status event determine `online-idle`, `working`,
`offline`, or `stale` ([`fleet.rs`](../crates/jeliya-core/src/fleet.rs)). A
disconnected agent with a last `working` status is stale, not working.

### Important limits of the current model

`agent` is a room membership role, not a granular capability system. An active
agent can receive the room timeline and roster and, subject to normal room
membership checks, can author events and fetch available files. The current
protocol cannot enforce "new messages only", "no history", "no files", or
"read-only agent". A consent screen must not offer such checkboxes.

`agents.fleet` is a liveness projection, not installation truth. It identifies
role-`agent` entries without filtering all membership states, so an invited or
removed identity can appear offline. A marketplace needs its own durable local
installation record and must verify active membership separately.

Jeliya currently exposes voluntary `room.leave`, not owner removal. The pinned
SDK contains `member.removed`, but Jeliya does not author it and
[`materializer.rs`](../crates/jeliya-core/src/materializer.rs) drops it. This
must be fixed before the UI promises Remove or Revoke.

The web client uses real local daemon networking by default. Flutter macOS
currently passes `loopback: true` to its sidecar; Android runs the same
protocol over the in-process engine (`FfiClient`, `loopback: false`) —
host-conformance-verified, not yet proven on devices — and iOS has no
platform scaffold or engine build yet (nothing runs there today). A catalog
can be rendered everywhere the app runs, but a cross-device hosted join
cannot be called supported on those clients yet.

### Reuse and required change

| Reuse | Add or change |
|---|---|
| Agents navigation, fleet cards, room Agents tab, modal/error patterns | Marketplace and Installed views, detail page, room multi-select, persistent installation rows |
| `room.open`, `invite.create`, `room.join`, signed membership, `agents.fleet`, `agent.history` | Orchestration RPCs, durable installation store, exact identity verification, reconciliation |
| Peer/status-based liveness | Explicit evidence-backed installation states; never infer state from a provider success response |
| BYOA runner, safe echo E2E, client mocks | Hosted mock provider/catalog; marketplace test fixtures and deterministic state transitions |
| Flutter ARB localization and redaction utilities | Marketplace EN/FR copy; web localization strategy; structural redaction for new secrets |
| Existing SDK `member.removed` event | Owner-only `member.remove` RPC, fold/materialize support, active-member/fleet semantics |

## Product experience

### Information architecture

The top-level Agents area has two peer views:

- **Installed**: existing fleet information plus managed installation details.
- **Marketplace**: catalog browse and detail pages.

The existing **Bring your own agent** action remains visible from Installed and
opens the current identity/ticket/command flow. It is labelled unmanaged, not
inferior.

### Marketplace journey

1. **Browse.** Search by name, use case, publisher, deployment (`Hosted` or
   later `Local`), verification, and compatible platform. Catalog cards expose
   deployment and verification without suggesting either proves safety.
2. **Inspect detail.** Show publisher fingerprint and verification scope,
   immutable version, summary, capabilities, plain-language data access,
   source/license where available, hosted privacy/terms, update policy,
   revocation/deprecation, and security implications.
3. **Choose rooms.** Show only rooms the local identity owns and can open.
   Each row says eligible, already installed, installing, removed, or requires
   attention. A room may be selected independently.
4. **Review access.** The final consent is specific to deployment:

   | Disclosure | Hosted MVP | Local, later |
   |---|---|---|
   | Room messages/history | May receive the synchronized timeline, including history | Same, unless a future protocol capability changes it |
   | Members | May receive member identities/roster | Same |
   | Files | May see metadata and fetch available room files | Same |
   | Host files/processes/credentials | **None granted by Jeliya**; provider infrastructure is outside the user's host | Requested separately and enforced by runtime policy |
   | Network | Provider's privacy/terms explain its processing | Explicit egress destinations/policy, enforced by supervisor |
   | External processing | Provider receives room data only after its identity becomes an authorized member | Not applicable unless a local agent separately calls an external service |

   No "messages but not files" toggle appears until the room protocol can
   enforce it.
5. **Confirm.** The user approves the immutable listing version and selected
   room count. Jeliya persists a local installation intent and begins
   provisioning. Cancellation is available while it can still prevent the next
   action; it never erases data already received by a provider.
6. **Observe.** Each room row reports only what its evidence proves: provider
   accepted, identity provisioned, invite delivered, joined, connected, status
   observed, ready, failed, cancelled, or removed.
7. **Manage.** An installed detail view shows manifest name/version/publisher,
   identity fingerprint per room, liveness, last real status, audit timeline,
   and controls for retry, cancel, remove, and later upgrade. Pause/disable is
   operational only; it does not remove the agent's room access. The security
   action is removal.

### Duplicate and partial-result policy

The same `(listing_id, version, room_id)` is deduplicated locally. Repeating a
request returns or resumes the existing room binding rather than minting a
second invite. A different version is an upgrade candidate, not a duplicate.

Multi-room installation is a saga, not an atomic transaction. A user can see
Room A ready and Room B expired/failed. Retry and removal operate per room;
the parent record presents an honest aggregate such as `partially_ready`.

## Marketplace trust model

### Agent manifest

Catalog data is untrusted until a signed manifest passes local validation.
Listings use immutable versioned manifests; display copy is treated as data,
not executable markup.

```ts
type DeploymentMode = "hosted" | "local";

interface AgentManifestV1 {
  schema_version: 1;
  listing_id: string;                 // stable opaque id, e.g. "com.example.echo"
  version: string;                    // immutable semantic version
  name: string;
  summary: string;
  description_markdown: string;       // rendered through a strict sanitizer
  icon: { url: string; sha256?: string } | null;

  publisher: {
    publisher_id: string;
    display_name: string;
    signing_key_id: string;
    signing_public_key: string;       // Ed25519, base64url
    verification: "unverified" | "marketplace_verified";
    verification_scope?: string;
  };

  deployment: {
    mode: DeploymentMode;
    capabilities: string[];
    data_access_disclosure: string[];
    default_triggers?: string[];
    usage_instructions?: string;
    hosted?: {
      provisioning_url: string;
      privacy_policy_url: string;
      terms_url: string;
      provider_key_id: string;
      provider_public_key: string;
    };
    local?: {
      platforms: Array<{ os: string; arch: string }>;
      artifacts: Array<{
        url: string;
        sha256: string;
        signature: string;
        signature_key_id: string;
      }>;
    };
  };

  provenance?: { source_repository?: string; license?: string };
  update_policy: "manual" | "notify" | "auto";
  lifecycle: {
    status: "active" | "deprecated" | "revoked";
    reason?: string;
    replacement_listing_id?: string;
  };
  published_at: string;
}

interface SignedManifestV1 {
  manifest: AgentManifestV1;
  publisher_signature: string;        // signature over canonical manifest bytes
  marketplace_attestation?: {
    catalog_key_id: string;
    signature: string;                // attest listing/version/publisher key/hash
  };
}
```

`marketplace_verified` means the marketplace performed the documented identity
or policy check. It never means the agent is harmless, private, or safe to run.
The UI puts the verification scope next to the badge.

### Validation, caching, and revocation

`jeliya-marketplace` canonicalizes the manifest, validates schema/size/URLs,
checks publisher signatures and any marketplace attestation against pinned or
user-configured trust roots, and stores the verified bytes plus hash. It caches
last-known verified search results and manifests with fetch time, expiry, and
revocation-feed generation.

The daemon refreshes catalog metadata with HTTPS, redirect and SSRF policy,
and bounded response sizes. Offline mode may browse an unexpired verified cache
but disables new hosted provisioning. A newer manifest cannot replace the
selected version in an active installation. Unknown-key, invalid-signature,
downgrade, or revoked content is blocked and shown with a stable error code.

Revocation makes a listing unavailable for new installs/updates and creates a
visible warning for existing ones. It must not silently remove a P2P member:
that requires an owner-authorized room event and a local policy decision.

## Hosted installation protocol

### Responsibility boundary

```text
React / Flutter                  jeliyad + jeliya-marketplace        Provider             jeliya-core / room
───────────────                  ────────────────────────────        ────────             ──────────────────
render + collect consent ─────► verify catalog / persist saga ────► provision identity
                                create short-lived invite ─────────► receive ticket+hints
render evidence ◄────────────── observe state ◄──────────────────── start hosted agent ─► join / signed events
                                               ◄────────────────────────────────────────── membership + peers/status
```

The UI never talks to provider endpoints directly. This avoids browser CORS,
untrusted redirect handling, provider credentials and raw invite tickets in UI
state; it also keeps web and native behavior identical. `jeliya-core` remains
the sole Iroh Rooms boundary. The new marketplace crate is SDK-free.

### Identity and idempotency model

Use a parent `installation_id` for a user-approved listing/version selection
and one child binding per room. Each binding receives a distinct newly
provisioned Jeliya identity. This fits the current one-room runner model,
limits cross-room correlation and blast radius, and makes per-room removal
precise.

Every operation has a client-generated `idempotency_key`, persisted before its
external request. The provider receives it with the listing/version and an
opaque room slot, not room content. A retry returns the same provisioned
identity or a deterministic conflict; it must never silently substitute one.

### Provider contracts

Before final consent, Jeliya may request a provisional identity binding. The
provider is not given tickets, dial hints, room messages, files, room names, or
member identities at this point.

```json
POST /v1/installations/provision
{
  "listing_id": "com.example.echo",
  "version": "1.4.2",
  "idempotency_key": "opaque-random",
  "room_slots": [{ "slot_id": "opaque-random" }]
}
```

```json
201 Created
{
  "provider_installation_id": "prv_...",
  "listing_id": "com.example.echo",
  "version": "1.4.2",
  "bindings": [{
    "slot_id": "opaque-random",
    "agent_identity_id": "64-hex-jeliya-identity",
    "agent_identity_proof": "signature by that identity key",
    "binding_signature": "signature by manifest provider key"
  }],
  "expires_at": "2026-07-10T12:00:00Z"
}
```

The provider binding signature covers provider installation ID, listing ID,
version, local installation ID, slot ID, agent identity, and expiry. Jeliya
verifies both the manifest-selected provider key and proof that the claimed
identity controls the key. A response with a different listing/version,
expired proof, duplicate slot, or unmatched identity fails as
`provider_identity_mismatch`.

After final consent, Jeliya opens each selected room and mints an identity-bound
short-lived invite. Five minutes is the initial policy; it is configurable only
by local policy, never by provider input.

```json
POST /v1/installations/authorize
{
  "provider_installation_id": "prv_...",
  "binding_id": "slot-id",
  "agent_identity_id": "64-hex-jeliya-identity",
  "room_invitation": {
    "ticket": "roomtkt1...",
    "peer_dial_hints": ["endpoint@host:port"]
  }
}
```

The request uses HTTPS request bodies, never URLs or headers likely to be
recorded. Raw tickets never appear in logs, diagnostics, analytics, CLI
arguments, browser storage, or durable installation records. Structured
redaction treats tickets as a secret type rather than relying only on regexes.

### Evidence-backed state machine

```text
draft → manifest_verified → provision_requested → identity_provisioned
      → consented → invite_created → authorization_submitted
      → joining → joined → connected_or_status_observed → ready
                                        │
                                  failed / cancelled / removal_pending → removed
```

| State | Source of truth | Meaning |
|---|---|---|
| `manifest_verified` | local manifest verifier | selected immutable manifest passed policy |
| `provision_requested` / `identity_provisioned` | provider HTTP + verified signatures | provider accepted and bound a specific identity |
| `invite_created` | local `invite.create` result and folded event | a short-lived identity-bound invite exists |
| `authorization_submitted` | daemon delivery acknowledgement only | provider was given the ticket; it has not joined |
| `joined` | signed active `member.joined` / membership fold | exact provisioned identity joined the room |
| `connected` | bound device in real peer connection state | a live P2P connection was observed |
| `status_observed` | signed `agent_status` by exact identity | status was actually authored, not provider HTTP assertion |
| `ready` | membership plus connected **or** status observed | minimum truthful installed evidence |
| `failed`, `cancelled`, `removed` | persistent daemon state plus relevant room evidence | terminal or operator action; reason and evidence retained |

Provider HTTP 200 never means joined, online, or ready. A signed status proves
the author identity, not the provider's binary/version or truthfulness.

### Failure, retry, and cleanup behavior

- **Ambiguous request failure:** query provider by idempotency key. If still
  ambiguous, reconcile the room for the exact provisioned identity before
  making a new provision request.
- **Expired invite:** never resend an old ticket. Reuse the verified identity,
  mint a new short-lived invite, submit it, and retain the prior attempt audit.
- **Daemon restart:** no ticket is persisted. Reconcile signed membership and
  evidence; if not active, create a fresh invite only after explicit retry or
  the documented automatic retry policy.
- **Unavailable hints / timeout:** record `provider_timeout` or
  `join_timeout`, retain the identity, and offer retry/cancel. The provider can
  use discovery/relay if supported; it must not be given arbitrary user URLs.
- **Provider cancellation:** mark the binding failed/cancelled, then attempt
  owner removal if it joined. Provider-side stop is not room revocation.
- **Multi-room:** continue independent bindings unless the user cancels the
  parent. Do not roll back a ready room automatically.
- **Removal:** owner publishes `member.removed` first; Jeliya observes it;
  then the daemon sends a provider cleanup request. If cleanup fails, the room
  result remains removed and the provider cleanup is retried separately.

## Local-agent execution model: later phase

Local mode requires a dedicated supervisor, proposed as `jeliya-agentd`. It is
not a mode of the browser, `jeliya-agent.mjs`, or the existing macOS sidecar.
`jeliyad` may orchestrate it through a narrow local IPC contract but must not
execute untrusted packages itself.

Before enabling an Install button, the supervisor needs all of the following:

- verify publisher and artifact signatures, hashes, platform/architecture, and
  immutable version pin before unpacking;
- separate identity, encrypted-or-OS-protected secrets, data directory, and
  audit trail per room binding; separate per-task workspace;
- filesystem allowlists and confinement; no access to Jeliya identity secrets,
  host home directory, SSH material, browser profiles, or ambient credentials;
- restricted subprocess policy, CPU/memory/disk/process/time quotas, crash
  recovery, ownership-safe shutdown, and deterministic log capture;
- default-deny network egress with a visible policy; resolver/proxy controls to
  prevent bypass; no inherited environment except an explicit allowlist;
- secret broker with named, scoped grants instead of environment variables;
- side-by-side signed upgrades, health check, atomic switch, rollback, and
  uninstall that removes artifacts/data/secrets after member removal;
- OS-specific enforcement: namespaces/Landlock/seccomp/cgroups or a rootless
  container on Linux; a signed helper with sandbox/VM boundary on macOS;
  AppContainer/restricted token and Job Object on Windows. Mobile platforms do
  not run arbitrary marketplace packages.

The first local format should be a constrained WASI-style runtime if product
needs justify it. A native arbitrary-code package format is not an MVP.

## Data model and daemon API

### Local installation records

These records live in a transactional local marketplace database under the
daemon data directory. They are projections of local intent and observed room
evidence, not room events. They are never synced to room peers by default.

```ts
interface AgentInstallation {
  installation_id: string;
  listing_id: string;
  manifest_hash: string;
  version: string;
  deployment_mode: "hosted" | "local";
  created_at: string;
  updated_at: string;
  state: "draft" | "provisioning" | "partially_ready" | "ready" |
         "failed" | "cancelled" | "removal_pending" | "removed";
  room_bindings: RoomInstallationBinding[];
}

interface RoomInstallationBinding {
  binding_id: string;
  room_id: string;
  agent_identity_id: string;
  provider_installation_id?: string;       // non-secret opaque identifier
  state: string;
  evidence: {
    membership_event_id?: string;
    connected_at?: string;
    status_event_id?: string;
    last_error?: MarketplaceError;
  };
}

interface LocalInstallRecord {
  installation_id: string;
  binding_id: string;
  artifact_sha256: string;
  runtime_version: string;
  identity_ref: string;                    // secret-manager reference only
  data_dir_ref: string;
  sandbox_policy_hash: string;
  update_state: "pinned" | "upgrade_available" | "rollback_available";
}

interface MarketplaceError {
  code: string;
  safe_message: string;
  retryable: boolean;
  retry_after_ms?: number;
}
```

Initial stable errors: `manifest_invalid`, `manifest_signature_invalid`,
`manifest_revoked`, `marketplace_offline`, `provider_untrusted`,
`provider_identity_mismatch`, `provider_timeout`, `not_room_owner`,
`room_not_openable`, `invite_expired`, `join_timeout`, `already_installed`,
`member_removal_unsupported`, `local_runtime_unsupported`, and
`cancelled`. Raw provider errors are retained only in access-controlled local
diagnostics after redaction.

### RPCs

New RPC methods are daemon-owned. They use the existing envelope/error shape.

| RPC | Purpose |
|---|---|
| `marketplace.search { query, filters, cursor? }` | verified cached/network catalog search; returns cache age and offline status |
| `marketplace.agent.get { listing_id, version? }` | verified manifest/detail/revocation view |
| `agent.installation.prepare { listing_id, version, room_ids, idempotency_key }` | eligibility, duplicate detection, pre-consent provisioning and identity proofs; no invite yet |
| `agent.installation.confirm { installation_id }` | records consent and starts per-room invite/authorization saga |
| `agent.installation.get/list` | durable state and evidence for UI restoration |
| `agent.installation.retry { installation_id, binding_id? }` | reconciles then retries only a safe next action |
| `agent.installation.cancel { installation_id, binding_id? }` | stops outstanding orchestration; removes joined members when possible |
| `agent.installation.remove { installation_id, binding_id? }` | owner removal then provider/local cleanup |
| `member.remove { room_id, identity_id }` | owner-only signed `member.removed`; prerequisite for honest removal |
| `marketplace.config.get/set` | configured/self-hosted catalog endpoint and explicit trust roots |

Reuse `room.open` per selected room because its returned endpoint is room
specific. Do not use `daemon.status.endpoint` for multi-room invites: it
represents only the first open room. Reuse `invite.create`, membership folding,
`agents.fleet`, and `agent.history`; do not add marketplace data to P2P events.

`member.remove` is the smallest required protocol-facing extension. It should
validate owner authorization, build/validate/fold/publish the SDK's existing
event, materialize a roster change, and make all reads distinguish invited,
active, left, and removed. It must be idempotent for an already removed member.

## Threat model

| Threat | Mitigation | Residual risk |
|---|---|---|
| Malicious listing or compromised publisher | Signed immutable manifests, trust roots, review/verification policy, manifest disclosures, revocation feed | A valid signed agent can still be harmful or misleading |
| Marketplace server compromise | Verify publisher signatures/attestations locally; pinned/configured keys; cache verified manifests; no room data on catalog browse | Compromise can deny service or promote a previously trusted malicious version |
| Provider identity substitution | Provider binding signature plus identity-key proof; invite bound to that exact identity; verify signed active membership | Provider can operate the correct identity maliciously |
| Stolen invite ticket | Short expiry, single use, body-only HTTPS transfer, no persistence/logs/URLs/argv, structural redaction | Interception before redemption can allow one unauthorized join; TLS and host security remain required |
| Secrets in diagnostics | Typed secret values, redaction tests, controlled diagnostic access, bounded retention | Application/OS compromise can still read process memory |
| Hostile Markdown/icon/URL | No raw HTML; sanitized Markdown; allowlisted schemes; image proxy/cache; size/hash limits; no local/file URLs | Rendering bugs remain possible |
| SSRF/redirect abuse | HTTPS-only endpoint policy, DNS/IP restrictions, redirect limit and revalidation, no provider-supplied callback fetches | DNS rebinding and proxy environments need continuing hardening |
| Local package supply chain/updates | Defer local execution; later signed artifacts, pinning, TUF-like metadata, rollback and downgrade prevention | Sandboxes reduce, not eliminate, native-code risk |
| Agent reads unexpected room history/files | Plain-language broad-access disclosure; no false toggles; future protocol capabilities before granularity | An authorized agent can retain data it already received |
| Local agent reads host credentials | No local MVP; separate sandbox/supervisor/secret broker in later phase | Kernel/OS sandbox escape or explicit user grants remain risks |
| Duplicate/orphan installation | Durable idempotency keys, per-room bindings, reconciliation before retry, provider query API | Provider may retain an orphan until its cleanup endpoint succeeds |
| Cannot revoke a joined agent | Block release on owner `member.remove`; removal first, provider cleanup second | Removal cannot recall data already copied |
| Downgrade/replay | Immutable version + manifest hash, monotonic update metadata, signed revocation generation, no silent downgrade | Users may intentionally pin an older safe-but-vulnerable version |
| Marketplace outage | Verified cache for browsing; explicit offline state; provider operations retry/reconcile | New hosted installs cannot complete offline |

## Repository change map

| Area | Likely changes |
|---|---|
| `crates/jeliya-core` | owner removal writer; fold/materialize `member.removed`; active membership query and per-room evidence helper; tests |
| `crates/jeliyad` | marketplace RPC dispatch, persistent saga, push notifications, redacted diagnostics, configuration |
| new `crates/jeliya-marketplace` | manifest/types/signature validation, HTTP policy, cache, provider protocol, install state machine; no Iroh SDK dependency |
| `ui/src` | split fleet from Marketplace/Installed screens; multi-room consent/progress/manage UI; protocol models; deterministic mock and component/accessibility tests |
| `app/lib/src` | Marketplace store as daemon projection, screens/dialogs, typed protocol, ARB EN/FR, widget/localization tests |
| `dart/jeliya_protocol` | installation/manifest/error models, RPC methods, mock behavior and tests |
| `scripts` and tests | mock catalog/provider, hosted safe-echo E2E, retry/removal scenarios; no paid model or generated arbitrary code |
| CI | Rust tests, web Vitest, agent/fleet/hosted E2E gates in addition to current Flutter/Dart checks |

## Phased delivery plan

| Phase | Deliverable | Dependencies | Relative complexity |
|---|---|---|---|
| 0 | Confirm launch clients; add owner `member.remove`; fix membership/fleet semantics; CI baseline | SDK event builder, core/daemon/UI contract | High |
| 1 | `jeliya-marketplace`: manifest verification, endpoint policy, cache, catalog search/detail, fixtures | trust-root policy and storage choice | High |
| 2 | Hosted single-room install saga: provision, identity bind, short invite, reconcile, evidence-backed UI | Phases 0–1, provider test service | High |
| 3 | Multi-room parent/child records, retry/cancel/remove, installed management, web/native localization and accessibility | Phase 2 | High |
| 4 | Self-hosted catalogs, revocation/update notifications, production native networking and explicit mobile scope | Phases 1–3 | Medium–High |
| 5 | `jeliya-agentd` constrained local runtime, package format, sandbox pilots | OS security engineering and audit review | Very high |

The work can run in parallel after Phase 0: core removal semantics, marketplace
crate, and UI prototypes with mocks. Provider integration and final UI should
wait for the state-machine contract.

## Test strategy and acceptance criteria

Unit tests cover manifest schema/canonicalization/signatures, trust roots,
revocation and downgrade checks, URL/redirect/SSRF policy, identity proof and
provider binding, idempotency/state transitions, ticket redaction, and removal
authorization. All tests use fixed clocks and deterministic keys.

Integration tests use a mock catalog and provider plus the existing safe `echo`
worker. They cover successful exact-identity join; wrong identity rejection;
expired invite; lost response and reconciliation; provider timeout; duplicate
install; non-owner failure; multi-room partial completion; offline verified
cache; provider cancellation; removal/revocation; and no catalog access to room
messages/files before membership.

Client tests cover web and Flutter browse/detail/consent/progress/manage views,
navigation away and restart restoration, keyboard/focus semantics, screen-reader
labels, responsive layout, English/French parity, long French strings, and mock
state transitions. No test uses a paid LLM or executes model-generated code.

The hosted MVP is accepted only when all of these are true:

1. BYOA still creates an agent-role invite and shows the manual command without
   spawning a process.
2. Catalog browse performs no room-content or file transfer to a marketplace.
3. Every installed version is a locally verified immutable manifest hash.
4. The final consent accurately states broad room access and hosted processing.
5. The daemon, not a client, owns provisioning and persistent progress.
6. A provider cannot cause success with a substituted identity.
7. Tickets have short expiry, are single use, and never appear in logs, URLs,
   diagnostics, command arguments, or persisted installation records.
8. `ready` requires signed active membership for the exact identity plus peer
   connectivity or a real signed status event.
9. Timeout, retry, duplicate, and partial multi-room states remain truthful
   after daemon/client restart.
10. Only active rooms owned by the user are selectable.
11. `member.remove` is owner-only, signed, idempotent, materialized, and
    prevents the removed agent from continuing as an authorized participant.
12. Remove changes room membership before provider cleanup and never claims to
    erase already received data.
13. Revoked manifests are blocked for fresh installation and visibly warn on
    existing records.
14. Web/native tests, manifest/security tests, and deterministic hosted E2E
    are automated in CI.
15. Local installation remains unavailable until the supervisor enforces the
    stated sandbox and lifecycle controls.

## Open questions and blockers

1. What marketplace trust-root governance, publisher verification process, and
   incident/revocation service are acceptable for Jeliya's operating model?
2. Which provider authentication mechanism binds a user-approved installation
   without turning the catalog into a Jeliya account system?
3. What privacy/retention/audit terms are required before hosted providers can
   process room data, and who enforces provider compliance?
4. Should a hosted provider get one identity per room (recommended) or a
   multi-room identity only after runner/protocol support exists?
5. Is the first release web-only, or will macOS real networking and a mobile
   production transport be delivered first?
6. Which durable local store and encryption/keychain policy should contain
   installation metadata and provider opaque identifiers?
7. What is the owner UX for a revoked agent whose provider is unavailable, and
   how long are cleanup retries/audit records retained?
8. Does the product need granular agent permissions enough to justify an
   explicit future room-protocol capability model?

Until owner removal and an explicit client-network scope are resolved, the
marketplace can be prototyped with mocks but must not be marketed as an
install-and-remove capability.
