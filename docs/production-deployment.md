---
type: "Architecture"
title: "Production deployment architecture"
description: "Repository-grounded assessment, target architecture, security boundaries, infrastructure plan, and phased gates for deploying Jeliya at app.jeliya.ai."
tags: ["architecture", "deployment", "production", "security", "pwa", "iroh"]
timestamp: "2026-07-19T15:15:00Z"
status: "proposal"
implementation_status: "planned"
verification_status: "partial"
release_status: "unreleased"
audience: ["contributors", "maintainers", "operators", "product", "release-engineers", "security-reviewers"]
---

# Production deployment architecture

This document is the decision-ready plan for making Jeliya safely usable from
`https://app.jeliya.ai` while preserving its local-first, peer-to-peer,
signed-event architecture. It distinguishes repository facts from proposed
work, compares the viable deployment models, defines the recommended trust
architecture, and provides measurable delivery gates.

It does not authorize a production deployment by itself. The page remains a
proposal until the architecture decision is accepted, and its implementation,
verification, and release status must advance independently under the
[documentation profile](PROFILE.md).

## Executive decision

The current React build is **not** a deployable functional web application, and
`jeliyad` must **never** be exposed through a public listener or reverse proxy.

The target is a **capability-aware hybrid**:

1. `app.jeliya.ai` serves an immutable static PWA.
2. The first production release pairs that PWA with a signed local Jeliya
   companion over a new mutually authenticated, end-to-end-encrypted Iroh
   control protocol.
3. A browser-resident Wasm room peer follows only after browser storage,
   signing, synchronization, and Iroh Rooms adapters pass independent gates.
4. Dedicated relays route encrypted traffic but never join rooms.
5. Optional server peers use distinct identities and are explicitly invited.
   Under the current protocol, a server peer can read the room content it
   receives. A content-blind server requires a new application-encryption layer.

For a small team of two to three engineers, the companion-backed production
slice is estimated at **11 to 17 engineering weeks**. A robust browser-only peer
adds approximately **10 to 14 weeks**. These are planning estimates, not release
commitments.

## Assessment boundary and evidence

The repository assessment was performed on 2026-07-17 and 2026-07-18 from
Jeliya HEAD `4d4621c929e6f9678b31b7e4a3ee1c8d751b545b` on branch
`feat/69-fleet-attention-projection`.

The exact qualification boundary is different:

- Signed direct and forced-relay evidence binds the earlier Jeliya commit
  `55024a46b3e112796ba2acf1dc408dab26dbba2e` and Iroh Rooms commit
  `71fbb5007bef4ce83631c94762ec68c2beef3d79` (tag `v0.1.0-rc.3`).
- The current dependency candidate is Jeliya
  `9c71fac2104a74076662177cf4ef74bb5050bae9` with the deliberately untagged
  Iroh Rooms revision `a5d98b70d717f35d3ce60953a88e12e646f2e871`, the first upstream `main`
  merge carrying the fixes for `kortiene/iroh-room#121` and
  `kortiene/iroh-room#119` plus the intervening connection-generation fixes.
- Exact-revision upstream regressions, the Jeliya workspace tests, and the
  two-daemon loopback suite pass at the new pair. The older signed manifests do
  not transfer: fresh signed direct and forced-relay runs must bind the new
  public Jeliya commit and dependency revision before release qualification.

Local verification performed during the assessment:

- React: 87 Vitest tests passed and the Vite production build succeeded.
- Rust core and daemon: 71 unit tests passed; one opt-in performance test was
  ignored.
- Documentation, secret-storage, and release-contract checks passed.
- During the initial assessment, a full `cargo test --locked --workspace` could
  not build `jeliya-ffi` because the local environment lacked Dart SDK headers;
  core and daemon tests passed separately.
- Follow-up qualification on 2026-07-19 supplied the installed Dart SDK headers:
  the full locked workspace passed 77 tests with one intentional performance
  ignore at `9c71fac...` + `a5d98b70...`.
- On 2026-07-17, `jeliya.ai` and `app.jeliya.ai` had no resolvable A, AAAA, or
  CNAME record from the assessment environment.

The repository is the source of implementation truth. Current platform facts
come from authoritative browser, WebAssembly, and Iroh documentation listed in
[Citations](#citations).

## Current-state classification

### What works today

- The Rust core authors, validates, stores, folds, and synchronizes signed Iroh
  Rooms events. The workspace pins the reviewed upstream revision and forbids
  unsafe Rust in workspace code.
- The shared engine implements identity creation, rooms, invites, membership,
  messaging, files, pipes, peer status, and agent projections. See
  [`crates/jeliya-core/src/engine.rs`](../crates/jeliya-core/src/engine.rs).
- Native storage uses a shared SQLite WAL database and per-room filesystem blob
  stores. See
  [`crates/jeliya-core/src/supervisor.rs`](../crates/jeliya-core/src/supervisor.rs).
- Native Iroh networking supports real-network and loopback-test modes.
- Signed evidence exists for direct and forced-relay behavior at exact recorded
  revisions, with the limits documented in
  [Verification evidence](verification-evidence.md).
- Files are bounded to 100 MiB, BLAKE3-verified, confined against arbitrary
  local-file sharing, and fetched from active peers. There is deliberately no
  central inbox or guaranteed offline delivery.
- Pipes are restricted to numeric loopback targets and an authorized peer.
- The React UI works against the loopback daemon and has mock browser coverage.
- CI pins third-party actions and includes Rust, TypeScript, Dart/Flutter,
  platform, dependency-audit, release-sealing, and smoke-test jobs. See
  [`.github/workflows/ci.yml`](../.github/workflows/ci.yml) and
  [`.github/workflows/release.yml`](../.github/workflows/release.yml).

### What can be deployed immediately

Only the following surfaces can be deployed without architectural work:

- a clearly labeled `VITE_MOCK=1` visual demo;
- documentation, downloads, checksums, and installation instructions;
- existing daemon artifacts for local loopback use, subject to their technical
  preview and unsigned-package limitations.

None of those surfaces satisfies the goal of a safe, functional Jeliya product
at `app.jeliya.ai`.

### What requires architecture changes

- browser-resident identity, event store, blob store, synchronization, and Iroh
  networking;
- a service worker, offline cache, migrations, and storage-eviction recovery;
- a secure browser-to-companion control protocol;
- OS-backed key storage, recovery, and key rotation;
- multiple devices per identity, device revocation, and cross-device sync;
- invitation cancellation and safer default expiry;
- idempotent message submission and incremental timeline cursors;
- relay authentication that does not expose a project secret to a browser;
- production DNS, TLS, security headers, infrastructure, and deployment
  workflows;
- signed and notarized native companions;
- a permissioned, quota-bound Wasm component host;
- application-layer content encryption if managed infrastructure must store
  room data without reading it.

### What remains experimental or blocked

- The Iroh Rooms online runtime is experimental and unpublished. Its current
  store, blob, and network implementation is native, not a browser runtime.
- Browser Iroh is supported, but current browser connections are relay-only and
  require an application-specific `wasm-bindgen` wrapper.
- Native GUI artifacts are unreleased or incompletely verified. Android lacks
  Keystore wrapping and remote-network evidence; iOS has no application
  scaffold. See [Platform matrix](platform-matrix.md).
- Bare daemon and native artifacts are unsigned. See
  [Signing and notarization](signing-notarization.md).
- The existing agent runner is an intentional, unsandboxed local code-execution
  surface. It must remain unavailable to the hosted browser product.
- No component loader, signed package format, permission broker, quota model,
  upgrade rollback, or revocation path exists.
- Upstream `kortiene/iroh-room#121` and `kortiene/iroh-room#119` are fixed on
  upstream `main` after tag `v0.1.0-rc.3`. Jeliya deliberately pins the first
  merge carrying both fixes (`a5d98b70...`) and locally requalifies the
  provisional-peer gate, connection-generation teardown, synchronization
  isolation, and store retry/degradation behavior. Store retry exhaustion or
  queue overflow fails loudly through a durable critical `store_degraded`
  decision; it does not make disk failure impossible. Fresh signed network
  evidence remains a Phase 0 gate.

## Why the loopback daemon must not be public

`jeliyad` is local-only by construction:

- [`crates/jeliyad/src/main.rs`](../crates/jeliyad/src/main.rs) binds only
  `127.0.0.1` and exposes no flag for a non-loopback address.
- It creates one per-process bearer token and writes that token to the local
  owner-only portfile.
- `/api/session` gives the token only to the expected loopback browser shape.
  Its documented threat model explicitly excludes hostile same-user processes
  and shared multi-user service operation.
- Host and Origin checks defend a loopback application from DNS rebinding and
  cross-site WebSocket hijacking. They are not remote account authentication.
- The RPC surface includes identity creation, daemon shutdown, room history,
  native file operations, pipes, and agent projections.
- One daemon data directory represents one user identity. There is no tenant,
  account, authorization-domain, quota, or public audit model.
- It has no public TLS, remote pairing, device approval, abuse controls, or
  multitenant resource isolation.
- The current UI transports bearer material in WebSocket and upload query URLs.

A reverse proxy would not add the missing security model. It would instead
invalidate the Host and Origin assumptions and place a single-user,
high-authority local API behind public ingress. The production companion control
protocol must be designed as a separate surface. Do not add a public-listen
flag, proxy `/ws`, or reuse the daemon token remotely.

## Deployment-model comparison

Planning estimates assume two core/full-stack engineers, one web/operations
engineer at least part-time, and an independent security review.

| Dimension | Static PWA with Wasm peer | Hosted shell with native companion | Hosted gateway or managed backend | Capability-aware hybrid |
|---|---|---|---|---|
| Security | Keys and plaintext exist in the browser origin; origin or CDN compromise can sign or exfiltrate | Root and device keys remain native; origin compromise can still read displayed content and use granted scopes | Central ingress, tenant isolation, and key custody become critical risks | Bounds authority by mode; highest design complexity |
| Privacy | Strong data locality; relay sees IP, endpoint, timing, and volume metadata | Authoritative data stays native; browser sees rendered content | Backend normally sees content and metadata unless a new encrypted-envelope protocol is built | User chooses local browser, companion, or explicitly trusted server peer |
| Local-first integrity | Strong if signed events persist locally and sync peer-to-peer | Strongest reuse of the current native store | Weak if the gateway becomes authoritative or mandatory | Strong while browser/native stores remain authoritative and servers stay optional |
| Browser compatibility | Modern browsers only; Iroh is relay-only; feature detection required | Broad modern-browser coverage after companion installation | Broadest browser reach | Broad, with explicit capability degradation |
| Offline behavior | Good while storage survives; active-browser execution only | Cached reads and drafts; signing and sync require the companion | Limited unless the gateway is replicated into an offline client | Browser mode can work offline; companion mode has truthful limited offline behavior |
| Identity and key storage | WebCrypto plus IndexedDB wrapping; active origin can invoke usable keys | OS Keychain, DPAPI/CNG, Secret Service, or Keystore | HSM/server keys or client-side crypto required | Root/device keys remain on the selected execution peer |
| P2P networking | Browser traffic always uses a relay | Native direct, NAT traversal, and relay remain available | Backend becomes a network hub | Browser relay-only; native direct/relay; optional server peers |
| Component execution | Worker/iframe sandbox only | Native Wasmtime can support broader capabilities | Requires a hardened server sandbox and tenant scheduler | One signed package with per-runtime capability profiles |
| File handling | Picker and OPFS; no arbitrary native path; quota-sensitive | Full native files through approved companion actions | Central upload, storage, privacy, and egress cost | Capability-specific streaming; no implicit cloud copy |
| Background agents | Not reliably supported | Companion can run subject to native OS policy | Natural fit but operationally expensive | Browser never claims always-on work; native/server may |
| Operational complexity | Low static hosting, medium runtime/relay work | Medium: installers, pairing, relays, native lifecycle | High: accounts, databases, isolation, backups, and abuse | Medium-high, but can launch incrementally |
| Cost shape | CDN plus relays; browser traffic is always relayed | CDN plus relays; native direct paths can reduce egress | Compute, storage, database, backup, and egress | CDN/relays first; server cost only for opted-in services |
| First safe production | Approximately 16 to 24 engineer-weeks | Approximately 11 to 17 engineer-weeks | At least 24 weeks | First companion slice in 11 to 17 weeks; browser mode follows |

### Decision

Adopt the hybrid model and use the companion-backed shell as the first
production slice.

A browser peer remains the intended zero-install capability, but the repository
does not yet contain its storage or network runtime. A gateway would gain
browser reach by replacing Jeliya's current privacy and local-first boundaries
with server trust.

## Target system and trust boundaries

```text
                           Static supply plane
                     +--------------------------+
                     | DNS + TLS + CDN          |
                     | app.jeliya.ai            |
                     | hashed PWA/Wasm/config   |
                     +------------+-------------+
                                  | TB1: web origin controls browser code
                                  v
+------------------------------------------------------------------+
| Browser                                                          |
|                                                                  |
| UI + service worker + capability detection                       |
|                                                                  |
|  Companion mode                 Browser-peer mode, later          |
|  - paired control key           - device key                     |
|  - encrypted view cache         - signed-event runtime           |
|  - drafts/idempotent intents    - IndexedDB + OPFS               |
+--------------+--------------------------+------------------------+
               | E2E Iroh control         | E2E Iroh Rooms
               +--------------+-----------+
                              | TB2: relays see transport metadata
                 +------------v-------------+
                 | Dedicated Iroh relays    |
                 | at least two regions     |
                 | stateless, not members   |
                 +------+-----------+-------+
                        |           |
             +----------v---+   +---v---------------+
             | Native       |   | Optional server   |
             | companion    |   | peer              |
             |              |   | separate identity |
             | OS keystore  |   | explicit invite   |
             | SQLite/blobs |   +-------------------+
             | native Iroh  |        TB4: server peer reads content
             | direct/relay |        unless new E2EE is implemented
             +------+-------+
                    |
       TB3: native files, pipes, and agent execution require
            explicit local approval and stronger permissions

       TB5: Wasm components are untrusted code behind WIT imports,
            host policy, process/worker isolation, and quotas
```

The trust boundaries are:

- **TB1, web supply chain.** A compromised origin, CDN account, or frontend
  dependency controls the browser session. CSP reduces injection risk but
  cannot make a deliberately malicious first-party build trustworthy.
- **TB2, relay metadata.** Relays cannot decrypt Iroh connections, but they see
  source IPs, endpoint-routing data, timing, and byte volume.
- **TB3, native authority.** Companions hold identity keys, local files, pipes,
  and optional agent capability. Browser controllers receive narrow, revocable
  grants.
- **TB4, room membership.** Every room peer is authorized to receive the room
  data delivered to it. Signatures prevent forgery; they do not prevent an
  authorized peer from copying content.
- **TB5, component execution.** Package signatures prove provenance, not
  harmlessness. Imports, policy, quotas, and isolation enforce authority.

## Component responsibilities

| Component | Responsibilities | Prohibited responsibilities |
|---|---|---|
| `app.jeliya.ai` | Serve immutable PWA/Wasm assets, public environment config, publisher trust roots, and signed revocation metadata | Store room events, identities, invites, or user keys; proxy `jeliyad`; contain secrets |
| Browser | Remove invite fragments, render UI, detect capabilities, maintain shell/cache, and operate either a scoped companion session or a browser room peer | Claim native direct P2P, durable background execution, or durable storage when unavailable |
| Native companion | Hold native identity/device keys, SQLite and blobs; run native Iroh; enforce paired-client scope; expose native files/pipes/agents only with approval | Expose public HTTP/WS; give a browser a daemon token; accept an unpaired controller |
| Relays | Route end-to-end-encrypted Iroh traffic, assist NAT traversal, enforce short-lived access tokens and resource limits | Join rooms, store room history, or retain endpoint relationships indefinitely |
| Optional server peer | Provide explicit availability, replication, or hosted execution under its own identity | Impersonate a user, silently join a room, or claim content blindness under the current protocol |

## Identity, device pairing, recovery, and cross-device access

### Current identity boundary

[`crates/jeliya-core/src/identity.rs`](../crates/jeliya-core/src/identity.rs)
creates one identity/root key and one event/device key. It stores both seeds in a
plaintext JSON secret protected by filesystem permissions. The current UI
truthfully states that the identity is unrecoverable. There is no export,
recovery, rotation, device authorization, or same-identity cross-device flow.

### Target creation and storage

1. Generate a long-lived profile/root key and a per-device endpoint/event key
   locally.
2. On native platforms, wrap secrets with macOS Keychain, Windows DPAPI/CNG,
   Android Keystore, or Linux Secret Service. An encrypted-file fallback must be
   explicit and password-hardening parameters must be versioned.
3. In the browser, prefer a nonextractable WebCrypto Ed25519 key when browser
   compatibility and exact wire interoperability pass. Otherwise wrap the seed
   with a nonextractable WebCrypto key and load it into Wasm only while active.
4. Treat browser key protection as at-rest defense. A malicious same-origin
   script can still invoke a usable key and may observe active memory.

### Browser-to-companion pairing

The browser control identity is separate from the Jeliya profile or room-device
identity.

- The companion shows a QR or custom-protocol link containing an ephemeral
  public key, endpoint, and nonce, never a reusable bearer secret.
- The peers establish a Noise XX-equivalent authenticated transcript over Iroh.
- Both sides display a short authentication string and require user
  confirmation.
- The companion records the browser public key, granted scopes, expiry,
  creation time, and last use.
- Default scopes cover selected-room reads and idempotent chat sends only.
- Invite creation, file access, pipes, identity operations, and agents require
  separate approval.
- Control keys are rate-limited, expire, and can be revoked immediately.

### Recovery

- Generate a random 256-bit recovery key, optionally represented as a recovery
  phrase.
- Export a versioned authenticated-encryption bundle containing the profile
  root, room membership index, device authorization state, and relay config.
- Do not derive the only recovery key from a low-entropy user password.
- Optional cloud storage holds only the opaque encrypted envelope.
- Require a successful test restore before setup is called complete.
- Explain that the recovery bundle restores identity authority, not unique
  unreplicated events or blobs.

### Cross-device access and revocation

Protocol v2 needs root-signed `device.authorized` and `device.revoked` events and
multiple active device bindings per identity.

- An existing authorized device pairs with the new device and asks the profile
  root to authorize its public device key.
- Room peers receive and validate the device-authorization update.
- The new device synchronizes signed history from current room peers or an
  explicitly invited availability peer.
- Device revocation blocks future authorship and future encrypted epochs. It
  cannot recall material already received.
- Strong confidentiality after removal requires group-content encryption and
  key-epoch rotation; current signed plaintext event bodies do not supply it.

## Secure invitation links

Use a fragment-only URL:

```text
https://app.jeliya.ai/join#v1.<base64url(canonical-ticket-envelope)>
```

URI fragments are processed by the browser and are not included in the HTTP
request or Referer header. Required controls are:

- A minimal first-party bootstrap reads the fragment into memory and calls
  `history.replaceState()` before React startup, service-worker registration,
  error reporting, or telemetry.
- Never store a ticket in `localStorage`, IndexedDB, Cache Storage, logs, crash
  reports, query strings, or URL paths.
- Set `Referrer-Policy: no-referrer` and load no third-party analytics, scripts,
  pixels, fonts, or social-preview processors on the join route.
- Add structural ticket and token redaction to browser, companion, relay-auth,
  support, and test tooling.
- Default to single-use with a 30-minute expiry for live pairing and no more
  than 24 hours for asynchronous invites.
- Add `invite.cancel` and close the provisional join window immediately after
  redemption or cancellation.
- Keep the upstream pin at or after `58aca4ba...` and require the
  `uninvited_provisional_dialer_receives_no_live_fanout` regression to pass at
  the exact resolved revision.

Current tickets are bound to a known invitee identity. Preserve that property.
New-user onboarding is therefore a two-step flow:

1. The invitee shares a public identity request.
2. The owner returns an identity-bound secret fragment.

A generic holder-bearer invitation is a different capability model and must not
replace identity binding implicitly. Browser extensions, screenshots, copied
links, and OS clipboard managers remain disclosure risks that the product must
state.

## Browser persistence, PWA, and offline behavior

The existing UI has an install manifest but no service worker or browser room
runtime. Its persistent state is limited to view selection, aliases, and drafts;
the daemon remains authoritative. See
[`ui/public/site.webmanifest`](../ui/public/site.webmanifest),
[`ui/package.json`](../ui/package.json), and [Daemon protocol](PROTOCOL.md).

### Companion mode

- Cache the static shell and an encrypted local projection of recently viewed
  rooms.
- Keep the companion authoritative.
- Treat cache or pairing-key eviction as a re-pair and resync event, not
  identity loss.
- Keep offline drafts locally. Queued sends carry a stable `client_msg_id`; the
  companion signs them after reconnection.
- Mark files, pipes, membership actions, and agents unavailable while the
  companion cannot be reached.

### Browser-peer mode

- IndexedDB stores signed event records, membership indexes, cursors, and
  package metadata.
- OPFS stores blobs, component packages, journals, checkpoints, and large
  snapshots.
- Cache Storage contains only exact content-hashed shell and Wasm assets.
- Request `navigator.storage.persist()`, display storage estimates, and enforce
  application quotas. Persistent storage reduces automatic eviction but does
  not prevent user deletion.
- Use atomic journals, versioned migrations, signed-event validation on boot,
  and crash-safe checkpoints.
- Maintain an eviction sentinel. Missing critical state stops authorship and
  offers recovery import or peer resynchronization. Never silently create a new
  identity.
- Explain that unreplicated local blobs disappear if browser storage is lost.

### Background boundary

A service worker supports cached startup and short deferred work. It is not a
permanent room peer or an agent host. Background Sync is unavailable in some
major browsers, and browsers terminate long-running service-worker work.
Browser peers are available while the application is active. Native or optional
server peers provide durable availability.

## WebAssembly component system

No production component system exists today. The unsandboxed agent runner is
not a substitute and remains disabled in hosted mode.

### Signed package

Every package contains:

- a canonical manifest;
- Wasm component bytes plus SHA-256 and BLAKE3 digests;
- a versioned WIT world and ABI version;
- publisher key and Ed25519 package signature;
- requested capabilities;
- minimum runtime version;
- memory, CPU, output, event-rate, storage, and concurrency limits;
- state-migration declarations.

Use TUF-like root, targets, snapshot, and timestamp metadata for publisher
delegation, freshness, rollback protection, and revocation. TLS alone does not
provide package provenance or rollback protection.

### Permissions and sandbox

- Define a narrow Jeliya WIT world. A missing import means the component cannot
  ask the host for that facility.
- Never give components an identity key or a generic signing primitive.
- A component proposes an action; the policy broker validates it, and the
  runtime/user signs the resulting event.
- Capabilities may cover selected event reads, selected file reads, bounded
  proposed events, component-private storage, and bounded clock/random access.
- Deny network by default. Optional access uses an explicit origin or hostname
  allowlist.
- Browsers expose no process, pipe, arbitrary host filesystem, or arbitrary
  socket capability.
- Browser components run in dedicated workers. Rendered component UI uses an
  opaque-origin sandboxed iframe and sanitized message boundary.
- Native components run through Wasmtime Component Model without ambient WASI
  filesystem, environment, process, or network access.
- Enforce maximum memory, CPU/wall-clock deadline, output bytes, event rate,
  storage, and concurrent instances. Quota violation terminates the instance
  without corrupting host state.

### Upgrade, rollback, and revocation

1. Verify and stage the new package beside the active version.
2. Migrate a copy of component state.
3. Run a bounded health check.
4. Atomically switch the active version.
5. Retain the last known good version for rollback.
6. Never rewrite signed room history during rollback.
7. Use signed revocation metadata to block future execution and clear cached
   packages. Revocation cannot undo events already authored.
8. Pin component versions per room when reproducibility matters.

Third-party components are excluded from the first production slice.

## Browser networking and Iroh Rooms

Iroh can compile to browser Wasm, but browser connections currently traverse a
relay because browser sandboxes do not provide the UDP hole-punching path. The
connections remain end-to-end encrypted. Iroh also requires default features to
be disabled and recommends an application-specific `wasm-bindgen` wrapper
rather than an off-the-shelf npm package.

This does not make the current Iroh Rooms runtime browser-compatible:

- `jeliya-core` enables the Rooms experimental online runtime.
- The current runtime assumes SQLite, filesystem blobs, and native Tokio
  networking.
- The stable upstream tier provides pure protocol authoring and validation that
  can be reused in Wasm.
- Persistence, sync transport, blob storage, clocks, and task spawning require
  browser adapters.

The implementation relationship is:

- Iroh Rooms remains the canonical signed event and membership format.
- Portable traits are introduced upstream or in an audited short-lived patch
  for event store, blob store, sync transport, clock, and task scheduling.
- Native uses SQLite, filesystem, and native-Iroh adapters.
- Browser uses IndexedDB, OPFS, and an Iroh Wasm relay adapter.
- One conformance corpus runs across native, browser, FFI, and fixture clients.
- Every release pins and qualifies an exact upstream revision.

### Relay design

- Start with two dedicated managed relays, one in North America and one in
  Europe.
- Relays are not room members and retain no room history.
- A browser obtains a short-lived, endpoint-bound relay credential from
  `relay-auth.jeliya.ai` after proof of possession. The project API secret never
  enters static assets.
- Native companions use the same short-lived credential policy rather than
  embedding a global project secret.
- Preserve the ability to move to self-hosted relays through configuration and
  infrastructure-as-code.
- Treat source IPs, endpoint routing, timing, and traffic volumes as sensitive
  metadata even though room content remains encrypted from the relay.

## Infrastructure, DNS, TLS, headers, secrets, and environments

### Initial infrastructure choice

- **DNS, CDN, and static host:** Cloudflare DNS and Pages, receiving an already
  built immutable artifact rather than rebuilding source.
- **Minimal dynamic service:** Cloudflare Worker at
  `relay-auth.jeliya.ai` for short-lived relay credentials.
- **Relay:** two Iroh managed dedicated relays.
- **Later component/recovery objects:** private R2 bucket protected by signed
  metadata and application encryption.
- **Infrastructure code:** OpenTofu under `infra/`.
- **Native signing:** Apple Developer ID/notarization, HSM-backed Windows
  Authenticode such as Azure Trusted Signing, and signed Linux
  repository/checksum plus provenance.

The provider choice is reversible. If provider-specific relay authentication or
identity requirements cannot satisfy the threat model, Phase 0 must choose an
equivalent static CDN, edge token service, and dedicated relay deployment before
implementation starts.

### DNS and TLS

- Create the production `app.jeliya.ai` record and a separate
  `staging.app.jeliya.ai` record.
- Enable DNSSEC and restrictive CAA records.
- Permit TLS 1.2 and 1.3 only, use managed renewal, and redirect HTTP to HTTPS.
- Enable HSTS with `includeSubDomains` only after every affected subdomain is
  HTTPS. Consider preload after an observation period.
- Separate development, staging, and production origins, relay projects, trust
  roots, credentials, and browser storage.

### Baseline Content Security Policy

```text
default-src 'none';
script-src 'self' 'wasm-unsafe-eval';
style-src 'self';
img-src 'self' data:;
font-src 'self';
connect-src 'self' https://relay-auth.jeliya.ai https://<relay-hosts> wss://<relay-hosts>;
worker-src 'self';
manifest-src 'self';
object-src 'none';
base-uri 'none';
frame-src 'none';
frame-ancestors 'none';
form-action 'none';
require-trusted-types-for 'script';
```

When component UI is introduced, add only the reviewed isolated component
origin to `frame-src`. Do not loosen the main origin to run arbitrary inline
component code.

Additional headers:

```text
Strict-Transport-Security: max-age=63072000; includeSubDomains
X-Content-Type-Options: nosniff
Referrer-Policy: no-referrer
Cross-Origin-Opener-Policy: same-origin
Cross-Origin-Resource-Policy: same-origin
Permissions-Policy: camera=(), microphone=(), geolocation=(), payment=(), usb=()
```

Add COEP only if Wasm threading requires cross-origin isolation and the complete
asset graph passes. Do not depend on hosted-page access to loopback addresses;
the relevant browser policy is still experimental and platform-dependent.

### Cache policy

- `index.html`, the service worker, and public environment config use
  `Cache-Control: no-cache`.
- Content-hashed JavaScript, CSS, Wasm, and images use
  `public, max-age=31536000, immutable`.
- Keep N and N-1 assets available through rollout and rollback.

### Secrets

- No Vite variable, bundle, manifest, or public config contains a relay project
  secret, signing key, cloud credential, or telemetry credential.
- Use protected GitHub environments and the narrowest provider credential
  supported. Prefer workload identity/OIDC where available; otherwise use a
  scoped, rotated deployment token.
- Keep component root trust offline or in an HSM. Use delegated online signing
  keys with short validity.
- Keep Apple and Windows signing material in platform-approved secret/HSM
  services and fail closed when signing is unavailable.

## CI/CD, smoke testing, rollback, and incident response

### Pull requests

Every pull request runs:

- the existing Rust, Dart/Flutter, TypeScript, documentation, secret, release,
  and dependency gates;
- Wasm compilation, API compatibility, and bundle-size budgets;
- Chromium, Firefox, and WebKit tests;
- real companion integration through a dedicated test relay, not only a mock;
- forced-relay two-peer convergence and reconnect;
- service-worker install, update, offline, and N/N-1 compatibility tests;
- storage quota, eviction, corruption, and migration tests;
- an assertion that invite fragments never enter HTTP requests, logs, or crash
  evidence;
- CSP and Trusted Types tests;
- protocol conformance, fuzzing, and malformed-frame tests;
- SBOM, license, secret, and Cargo/npm advisory checks;
- native signature validation for release candidates.

### Merge and staging

1. Merge to `main` builds the static artifact once.
2. CI emits its digest, SBOM, and signed provenance.
3. Those exact bytes deploy automatically to staging.
4. Staging smoke and compatibility suites run against dedicated staging relays.
5. Release candidates bake for at least 24 hours.

### Production promotion

Use a protected production environment with manual approval. Promote the exact
staged digest; do not rebuild.

Production smoke tests cover:

- DNS, certificate, HTTP redirect, and security headers;
- absence of unapproved third-party requests;
- PWA install and offline-shell startup;
- service-worker N/N-1 update;
- two ephemeral peers creating, joining, sending, and reconnecting;
- deliberately forced relay;
- companion pairing and control-key revocation;
- invite absence from CDN and token-service logs;
- quota warnings and storage recovery;
- absence of files, pipes, agents, and components when disabled.

### Rollback

- The CDN deployment pointer returns to immutable N-1 within 15 minutes.
- Runtime data migrations support N and N-1 or provide a forward-compatible
  read-only fallback.
- Component metadata has an independent signed kill switch.
- The companion can enforce a minimum-safe control-protocol version, but the web
  origin cannot rewrite native state or silently elevate scopes.

### Incident runbooks

Create and exercise runbooks for:

- malicious frontend or CDN credential compromise;
- relay project-secret or token-service compromise;
- native signing-key compromise;
- component publisher compromise;
- leaked invitations;
- protocol room-isolation failures;
- dependency advisories;
- browser-storage loss or corruption.

Responses include frontend rollback, browser-control-key revocation, relay-token
rotation, component revocation, signing-key rotation, content-free evidence
preservation, scoped user notification, and a fresh exact-revision
qualification run.

## Privacy-safe observability

Allowed aggregate metrics are:

- frontend build and runtime version;
- capability mode, such as companion or browser peer;
- result/error code and latency histogram;
- byte-size buckets;
- relay region and aggregate direct/relay ratio;
- storage/quota state bucket;
- first-party component name/version when components are enabled.

Never attach:

- message bodies or event payloads;
- room IDs;
- identity, device, or endpoint IDs, including shortened values;
- invite tickets or ticket hashes;
- peer IPs or private network addresses;
- filenames or local paths;
- WebSocket or control frames;
- complete request query strings;
- a stable cross-session telemetry identifier.

Operational constraints:

- CDN and relay providers necessarily observe source IPs. Treat access logs as
  sensitive.
- Aggregate metrics inside the service where possible.
- Retain raw security access logs for no more than 72 hours initially, with
  restricted access and documented incident exceptions.
- Disable query logging where the provider allows it.
- Scrub CSP reports of `document-uri`, query values, and code samples.
- Keep beta client telemetry opt-in and use a rotating, unlinkable session ID.
- Extend the existing privacy-safe diagnostics model, but remove shortened
  identity and network identifiers before automatic upload.

## Availability, backup boundaries, abuse controls, and cost

### Initial service objectives

| Surface | Initial objective |
|---|---|
| Static shell | 99.95 percent monthly availability |
| Relay authentication plus at least one relay | 99.9 percent monthly availability |
| Online event convergence | 99 percent of accepted chat events visible to both online peers within 10 seconds |
| Companion pairing | At least 99 percent success on the supported OS/browser matrix |
| Frontend rollback | At most 15 minutes |
| Relay regional failover | At most 2 minutes |
| Optional server-peer recovery | RTO at most 4 hours and RPO at most 5 minutes when enabled |

These are launch objectives to measure during beta. They are not guarantees
inherited from the current preview.

There is intentionally no service-availability claim when every room peer is
offline. An installed shell may open without the CDN, but synchronization and
file availability depend on active peers.

### Backup and recovery boundaries

- Static assets, configuration, component metadata, and infrastructure state
  are centrally backed up.
- Relays are stateless and need configuration recovery, not room-data backup.
- Jeliya infrastructure cannot restore a client-only identity or unique local
  file.
- A recovery bundle restores identity authority, not unavailable history or
  unreplicated blobs.
- An optional server peer improves availability but is a content-reading member
  under the current protocol.
- A blind store-and-forward service is blocked on encrypted event envelopes and
  group-key epochs.

### Abuse controls

- short-lived endpoint-bound relay tokens;
- per-IP and per-endpoint handshake, connection, byte, and rate limits;
- owner-enforced invitation creation and redemption limits;
- initially one pending invitation window per room;
- event, body, file, and per-room authoring limits;
- browser, component, and server-peer storage quotas;
- no arbitrary relay egress or generic TCP proxying;
- native-only, separately approved pipes and agents;
- user block/report tools with explicit content disclosure when reporting;
- synchronization and component circuit breakers.

### Initial monthly cost model

Current Iroh managed relay pricing starts at $0.27 per hour. Two continuously
running relays therefore start near $389 per 30-day month before bandwidth or
SLA charges.

| Item | Monthly starting estimate |
|---|---:|
| DNS and static CDN | $0 to $25 |
| Two managed Iroh relays | Approximately $389 before bandwidth/SLA |
| Relay-auth Worker | $0 to $25 |
| Artifact/component object storage | $0 to $10 initially |
| Privacy-reviewed monitoring | $0 to $150 |
| Initial fixed total | Approximately $400 to $600 plus relay bandwidth |

Self-hosted relays may reduce the direct infrastructure bill to roughly $50 to
$200 per month plus egress, but move availability and on-call cost to the team.
Browser peers are always relayed, so file traffic can dominate cost.

Measure relay cost as:

```text
monthly relay cost =
  relay instance-hours
  + relayed GiB * provider egress rate
  + managed support or SLA charges
```

Optional server-peer cost is compute-hours plus stored GiB-months, backup, and
egress. Do not publish per-user pricing until real room size, online time, and
file-transfer distributions are measured.

## Repository change map

| Existing or new area | Proposed responsibility |
|---|---|
| [`ui/src/lib/client.ts`](../ui/src/lib/client.ts) | Replace the production same-origin `/ws` assumption with transport interfaces and capability negotiation |
| [`ui/src/main.tsx`](../ui/src/main.tsx) | Run secure invite cleanup and capability bootstrap before React mounts |
| [`ui/public/site.webmanifest`](../ui/public/site.webmanifest) | Add a stable app ID, scope, shortcuts, and validated install metadata |
| `ui/src/sw.ts` | Versioned service worker and N/N-1 cache lifecycle |
| `ui/src/runtime/` | Companion and browser-peer client adapters |
| `ui/src/storage/` | IndexedDB/OPFS journal, quota, migration, integrity, and eviction handling |
| `ui/src/pairing/` | Browser control keys, SAS pairing, scopes, expiry, and revocation UI |
| `ui/src/invites/` | Fragment-only parsing, immediate URL cleanup, and redaction |
| [`crates/jeliya-core`](../crates/jeliya-core) | Split host-independent protocol/runtime behavior from native persistence and network assumptions |
| [`crates/jeliya-core/src/identity.rs`](../crates/jeliya-core/src/identity.rs) | Keystore abstraction, encrypted recovery, device authorization, and rotation |
| [`crates/jeliya-core/src/supervisor.rs`](../crates/jeliya-core/src/supervisor.rs) | Pluggable store/net/blob traits, idempotency, cursors, invite cancellation, and relay policy |
| [`crates/jeliyad`](../crates/jeliyad) | Remain a loopback-only legacy/local sidecar; never receive a public bind option |
| `crates/jeliya-protocol/` | Pure protocol-v2 types, canonical encoding, signatures, and conformance fixtures |
| `crates/jeliya-runtime/` | Host-independent engine over store, network, blob, clock, and key traits |
| `crates/jeliya-platform-native/` | SQLite, filesystem, native Iroh, and OS keystore adapters |
| `crates/jeliya-web/` | `wasm-bindgen`, IndexedDB/OPFS, browser signing, and browser Iroh adapters |
| `crates/jeliya-control/` | Pairing transcript, scoped RPC, nonce/counter replay protection, and revocation |
| `crates/jeliya-companion/` | Signed native service with Iroh control ALPN and no public HTTP listener |
| `crates/jeliya-components/` | Later signed package, WIT policy, quota, and native component host |
| `crates/jeliya-server-peer/` | Later explicitly invited availability or hosted-agent peer |
| `.github/workflows/web-ci.yml` | Browser, Wasm, PWA, eviction, and real-relay validation |
| `.github/workflows/web-deploy.yml` | Immutable staging deploy and protected same-artifact production promotion |
| `.github/workflows/companion-release.yml` | Signed/notarized native companion publication |
| `infra/` | OpenTofu for DNS, CDN, Worker, relay configuration, and environments |
| `docs/adr/` | Accepted decisions for hosting, identity, pairing, encryption, and server-peer trust |
| `docs/runbooks/` | Deployment, rollback, relay failure, key rotation, and incident procedures |

Portable Iroh Rooms storage, network, and blob interfaces should preferably land
upstream. A long-lived private fork is a security and maintenance liability.

## Dependency-ordered roadmap and gates

No phase starts implementation work that depends on an unresolved go/no-go gate
from the previous phase.

### Phase 0: freeze the claim boundary, 1 to 2 weeks

Deliver:

- reconcile status, threat, evidence, and platform documentation;
- pin the exact public Iroh Rooms revision `a5d98b70...`, recording why the
  first merge with both required fixes is used despite having no release tag;
- requalify provisional-peer fanout, connection-generation teardown,
  synchronization isolation, and store retry/degradation at that exact
  revision;
- select one exact clean candidate commit;
- accept or reject the hybrid architecture through an ADR;
- update the threat model for browser origin, companion, and relays;
- prove browser-to-native Iroh connectivity with the intended relay
  authentication;
- confirm DNS, CDN, relay, and signing ownership.

Go/no-go gate:

- no contradictory release claim remains;
- `Cargo.toml` and `Cargo.lock` both resolve Iroh Rooms `a5d98b70...`;
- the named upstream fanout, connection-generation, isolation, and
  store-degradation regressions pass at that revision, together with Jeliya's
  join/loopback suite;
- complete CI passes twice on one immutable SHA;
- direct and forced-relay evidence is signed and bound to that SHA and
  `a5d98b70...`;
- a browser reaches a native test endpoint through an authenticated relay;

### Phase 1: production identity and protocol primitives, 3 to 5 weeks

Deliver:

- recovery bundle and OS-keystore abstraction;
- `client_msg_id` idempotency;
- incremental timeline cursor;
- invite default expiry and cancellation;
- companion pairing/control protocol;
- protocol version and capability negotiation;
- surface upstream's durable critical `store_degraded` decision and define the
  operator response to exhausted store retries or queue overflow.

Go/no-go gate:

- recovery succeeds from a fresh install on every supported OS;
- native production mode no longer leaves the root secret plaintext;
- 10,000 injected lost-response retries produce no duplicate message;
- cursor resync matches full-log materialization;
- expired and cancelled tickets fail on every transport;
- replay, wrong-SAS, expired-key, and revoked-key pairing tests fail closed;
- independent security review approves the wire formats and key lifecycle.

### Phase 2: companion-backed vertical slice, 5 to 7 weeks

Deliver:

- `jeliya-companion` and PWA companion transport;
- scoped chat-only browser controller;
- signed macOS and Windows packages and a verified Linux package;
- recovery and re-pair user interfaces.

Go/no-go gate:

- the companion has no non-loopback TCP or HTTP control listener;
- 1,000 automated pairing/revocation cycles accept no unauthorized controller;
- two NAT-separated users can create identities, create a room, invite, join,
  chat, reconnect, and resynchronize;
- direct and deliberately forced-relay runs pass;
- a malicious controller cannot invoke files, pipes, agents, or identity reset;
- a 48-hour soak loses no committed event;
- supported installers verify signatures and reject tampering.

### Phase 3: production web and relay operations, 2 to 3 weeks

Deliver:

- DNS, TLS, CDN, CSP, and related headers;
- service worker and encrypted companion-view cache;
- two dedicated relays and the relay-auth service;
- staging/production promotion, smoke, and rollback;
- privacy-safe metrics and incident runbooks.

Go/no-go gate:

- external TLS/header/CSP assessment passes;
- invitations appear in no CDN, Worker, relay, or client diagnostic log;
- offline shell and cached view open during origin outage;
- N-to-N-1 rollback completes within 15 minutes;
- a regional relay outage fails over within 2 minutes;
- load tests stay inside resource and cost ceilings;
- an external penetration review has no unresolved critical or high finding.

This is the first production launch gate.

### Phase 4: browser peer and multi-device identity, 10 to 14 weeks

Deliver:

- browser event, blob, and sync adapters;
- Wasm signing and Iroh endpoint wrapper;
- root-signed device authorization and revocation;
- browser recovery and eviction handling;
- browser/native protocol conformance.

Go/no-go gate:

- the latest two Chrome, Edge, Firefox, and Safari releases plus current iOS
  Safari and Android Chrome pass the supported matrix;
- browser and native peers produce byte-compatible signatures and membership
  folds;
- forced-relay chat and file tests pass for browser/native combinations;
- clearing storage triggers recovery and never silent identity replacement;
- an active browser peer works offline and converges after reconnection;
- a revoked device cannot author an accepted future event;
- product copy makes no durable background-availability claim;
- the exact upstream/browser-adapter revision receives security qualification.

### Phase 5: components and optional server peers, 8 to 16 weeks

Deliver:

- signed component registry and WIT host;
- browser/native capability profiles;
- quotas, upgrade, rollback, and revocation;
- optional explicitly invited server peer;
- group-encryption design if blind storage is required.

Go/no-go gate:

- sandbox escape and confused-deputy review passes;
- a component cannot access a secret, file, room, network, process, or pipe
  without the corresponding import and grant;
- quota violation terminates cleanly without corrupting host state;
- rollback preserves prior component state and signed room history;
- the server-peer UI states precisely whether the server can read content;
- no blind-backup privacy claim is made before encrypted-envelope and key-epoch
  interoperability tests pass.

## Smallest production-worthy vertical slice

The first release at `https://app.jeliya.ai` includes:

- an installable static PWA;
- a signed local companion for the supported desktop platforms;
- local identity creation with a tested recovery kit;
- secure SAS-confirmed pairing with a scoped browser control key;
- room create, list, and open;
- identity-bound fragment invitations;
- join and text chat;
- idempotent sending;
- signed-event timeline, reconnect, and resync;
- truthful direct/relay status;
- encrypted cached room view and offline drafts;
- control-key revocation;
- production DNS, TLS, CSP, two relays, smoke tests, rollback, observability,
  and incident response.

It explicitly excludes:

- a browser-owned room identity;
- files;
- pipes;
- agents;
- third-party components;
- optional server peers;
- mobile background-availability claims;
- generic holder-bearer invitations.

This slice preserves the native local-first signed-event core and creates a safe
public entry point without treating the current browser or daemon boundaries as
capabilities they do not provide.

## Assumptions, unresolved decisions, and high-risk unknowns

### Planning assumptions

- The first supported production matrix is desktop-focused and is narrowed in
  Phase 0 before package work starts.
- The team can obtain Apple and Windows signing services and operate protected
  production environments.
- Dedicated relay service supports the required endpoint-bound short-lived
  credentials, or an equivalent self-hosted design is selected.
- No server availability peer is required for the first production slice.
- The product accepts that a hosted first-party origin can observe the content
  it renders and actions within its granted scope.

### Decisions that require an ADR

1. Final CDN, edge-token, relay, and infrastructure provider selection.
2. Companion control protocol and pairing transcript.
3. Recovery-bundle format, custody, and optional opaque hosting.
4. Multi-device and revocation event semantics.
5. Whether optional server peers may read content or require group encryption.
6. Browser signing strategy: nonextractable WebCrypto signer or wrapped Wasm
   seed.
7. Component package metadata, trust-root custody, and WIT world.
8. Supported browser, desktop OS, and mobile matrix.

### Highest-risk unknowns

1. Whether Iroh Rooms will accept and maintain the portable browser store,
   transport, and blob interfaces upstream.
2. Browser relay-auth token issuance and proof-of-possession behavior.
3. Multi-device compatibility with existing room membership history.
4. Browser-origin/CDN compromise and the maximum authority granted to a web
   controller.
5. Recovery usability and user custody for an accountless identity.
6. A tagged-release and maintenance path for the deliberately untagged
   `a5d98b70...` Iroh Rooms revision.
7. Native signing, notarization, SmartScreen, and Linux distribution timing.
8. Relay bandwidth economics for browser file transfer.
9. PWA storage behavior across real Safari/iOS and low-storage devices.
10. Exact-revision qualification of the final production candidate.

## Citations

- [Iroh: WebAssembly and browsers](https://docs.iroh.computer/languages/wasm-browser) - Browser build instructions, relay-only connections, end-to-end encryption, feature flags, and wrapper guidance.
- [Iroh: Use your own relay](https://docs.iroh.computer/add-a-relay) - Production relay guidance, authentication, stateless failover, and two-region recommendation.
- [Iroh hosting](https://www.iroh.computer/services/hosting) - Public-service limitations and current managed-relay starting price.
- [kortiene/iroh-room#121](https://github.com/kortiene/iroh-room/issues/121) and [iroh-room PR #125](https://github.com/kortiene/iroh-room/pull/125) - Provisional-peer fanout and handshake gating.
- [kortiene/iroh-room#119](https://github.com/kortiene/iroh-room/issues/119) and [iroh-room PR #132](https://github.com/kortiene/iroh-room/pull/132) - Store-insert retry, local hole healing, and fail-loud degradation.
- [MDN: Storage quotas and eviction criteria](https://developer.mozilla.org/en-US/docs/Web/API/Storage_API/Storage_quotas_and_eviction_criteria) - Best-effort and persistent browser storage and eviction boundaries.
- [MDN: Origin private file system](https://developer.mozilla.org/en-US/docs/Web/API/File_System_API/Origin_private_file_system) - OPFS availability, worker support, and origin-private storage behavior.
- [MDN: Offline and background operation](https://developer.mozilla.org/en-US/docs/Web/Progressive_web_apps/Guides/Offline_and_background_operation) - PWA offline and service-worker execution boundaries.
- [MDN: Background Synchronization API](https://developer.mozilla.org/en-US/docs/Web/API/Background_Synchronization_API) - Limited browser availability and deferred-work semantics.
- [MDN: URI fragment](https://developer.mozilla.org/en-US/docs/Web/URI/Reference/Fragment) - Fragment processing in the browser rather than the HTTP request.
- [MDN: Referer header](https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Headers/Referer) - Exclusion of URL fragments from Referer values.
- [MDN: SubtleCrypto sign](https://developer.mozilla.org/en-US/docs/Web/API/SubtleCrypto/sign) - Browser Ed25519 signing support.
- [MDN: SubtleCrypto unwrapKey](https://developer.mozilla.org/en-US/docs/Web/API/SubtleCrypto/unwrapKey) - Wrapped and nonextractable browser-key behavior.
- [MDN: loopback-network Permissions Policy](https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Headers/Permissions-Policy/loopback-network) - Experimental hosted-page loopback control.
- [WebAssembly Component Model: WIT worlds](https://component-model.bytecodealliance.org/design/worlds.html) - Import/export capability boundaries for components.
