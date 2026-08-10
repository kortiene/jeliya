---
type: "Decision"
title: "First-release distribution boundary"
description: "Decision record for how the first release is delivered: one content-addressed Dioxus artifact served by the trusted local daemon and embedded in packaged desktop targets, the trust boundary each surface sits on, the operator-pasted pairing code that authenticates an ordinary browser, and the hosted-origin and delegated-browser work deferred behind a new architecture decision."
tags: ["architecture", "clean-slate", "dioxus", "release", "security"]
timestamp: "2026-08-10T00:00:00Z"
status: "canonical"
implementation_status: "planned"
verification_status: "unverified"
release_status: "unreleased"
audience: ["contributors", "maintainers", "release-engineers", "security-reviewers"]
---

# First-release distribution boundary

**Nothing in this record is built.** It satisfies #113 and fixes the delivery
shape [the architecture](dioxus-architecture.md) defers to it. #183 and #199
produce and qualify the artifact; #171 implements the browser session adapter;
#196 reviews the WebView boundary.

It replaces the [production deployment proposal](production-deployment.md),
which predates the Dioxus program and assumed React, a CDN PWA, a separately
installed companion, browser-resident peer storage, iOS, and mixed-version
rollout. That proposal was never accepted; it and
[its review](production-deployment-review.md) are retained as history.

## The decision

**One artifact, two delivery paths, one trust boundary.**

| Surface | How it is delivered | Who holds the daemon token | In the first release |
|---|---|---|---|
| Packaged desktop | The same artifact bytes, embedded in the app, rendered in the system WebView | the native process | **yes** |
| Ordinary browser | The same artifact bytes, served by the local `jeliyad` over loopback | the daemon; the page holds only a tab-scoped session and the tickets it draws | **yes**, via a pairing code |
| Hosted origin (`app.jeliya.ai`) | — | — | **no** |
| Native companion pairing | — | — | **no** |
| Browser-resident room peer | — | — | **no** |
| Service worker / PWA install | — | — | **no** |
| Browser-owned identity | — | — | **no** |
| iOS | — | — | **no** |

The first release ships **no server**. Every byte a user's browser renders came
from a daemon on their own machine, and the only network the product uses is the
peer-to-peer room transport that
[protocol v2](protocol-v2.md) already governs.

## Why an ordinary browser is in scope at all

It would be simpler to ship only the packaged desktop app. The browser path is
retained for one reason: **it is the only surface that needs no installer**, and
a local-first tool that cannot be tried without installing something loses the
audience most likely to evaluate it. The cost of keeping it is one credential
mechanism, specified below, and no server.

It is retained on the explicit condition that it introduces **no hosted origin
and no browser-owned state**. A browser tab is a renderer for a local daemon,
never a participant. That is what keeps this a delivery decision rather than an
architecture change.

## Trust boundaries

Each row names what is trusted, what is not, and the control that separates
them. These extend
[the threat model](security-threat-model.md#trust-boundaries) rather than
restating it.

| Boundary | Trusted side | Untrusted side | Control |
|---|---|---|---|
| Artifact origin | one content-addressed artifact, identical bytes in every target, served by the local `jeliyad` or embedded in a packaged app | any other origin, any cached or legacy artifact, any renderer rollback bundle | exact-digest match; consumption of a legacy artifact **fails closed** |
| Daemon token custody | the native process, and the `0600` portfile | WebView script, page storage, a URL, argv, logs, diagnostics | the token never leaves native memory; a browser receives only a tab-scoped session credential and the single-use tickets it draws |
| Pairing code | the terminal that started the daemon, and the operator reading it | every other local process and every other user | short TTL, single use, constant-time comparison, and an attempt budget bound **per code** so reconnecting cannot reset it |
| Storage | one namespaced protocol-v2 generation, owned by the daemon | legacy keys, `app_prefs.json`, unverified old directories, browser storage | the browser stores **nothing** that survives the tab; no unverified directory is deleted automatically |
| Navigation | the artifact's own routes | any external URL, any new window, any download, devtools in release | navigation, new-window, download, and devtools policies fail closed in the packaged WebView (#196) |
| Native capability | injected `PlatformServices` | shared components and anything executing in the WebView | platform authority reaches components only through the injected boundary; a browser gets the boundary's **web** implementation, not a native one |

The last row is the one most easily got wrong. A browser tab and a packaged
WebView render **the same components**, so the difference between them must live
entirely in which `PlatformServices` implementation is injected — never in a
`cfg` fork inside shared code, and never in a capability a component assumes it
has.

## The browser credential path

A page cannot authenticate itself. Something that already holds the daemon token
must obtain a ticket and hand it over, and in a packaged shell the native process
does exactly that. For an ordinary browser there is no such mediator, so the
operator becomes one.

**The daemon prints a pairing code; the operator pastes it into the page.** The
page exchanges it at `POST /api/session` for a tab-scoped **session credential**
and a first connect ticket; the daemon burns the code, and the page draws each
later ticket from the session.
[Protocol v2](protocol-v2.md#the-credential-never-travels-in-a-url-and-never-in-script)
specifies both requests and their rejections.

The session exists because a connect ticket is single-use and every WebSocket
connection needs a fresh one — so a code that yielded only a ticket would strand
the operator at the first page reload, with no way to obtain another short of
restarting the daemon. It lives in `sessionStorage`, dies with the tab, holds no
key material, and is not an identity: the browser still stores nothing that
outlives the tab, which is the property this record actually requires.

### Why not put a ticket in the launch URL

Because it is measurably unsafe on the platform we ship to. Having the launcher
mint a ticket and open `http://127.0.0.1:PORT/?ct=…` is frictionless and wrong:

- `/proc/<pid>/cmdline` is mode **`-r--r--r--`** on a default Linux install, and
  `hidepid` is not set. Verified on the development host: every sampled process,
  including root's, is world-readable.
- So **any local user can read another process's full argument vector** — and
  therefore a ticket handed to a browser on its command line.
- That is a user who cannot read the `0600` portfile, which makes it a
  **cross-user privilege escalation**: precisely the failure protocol v2 already
  rejected once, when an earlier draft gated ticket issuance on loopback headers
  alone.

Browser history is the same objection in a second place — `replaceState` cannot
retract a URL already committed to a profile, and session restore may replay it.
A terminal carries neither exposure.

The pairing code is not free of risk: it is a bearer secret with a small
alphabet, and a local page on another port could ask the operator to paste it.
That is the residual risk of every pairing flow, it requires the operator to act
against an instruction they were just given, and it is strictly smaller than
handing the same secret to every process on the machine.

**The guessing budget is bound to the code, not to the connection.** A hostile
local process can open a fresh connection per attempt, so a per-connection limit
is no limit at all — it turns a short code into an offline search performed
online. `pairing_code_max_attempts` failures void the code regardless of who
submitted them or across how many connections.

## The embedded artifact

Owned by #183, qualified by #199. This record fixes only what the distribution
boundary requires of it.

- **One artifact.** The bytes a packaged app embeds and the bytes a daemon
  serves are identical. There is no per-surface build.
- **Content-addressed, with recorded provenance** — source revision, toolchain
  versions, and digest.
- **Exact-version rejection.** A daemon consumes only the artifact digest it was
  built against. A mismatch **fails closed** with the reset path shown; it does
  not fall back, and it does not serve what it has.
- **No renderer rollback artifact.** Producing a React or Flutter bundle under
  this program is forbidden — a rollback path is a second supported generation,
  which the clean-slate policy does not have.
- **Cache behavior is the daemon's to state.** Because the artifact is
  content-addressed and served from loopback, it is immutable per digest and may
  be cached indefinitely by hash; the entry point must not be, or a stale shell
  outlives its own generation.

## Deferred behind a new architecture decision

None of the following is in the first release, and none may be added by
implementation. Each requires a **new decision record, a threat-model amendment,
and separately approved backlog** before any work starts:

- a hosted origin, CDN, or any server the product depends on;
- a service worker, PWA install, or offline shell;
- a delegated browser controller, or any browser-held authority over a daemon;
- a browser-resident room peer, or browser-owned identity or key material;
- iOS.

Deferral is not a roadmap commitment. Each is listed because it appeared in the
superseded proposal and must not re-enter by assumption.

## Amendment disposition

The proposal's amendment epic is reconciled as follows. Eight are already closed;
the two that survive are reframed rather than inherited.

| Issue | Disposition |
|---|---|
| #115 | Closed — obsolete estimate wording |
| #116, #117 | Closed — delegated-browser approval and key custody are out of scope for an embedded-only first release |
| #119 | Closed — accessibility and localization requirements moved to #177/#197 |
| #120, #123 | Closed — WebKit storage eviction and service-worker rollback presuppose a browser-resident peer and a CDN, neither of which exists here |
| #122 | Closed — filesystem confinement is fixed, and protocol v2 removed daemon paths from the wire entirely |
| #126 | Closed — the surviving unsafe-boundary criterion moved to #203 |
| **#118** | **Retained** as the public-publication legal and compliance gate |
| **#121** | **Retained, reframed** around native update-channel trust, signing, and anti-rollback — with no compatibility window, mixed-version period, or N/N-1 behavior, none of which a single-generation clean slate has. Now decided in the [native update, signing, and anti-rollback policy](native-update-policy.md). |

## What this record does not decide

- The artifact's build, provenance format, or qualification evidence — #183 and
  #199.
- The browser session adapter's implementation — #171.
- What the system WebView boundary actually contains. **This record makes no
  sandbox claim**; #196 owns that review.
- Signing and notarization authority — #1 and #2.
- Legal, privacy, and compliance gates for public publication — #118.
- Android packaging and device qualification — #190–#194, with feasibility
  evidence still owed by #160.
- Whether the pairing code is the right long-term ergonomics. It is the right
  first-release security posture; a better mediator may exist once a packaged
  launcher owns the browser path.

## Citations

- [Dioxus clean-slate architecture](dioxus-architecture.md) — the client stack,
  the seam, and the artifact requirement this record delivers.
- [Protocol v2](protocol-v2.md) — the handshake, the ticket exchange, and the
  pairing-code credential path.
- [Security and threat model](security-threat-model.md) — the trust boundaries
  these extend, and the local-process threat that decided the credential path.
- [Production deployment architecture](production-deployment.md) — the
  superseded proposal, retained as history.
- [Production deployment architecture review](production-deployment-review.md) —
  its adversarial review, bound to the revision it examined.
