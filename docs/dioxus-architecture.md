---
type: "Decision"
title: "Dioxus clean-slate architecture"
description: "Decision record for the clean-slate typed Rust client stack on Dioxus system-WebView rendering, the protocol and storage generation it defines, the single embedded web artifact every daemon target ships, and the retirement of React, Flutter, the Dart protocol package, and the C ABI."
tags: ["architecture", "clean-slate", "dioxus", "protocol", "release"]
timestamp: "2026-08-10T18:00:00Z"
status: "canonical"
implementation_status: "partial"
verification_status: "unverified"
release_status: "unreleased"
audience: ["contributors", "maintainers", "release-engineers"]
---

# Dioxus clean-slate architecture

**Status: DECIDED 2026-07-27. M1's typed-API slices have landed (the #165,
#166, and #233 remainders stay open); the M2 entry seam (#167), the M2
transport-independent kernel (#168), the M2 authoritative room/session
reconciler (#169), the M2 platform-authority boundary (#174), the M3 web
foundation (#176), the M3 CSS/l10n/a11y foundation (#177), the M3
bootstrap/shell/routing/preferences (#178), and the M4 daemon supervisor
(#170) are implemented.** Jeliya
replaces its two user-facing clients with one clean-slate typed Rust client
stack rendered by Dioxus 0.7 in the platform's system WebView, defines one
protocol and storage generation, and retires React, Flutter, the Dart
protocol package, the C ABI, and `jeliya-ffi`.

Read every statement below as a requirement the tree either already meets —
the milestones the status line marks implemented — or must still grow into.
The released `v0.6.x` line and the current source candidate still ship React
in `ui/`, Flutter in `app/`, and Dart transports in `dart/jeliya_protocol/`;
the workspace members are now `jeliya-core`, `jeliyad`, `jeliya-api`,
`jeliya-codec`, `jeliya-client`, and `jeliya-ui` (the shared Dioxus crate,
#176), with `jeliya-ffi` quarantined from the active build under #166 until
#202 deletes it. [Protocol v2](protocol-v2.md) is specified and the in-tree
daemon's wire protocol is v2-only (a v1-era browser `GET /api/session` token
handshake survives for the served UI pending the #166 remainder); [the v1
daemon protocol](PROTOCOL.md) remains the contract every **released** daemon
speaks.

Program: #156. This record satisfies #157 and records the canonical
architecture selected by the first-release distribution decision, #113. It
supersedes the [production deployment proposal](production-deployment.md).

## Why

Three recorded repository facts forced the decision.

**The protocol contract existed three times.** `jeliya-core` exposed 24 v1 RPC
methods. The authoritative method map was duplicated in
`ui/src/lib/protocol.ts`, Dart models lived in
`dart/jeliya_protocol/lib/src/models.dart`, and all 24 dispatch arms lived in
`crates/jeliya-core/src/engine.rs`. The workspace had no shared API crate, so
no compiler checked that the three agreed (#163 — the landed `jeliya-api`
crate now closes this; the React and Dart copies persist only until #200 and
#202 retire them).

**Every user-facing decision ships twice.** React and Flutter each need their
own implementation, their own fixtures, and a gate to hold them together —
[cross-client design tokens](design-tokens.md) maps every token to a React
custom property and a Flutter getter, and the
[room attention](room-attention.md) record keeps two mock clients aligned so
both render identical decisions. Every product decision pays that cost again.

**The prior deployment proposal was not adopted.** The
[production deployment proposal](production-deployment.md) argued for a
capability-aware hybrid — an installable PWA plus a native companion. It was
adversarially reviewed and then superseded by #113 rather than amended.

The backlog records this decision as scope and non-goals, not as a
comparative benchmark. **This record therefore claims no measured
performance, bundle-size, or ecosystem advantage** for Dioxus over the
retired stack. Its stated purpose is one typed contract, one client stack,
and one artifact.

## Decision 1 — one renderer, in the system WebView

Jeliya builds one clean-slate typed Rust client stack around **Dioxus 0.7
system-WebView rendering**. Experimental WGPU and Blitz renderers are
excluded (#156, #157, #176).

Rendering happens in the platform's system WebView. Process, network, and
file authority stay in native Rust, and RSX components receive typed state
and actions only (#184). Desktop Rust handlers run natively even though UI
rendering occurs in a WebView (#172). **No daemon token crosses into
untrusted WebView script** (#159).

This record makes no claim that the system WebView sandboxes native Rust.
Asserting that is an explicit non-goal of the security review that will
establish what the WebView boundary does and does not contain (#196).

## Decision 2 — one protocol and storage generation

The program's clean-slate policy, verbatim from #156:

> Jeliya is pre-release. Protocol v1 and all existing developer identities,
> rooms, preferences, and signed logs are disposable. Define one protocol-v2
> and storage generation. Do not implement v1 support, dual read/write,
> schema migration, identity continuity, or data-preserving rollback. Old
> clients and old data fail closed with an actionable reset path; no
> unverified directory is deleted automatically.

**How old clients fail.** `jeliyad` is v2-only and rejects unsupported
clients during the handshake, **before dispatch and before any mutation
executes** (#161, #164, #166). No legacy client may execute a v2 mutation
(#157). Stale processes and state generations fail explicitly; a
protocol or storage mismatch fails closed, and only a proven-owned incumbent
may be replaced (#156, #170).

**How old state is treated.** Each platform writes only the new namespaced
schema. Legacy keys are ignored or explicitly removed, never interpreted as
new state (#178); the legacy `app_prefs.json` is ignored and no migration
reader is implemented (#185); DirectClient opens only new-generation state
(#173). **No unverified directory is deleted automatically** — silently
deleting or reinterpreting an unverified old data directory is an explicit
non-goal (#156), and the Android beachhead fails closed on one (#160). The
reset path is shown to the user; it is not taken on their behalf.

**Browser preference namespace.** #178 established the browser key namespace
(`jeliya.dx.v1`) and the enumerated legacy-key purge (applied once at
`WebPlatform` construction; removes only keys in the closed allowlist, never
reads them as state). The desktop preferences store and its version key (#185)
and the Android data directory (#173) remain ahead. Each must be a name no
retiring client ever wrote.

**What does not remain.** No active v1 codec, legacy state reader, migration
path, dual artifact, or rollback package (#156, #164). No migration tooling,
dual readers or writers, N-1 packages, canaries, observation windows, or
mixed-version windows (#156, #113).

**Where JSON survives.** `jeliya-api` is the Iroh-free typed contract and
JSON exists **only at the WebSocket edge** (#156); public API types contain
no `serde_json::Value` (#163); no JSON or method-string dispatch escapes the
codec (#164). **Internal persistence JSON is unaffected** and remains out of
scope (#165).

**Protocol v2 is not specified here.** #161 owns the v2 handshake and version
gate, envelopes, approved operations, outputs, pushes, errors, authorization,
resource limits, mutation identity and retry guarantees, push ordering, gap
detection, and authoritative resync — together with a hand-authored,
independently written conformance corpus. The 24 v1 methods are inventory,
not authority: v2 may retain, rename, combine, or remove operations
deliberately, and no requirement preserves a v1 field, event, null, or
storage shape.

**Retained scenarios, re-expressed in v2 terms against fresh state.** Their
v1 bytes and persistence shapes are not retained (#161, #162, #175, #195):
late join after multi-author history (#46); an expired ticket replaced by a
fresh ticket for the same identity (#47); multiple rooms live at once without
routing or push loss (#147); provider availability and Pipe reachability as
authoritative protocol facts rather than inferences from membership display
state (#50, #94); presence and liveness across rooms and network paths (#79);
departed rooms opened as local read-only historical archives (#91).

## Decision 3 — layering, owners, and allowed dependency direction

Owners are roles, not people. `core maintainer`, `desktop maintainer`, and
`release maintainer` are the vocabulary the
[known gaps and roadmap](known-gaps-roadmap.md) already used; `web
maintainer`, `mobile maintainer`, and `cross-platform maintainers` are
introduced by this record and adopted by that page in the same change. No
issue assigns owners, so these roles are this record's own allocation.

| Crate or module | Disposition | Owner | Must not depend on |
|---|---|---|---|
| `crates/jeliya-api` | new — typed requests with paired outputs, pushes, errors, and view models for every approved v2 operation | core maintainer | Iroh, WebSocket, Dioxus, or any platform crate; no `serde_json::Value` in public types |
| protocol-v2 codec | new — a dedicated crate or tightly isolated module with exhaustive request routing | core maintainer | anything that would let JSON or method-string dispatch escape it |
| `crates/jeliya-core` | retained, retyped — protocol-facing materializer and supervisor signatures stop returning `serde_json::Value` | core maintainer | envelope or push framing, which moves out of core |
| `jeliyad` | retained — v2-only, keeps its process, token, lock, and ownership safety invariants | core maintainer | any path that admits an unsupported client past the handshake |
| `crates/jeliya-supervisor` | landed — one reviewed Rust supervisor for Dioxus desktop and other native control planes, owned or adopted (`Supervisor`/`Sidecar`/`TargetResolver`/`DialTarget`); binary/data-dir resolution fail-closed; fresh portfile re-validation on every reconnect; bounded process-tree-safe escalation | desktop maintainer | UI state; it owns spawn and stop, transports do not; never reachable from a `wasm32` build |
| client kernel and seam | new — one cloneable UI-facing handle over a transport-independent kernel | core maintainer | a specific transport; backend erasure stays internal |
| `jeliya-ui` | new — the shared UI crate, with Dioxus and `dx` pinned | web maintainer | platform authority; it reaches it only through injected services |
| `PlatformServices` | new — one injectable boundary for files, persistence, lifecycle, URLs, clipboard and share, navigation, and window actions | cross-platform maintainers | nothing; every service has a deterministic test implementation |
| package identity | new — one reserved application or bundle identifier per packaged target | release maintainer | any identifier a retiring client already ships, which would let a legacy install upgrade into the new generation |
| `crates/jeliya-ffi` | to be removed — still present on disk and still the Flutter Android transport, but no longer built or tested by the workspace; quarantined from the active build under #166, deleted under #202 | mobile maintainer | — |

Direction rules that hold across the stack:

- Browser WASM must stay free of Iroh and native dependencies; `jeliya-api`
  must compile for `wasm32-unknown-unknown`, and CI must assert its
  dependency tree now that the crate exists (#157, #163, #171).
- Iroh types are not moved into `jeliya-api`. Conversion happens at explicit
  module boundaries (#165).
- Shared components contain no platform business-logic `cfg` forks (#174).
- `ClientHandle` and `PlatformServices` are injected separately (#174).

## Decision 4 — one seam, four adapters, one platform boundary

**The seam** (#167) is one cloneable concrete UI handle, preferred over an
object-unsafe generic trait, keeping backend erasure internal. It models
`Push`, `StateChanged`, `Gap`, and `ResyncRequired`. Calls are compile-time
paired with their outputs; multiple consumers cannot silently steal each
other's pushes; stop settles all accepted work and closes event streams.

**The kernel below it** (#168) is transport-independent: queues are bounded
and `QueueFull` is visible rather than absorbed; connection loss
distinguishes never-sent work from work that may have executed; only
operations with an explicit, tested v2 deduplication guarantee may replay,
and everything else never auto-replays; generations are fenced.

**One resync path** (#169): `ResyncRequired { generation, reason }` is the
only gap and resync path for v2 clients. There is no legacy bootstrap
fallback.

| Adapter | Platform | Owner | Binding | Honest lifecycle difference |
|---|---|---|---|---|
| deterministic mock | all | core maintainer | in-process fixture, shipped with the seam | none; it is the reference behavior |
| `WsWeb` | browser | web maintainer | browser WebSocket and fetch, fresh `/api/session` authentication on every attempt | connected is emitted only after protocol validation; it is the sole browser client, and React transport compatibility is not required |
| `WsNative` | desktop | desktop maintainer | native async WebSocket through the reusable supervisor and target resolver | the resolver runs on every connection attempt, only verified loopback endpoints are dialed, tokens stay native and redacted; it does not own spawn or stop, and Dart behavior is not retained |
| `DirectClient` | Android | mobile maintainer | typed `jeliya-core` in-process behind one bounded serialized actor | calls execute serially and one owner controls a canonical data directory; the path contains no JSON, Dart, C ABI, socket, token, or portfile; resume triggers authoritative resync **without a fabricated reconnect** |

Pretending `DirectClient` reconnects is an explicit non-goal (#173). One
fault-injected suite must prove all four expose the same view-level contract
while retaining honest transport-specific lifecycle differences (#175). No
such suite exists; of the four adapters only the deterministic mock has
landed (shipped with the seam, #167) — `WsWeb`, `WsNative`, and
`DirectClient` do not yet exist.

**`PlatformServices`** (#174) keeps platform authority out of shared RSX
components through one injectable boundary covering files, persistence,
lifecycle, URLs, clipboard and share, navigation, and window actions. Local
file paths and `content://` URIs are not interchangeable. The contract lives in
its own crate, `crates/jeliya-platform` — a cloneable facade over object-safe
capability traits, a closed outcome taxonomy (`Unavailable`/`Denied`/
`Cancelled`/typed failures, so a cancellation never becomes success), safe
path/URL types, and deterministic browser/desktop/Android fakes. `jeliya-ui`
re-exports it; target implementations follow in M3–M5, reaching the crate's
path-free construction factories only through the one door crate
`crates/jeliya-platform-implementation` (a Cargo feature unifies across a
build graph and so cannot hold that boundary; a dependency edge can).

## Decision 5 — one embedded artifact

The web build must produce **one** reproducible, content-addressed Dioxus
artifact, and **the exact same bytes** must be embedded in every daemon
target (#183). Its sealed manifest must carry the renderer, source SHA,
toolchain versions, and digest, and consumption of a legacy artifact must
fail. The same sealed manifest also carries pre-compressed `br` and `gzip`
variants of each compressible asset, each with its own digest, served by static
content negotiation, with the content-address kept over the uncompressed bytes
(see [how the embedded UI artifact is compressed on the wire](ui-artifact-wire-encoding.md));
#183 stays the owner of that schema. **No React or renderer rollback artifact may
be produced under this architecture.** Today the in-tree build embeds the
reproducible Dioxus artifact (`crates/jeliya-ui/dist`, #176) and fails closed
on React output; the React `ui/dist` archive that `v0.6.0` published remains
the **shipped** artifact until the release-line cutover (#200).

The delivery shape is fixed by
[the first-release distribution boundary](first-release-distribution.md)
(#113): the artifact is embedded and served by the trusted local `jeliyad` path
for browser use, and reused inside packaged desktop system WebViews. There is no
hosted-origin controller, native companion pairing, browser-resident room peer,
service worker, or browser-owned identity in the first release. That record also
settles how a browser with no native mediator authenticates — an operator-pasted
pairing code, never a ticket in the launch URL.

## Decision 6 — per-platform system WebView

| Platform | WebView | Required floor or policy | Issue |
|---|---|---|---|
| Linux | WebKitGTK | development and runtime dependencies must be pinned, the actual linked system libraries must be recorded in package evidence, and minimum glibc and WebKitGTK floors must be enforced | #187 |
| Windows | WebView2 | supported Windows versions and the evergreen or fixed-runtime policy must be recorded; navigation, storage, and devtools policy must be set; absent, outdated, and current runtimes must all be exercised | #188 |
| macOS | system WebView (WebKit) | **no floor decided** — a supported macOS and WebKit baseline must be chosen before qualification, not waived; nested native and WebView artifacts must be signed before the outer app and DMG | #186 |
| Android | system WebView | **none decided** — WebView version is captured as device evidence only, and no floor or evergreen policy exists | #160, #194 |

Nothing in that table is pinned, recorded, enforced, or exercised today; each
row states what its issue must deliver. The macOS and Android rows record open
decisions, not permissions: neither platform may qualify by treating an absent
floor as an absent requirement.

The decision names Linux and Windows precisely and names macOS only as the
system WebView. No narrower platform API is approved, and the Android row is
an open gap, not an omission.

**Windows is not yet a committed first-release target.** #188 must
explicitly include *or formally defer* Windows, and the desktop
qualification matrix is blocked on that answer if Windows remains in scope.
Treating Windows as supported because `dx bundle` runs is an explicit
non-goal (#188, #189).

One release-blocking desktop matrix (#189) covers supported OS and WebView
versions, navigation policy, daemon lifecycle, files and preferences, and
keyboard and screen-reader behavior. Navigation, new-window, download,
devtools, and storage policies **fail closed**. Its results, and the
accessibility and localization evidence beside them, are described as
**enforced evidence, not certification** (#189, #197).

There is no all-platform release barrier: a missing platform-specific gate
blocks only that platform's publication row (#199).

## Decision 7 — the boundaries that must stay explicit

| Boundary | Rule | Issue |
|---|---|---|
| artifact | one content-addressed Dioxus artifact, identical bytes in every target; legacy artifact consumption fails; the sealed manifest also carries pre-compressed `br`/`gzip` variants served by static negotiation, canonical digest unchanged ([wire encoding](ui-artifact-wire-encoding.md)) | #183 |
| daemon token | stays native, never crosses into untrusted WebView script, and is redacted in logs and diagnostics | #159, #170, #172, #184 |
| storage | one new namespaced generation per platform; legacy keys are ignored or removed, never interpreted as new state; Android state is app-private and backup-excluded | #173, #178, #185, #190 |
| navigation | navigation, new-window, download, devtools, and storage policies fail closed in the packaged WebView | #189, #196 |
| native capability | reaches shared components only through injected `PlatformServices`, never through a `cfg` fork in shared RSX | #174 |

## Feature graphs

**Browser.** The daemon serves the artifact it embeds; the browser client
holds no Iroh dependency and no identity of its own.

```
jeliya-ui (Dioxus, wasm32) -> WsWeb -> WebSocket + /api/session
  -> jeliyad (v2-only) -> jeliya-core -> iroh-rooms
```

**Desktop.** The packaged app renders the same artifact in the system
WebView; native Rust keeps process and network authority.

```
packaged app -> system WebView renders jeliya-ui
  -> native Rust handlers + PlatformServices
  -> WsNative -> supervised jeliyad (owned or adopted) -> jeliya-core
```

**Android.** One process, no socket.

```
system WebView renders jeliya-ui -> DirectClient (bounded serial actor)
  -> typed jeliya-core in-process
```

**Current agent.** No dual codec; the agent speaks v2 or it does not connect.

```
OpenCode agent (v2 client) -> WebSocket -> jeliyad (v2-only) -> jeliya-core
```

The cutover is tracked cross-repo as
[kortiene/jeliya-opencode-agent#45](https://github.com/kortiene/jeliya-opencode-agent/issues/45).
The full Rust OpenCode or Switchyard agent rewrite is out of scope, and agent
execution does not move into `jeliyad` or the human-facing app (#156).

## Measured unknowns and their spikes

The decision is made; these measurements are not.

| Unknown | Spike | State |
|---|---|---|
| Whether an embedded Dioxus web build serves and functions against a real `jeliyad` | #158 | **measured 2026-07-28 — it does**; see below |
| Whether native WebSocket supervision survives inside a packaged system WebView | #159 | not measured |
| Whether AAB packaging, the DirectClient beachhead, and device UX hold on a physical device | #160 | not measured |
| Absolute first-release bundle, startup, memory, timeline, battery, and network budgets | #198 | not measured |

**What #158 established.** A throwaway Dioxus 0.7.9 slice bootstrapped, listed
and opened a room, sent a message, and rendered the daemon's own `room.event`
push against a real supervised `jeliyad` on a fresh data dir — from a
development directory and from assets compiled into the daemon by `embed-ui`.
Its `wasm32-unknown-unknown` graph resolved 124 crates with no Iroh, no
`jeliya-core`, and no native transport, so Decision 3's browser rule is
achievable rather than merely intended. `ui/src/styles.css` drove the rendered
markup **byte-identical**, which says the design system's CSS survives the
renderer swap; it says nothing about what enforces the tokens once that file
retires (#177).

The spike is disposable and none of it is promoted: it speaks protocol v1
because that is what exists, its wire structs must not be lifted into
`jeliya-api`, and it implements no reconnect, no backoff, and no queueing —
those are #168's semantics and inventing them in a spike would be exactly the
assumption this record refuses. Its measurements are one machine, one debug
daemon, and no `wasm-opt`; they inform #198 and do not constrain it. Evidence
and caveats: `spikes/dioxus-web/README.md`.

One finding belongs to other issues rather than to this record: the daemon
serves embedded assets with no content encoding, ignoring `Accept-Encoding`.
That costs nothing on loopback and is a real input to #183 and #113 wherever
the same artifact travels further, since compressing this spike's wasm alone
removed 61% of it. How that artifact is encoded on the wire — seal Brotli and
gzip variants in the sealed manifest and negotiate statically, with the
canonical digest kept over the uncompressed bytes — is now decided in
[how the embedded UI artifact is compressed on the wire](ui-artifact-wire-encoding.md).

**What #176 landed (the M3 web foundation).** The production `crates/jeliya-ui`
crate now exists as a workspace member, with `dioxus` pinned (`=0.7.9`) and the
renderer kept optional and feature-gated so the default and MSRV `--workspace`
builds carry no Dioxus and no OpenSSL. It consumes `jeliya-api` view models,
`jeliya-client::ClientHandle` (driven by the deterministic mock — it opens no
socket), an injected `PlatformServices` seam, and the canonical `ui/src/styles.css`,
composing per target only at the crate root. The one reproducible artifact
(`crates/jeliya-ui/dist`) is built by `scripts/build-web.sh` and byte-identical
across two clean builds; its wasm graph excludes Iroh and every native crate at
the lockfile level; the daemon embeds it through `embed-ui` behind a build-time
guard that fails closed on React/Vite output. The reproducible-build contract,
pinned versions, and commands are `docs/dioxus-web-build.md`. Two boundaries
stay honest here: `jeliya-ui`'s `PlatformServices` is now the **canonical
contract from `crates/jeliya-platform` (#174)**, re-exported by `jeliya-ui` and
backed by a deterministic fake shape (the former provisional local seam is
deleted), and #176 does
**not** remove React or flip the tagged-release line — the sealed content-addressed
manifest is #183, the Dioxus-side token gate is #177, and React removal / the
release-line cutover is #200. `ui/` and its per-client gates stay intact until
then (Decision 5).

Two further open questions were decisions rather than measurements, and
neither waited on a spike. #92 selected the v2 shared-file maximum in M0 so
that #161 could specify the protocol — [protocol v2](protocol-v2.md) now
fixes `max_shared_file_bytes` from
[the shared-file size policy](shared-file-size.md).
#188 decides whether Windows is in first-release scope. Both are listed under
[what this record does not decide](#what-this-record-does-not-decide).

## Rejected and deferred alternatives

- **WGPU and Blitz rendering.** Excluded as experimental (#156, #157, #176).
- **A hosted origin, service worker, delegated browser controller,
  browser-resident room peer, native companion pairing, or browser-owned
  identity.** Excluded from the first release (#113). This is the substance
  of the superseded proposal.
- **Any compatibility or rollback affordance.** Migration tooling, dual
  readers or writers, N-1 packages, canaries, observation windows, rollback
  artifacts, and mixed-version windows are all out of scope (#156, #113).
- **Keeping React or Flutter runnable.** They are requirements-mining
  sources only — neither parity nor compatibility authorities — and pixel
  identity is a non-goal (#156, #162).
- **iOS.** Out of scope in #156, #113, and #157.

**A future hosted or delegated browser architecture requires a new decision
record, a new threat model, and a separately approved backlog** (#113,
#157). This record does not authorize one.

## What this record does not decide

- **Protocol v2 itself**, or its conformance corpus — #161.
- **The required cross-platform product behavior contract** — #162. React
  and Flutter tests, closed issue #77, and the
  [Room Workbench](room-workbench.md) record are requirements-mining input
  to it, not parity or compatibility authorities.
- **The v2 shared-file maximum** — #92.
- **Whether Windows ships in the first release** — #188.
- **Performance budgets** — #198.
- **Legal, privacy, and compliance gates for public distribution** — #118.
- **Native update channels, signing trust, and anti-rollback policy** —
  #121, now decided in the
  [native update, signing, and anti-rollback policy](native-update-policy.md).
- **What replaces the retired cross-client design-token, localization, and
  accessibility gates** — #177, #197. Retiring React and Flutter removes
  working enforcement before its replacement exists; that verification loss
  is recorded in [known gaps and roadmap](known-gaps-roadmap.md).

## Implementation

Every milestone carries its own exit gate. Landed issues are noted in the
slice table; all others remain ahead of the repository.

| Milestone | Exit gate |
|---|---|
| M0 — Architecture and platform feasibility | the clean-slate system-WebView architecture, fresh-state/reset policy, required product behavior ([product behavior contract](product-behavior-contract.md)), and web/desktop/Android feasibility evidence are recorded |
| M1 — Typed API and protocol v2 | one Iroh-free typed API and protocol-v2 contract drive a typed core and v2-only daemon; v1 clients fail before mutation and no public JSON or compatibility facade remains |
| M2 — Client runtime and platform adapters | bounded lifecycle-aware runtime, WsWeb, WsNative, DirectClient, PlatformServices, shared adapter tests, and the current OpenCode agent v2 cutover are complete |
| M3 — Web replacement | Dioxus web covers the required Room Workbench and global flows, passes its Playwright/real-daemon matrix, and produces the sole reproducible embedded UI artifact |
| M4 — Desktop lifecycle and packaging | clean-install macOS, Linux, and approved Windows Dioxus packages enforce daemon ownership/auth/shutdown, fresh storage, platform services, and system-WebView qualification |
| M5 — Android clean-install qualification | a signed clean install creates protected fresh state, runs DirectClient, and passes physical-device lifecycle, networking, files, accessibility, ABI, and backup-exclusion gates |
| M6 — First-release qualification | required behavior, security, accessibility, localization, absolute performance budgets, reproducible artifacts, and clean-install smoke tests pass independently per platform |
| M7 — Clean-slate legacy removal | React, Flutter, Dart protocol, C ABI, FFI, v1 fixtures, migration code, and rollback artifacts are gone; current-only builds, packages, tests, scans, and docs pass from a clean checkout |

The slices that carry this record:

| Issue | Slice | Status |
|---|---|---|
| #157 | This record, and the superseded pre-Dioxus proposal. | Landed |
| #113 | The first-release distribution decision it records. | Landed |
| #161 | Protocol v2 and its independently authored conformance corpus. | Landed |
| #162 | The required cross-platform product behavior contract — recorded as the [product behavior contract](product-behavior-contract.md). | Landed |
| #163 | The Iroh-free `jeliya-api` contract. | Landed |
| #167 | The lifecycle-aware client seam (`crates/jeliya-client`) and its deterministic mock. | Landed |
| #176 | The shared `jeliya-ui` crate: pinned Dioxus 0.7, reproducible wasm artifact, daemon embed guard, and Iroh-free dependency graph. Documented in [Dioxus web build and reproducibility](dioxus-web-build.md). | Landed |
| #174 | Injectable `PlatformServices` — the contract crate (`crates/jeliya-platform`), its deterministic browser/desktop/Android fakes, and the `crates/jeliya-platform-implementation` door crate that gates the construction factories; `jeliya-ui` re-exports the contract. Target implementations follow in M3–M5. | Landed |
| #168 | The bounded, lifecycle-aware client kernel (`crates/jeliya-client/src/kernel/`): the transport-independent sans-IO state machine that gives the seam its request/reply machinery. Key properties: bounded admission (`KernelLimits`: queue depth + bytes), exactly-once in-flight settlement, replay only where the protocol guarantees it (four gates, default off), honest `Unknown`/`DefinitelyNot` post-send uncertainty, local-cancel-only drop (no fabricated remote cancel), monotonic connection generation fencing, capped jittered backoff to an honest `State::Failed`, and total stop. New public types: `KernelLimits`, `KernelConfig`, `TickDelta`; the in-memory test driver ships behind `test-transport`. | Landed |
| #169 | The authoritative room/session reconciler (`crates/jeliya-client/src/reconcile/`): the transport-independent coordinator that sits *above* the seam and makes every detectable push gap, reconnect, local fan-out overflow, and Android process-resume produce the same bounded authoritative re-baseline. Sans-IO core (`core.rs`) + async driver (`driver.rs`); no wall clock, no RNG, no spawns, no new runtime dep. Key guarantees: observable `ResyncReason` emitted before each re-baseline (bootstrap, reconnect, gap, local overflow, daemon-forced, resume); single-flight coalesced reconciliation per room (≤ 1 in-flight + 1 queued); overflow forces a fresh authoritative read, never silently consumes the dropped push; convergence by `pos`/`event_id`/signed `at`; peer state replaced wholesale from authoritative reads; reconciler-local epoch fence discards stale baselines; `Reconciler::resume` triggers the identical outcome without fabricating a socket reconnect (`DirectClient`, §R11). All retained external-input collections are count/byte bounded: identifiers, backend-returned serialized outputs (byte/token/depth gated before typed decode; transports separately cap frame accumulation), decoded pages, position-aware durable plus all-page duplicate evidence, live event/peer buffers, rendered timelines, member/peer snapshots, control ingress, and per-subscriber updates; backend reads have a global concurrency cap. Oversized authority is rejected rather than truncated, mailbox loss is observable before retained later work, one shared latest-authority allowance prevents permanent Lagged-only delivery, and run cancellation/last-owner drop closes subscribers through RAII. Convergence is additionally gated: a trigger-named position above the watermark blocks publication until authority serves through it (bounded chase, then fail-closed park); `resync_required` redirect chains require strict bounded progress; a rollback that empties the rendered window rebuilds via full replacement; lowering truncations force an authoritative roster replacement; disputed positions are re-read from before the dispute; dropped peer pushes stay generation-fenced with deterministic saturated tombstone retention; outranked overflow counts surface as an attributed `Lagged` boundary before the covering view; and the driver bounds push-before-reply priority with a fairness budget. New public types: `Reconciler`, `ReconcileConfig`, `ReconcileLimits`, `ResyncReason`, `ResyncRequired`, `RoomView`, `RoomUpdate`, `RoomUpdateSubscription`, `ReconcileError`. The seam's call/lifecycle behavior and kernel semantics are unchanged; a malformed push with a recoverable room id can use the crate-private adapter/reconciler recovery command without changing `ClientEvent`; uncorrelated malformed frames remain the kernel's K4 drop. | Landed |
| #170 | The reusable owned/adopted `jeliyad` supervisor (`crates/jeliya-supervisor`): headless and native-only; resolves binary and data dir fail-closed; spawns-or-adopts with `Owned`/`Adopted` split; validates ready/portfile/`/api/health` agreement (PID-on-port + `protocol` + `storage_generation`); hands transports a freshly re-validated `DialTarget` (loopback WS URL with generation-gate query + redacted bearer token) on every reconnect; stops only owned daemons with bounded process-tree-safe escalation; never signals an unproven PID; never reachable from a `wasm32` build. Prerequisite of `WsNative` (#172) and M4 desktop packaging. | Landed |
| #177 | The CSS/l10n/a11y foundation (`crates/jeliya-ui/src/l10n/`): typed-Rust catalog with compiler-enforced EN/FR key and placeholder parity (`En`/`Fr` implement a single `Catalog` trait; `rustc` enforces agreement); the wire/error display seam; the identity-palette token source; formatting locale independence from day one. Node-side gates (empty value, `fr==en`, French typography, literal scan) apply unchanged. | Landed |
| #178 | The M3 browser shell, routing, and preferences (`crates/jeliya-ui/src/shell/`, `prefs/`, `platform_web.rs`, six new components). Host-testable pure modules: `Shell`/`shell_for` with fractional breakpoints, the router (`use_route` + canonicalization + fail-safe + last-room restore), the bootstrap/onboarding state machine, the preference schema (`jeliya.dx.v1` namespace, versioned envelope, corrupt/unsupported-version recovery, `reset_all`). Browser bindings: `WebNavigation` (History API pushState/replaceState/popstate), `WebLifecycle` (browser events → `LifecycleBus`), `WebPreferences` (session-scoped in-memory schema + boot-time legacy purge), `WebSecretStore` (tab-scoped in-memory), assembled as `WebPlatform`. New components: `GlobalNav`, `RoomShell`, `Fleet`, `Settings`, `Onboarding`, `Recovery`. Additive `Navigation::navigate_replace` (defaulted; `jeliya-platform` fake records it). The `ClientHandle` stays the deterministic mock until `WsWeb` (#171) slots in. | Landed |
| #183 | The one content-addressed embedded artifact. | Planned |
| #189 | The system-WebView security, lifecycle, and accessibility matrix. | Planned |

**Nothing is retired before its replacement is qualified.** React is removed
only after the Dioxus web release candidate passes (#200); Flutter desktop
only after packaged Dioxus qualification (#201); Flutter Android, the Dart
protocol, the C ABI, and `jeliya-ffi` are removed atomically only after the
clean-install DirectClient candidate passes (#202). Repository-wide
documentation, CI, packaging, and license consolidation follows in #203.

Each slice tests against this record. Where an implementation and this
document disagree, one of them is a bug — say which in the pull request.
