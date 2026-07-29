---
type: "Reference"
title: "Product behavior contract"
description: "The required clean-slate cross-platform product behaviors the Dioxus client stack must satisfy, mined from pre-Dioxus decision records and written as implementation-neutral, evidence-owned rows against fresh state."
tags: ["clean-slate", "dioxus", "product", "qa", "ux"]
timestamp: "2026-07-29T02:25:50Z"
status: "draft"
implementation_status: "planned"
verification_status: "unverified"
release_status: "unreleased"
audience: ["contributors", "maintainers", "product", "qa"]
---

# Product behavior contract

**Status: DRAFT 2026-07-29 (issue #162).** This is the proposed clean-slate
cross-platform product behavior contract required by the
[Dioxus clean-slate architecture](dioxus-architecture.md) milestone M0. It
inventories every required product destination, critical journey, truthful
state, failure, and recovery the replacement client stack owes, and it maps
each requirement to the issue that must prove it. This record stays `draft`
until #162 closes on review; until then the behaviors below are the proposed
contract, not yet the reviewed one.

**Every behavior below is a requirement on unwritten code against fresh
state.** No Dioxus code exists in this tree. React and Flutter tests, closed
issue #77, and the [Room Workbench](room-workbench.md),
[Room attention](room-attention.md), and
[Device-local self label](self-label.md) records were requirements-mining
sources for this contract. They are not parity or compatibility authorities:
nothing here obliges the clean-slate stack to reproduce a React or Flutter
rendering, a v1 payload, a v1 storage shape, or a legacy key. Where this
contract deliberately changes a retained behavior, the change carries its
rationale in [Intentional changes](#intentional-changes).

This contract informs protocol authority ([protocol v2](protocol-v2.md)) and
is consumed by the executable required-behavior ledger (#195). It changes no
protocol method and no wire value itself.

## No-fake-state rules

These rules come first because they bind every row that follows. They restate
the contribution requirements of [CONTRIBUTING.md](../CONTRIBUTING.md) as
product contract, in implementation-neutral language.

1. **No fake state.** No optimistic "delivered" checks, no spinners implying
   progress that is not happening, no invented presence. Render what the
   signed log and the runtime prove. The protocol has no delivery receipt and
   no read receipt, and no surface may invent one — including a queued or
   pending affordance that reads as confirmation.
2. **Green is earned.** A healthy/live affordance marks a real, verified
   fact — a live session, a connected peer — never a projection, a
   decoration, or a fallback.
3. **Failures are failures.** Errors surface the daemon's real error code and
   hint (`unavailable`, `unauthorized`, `hash_mismatch`) plus a way forward —
   never a silent partial result, a blank panel, or a fabricated "up to
   date".
4. **Every displayed field names its evidence.** No badge, count, completion,
   progress, availability, or read state is inferred without a documented
   evidence rule, and no field is rendered whose evidence the client does not
   hold. A daemon projection is only as fresh as the last sync; when freshness
   cannot be vouched for, the value is labelled stale, never silently aged.
5. **The wire is not the display.** Display labels and wire values are never
   the same constant; wire tokens route through the localization seam and are
   never translated. Unknown wire values pass through raw (forward compat).
6. **Destructive and sensitive actions never take initial focus**, repeat the
   room's disambiguator (short id) even when no homonym exists, and their
   focus order defaults to abandoning, not confirming. Leaving the wrong room
   publishes a signed departure that cannot be taken back.
7. **Recovery from ambiguous transport failure is truthful.** A client that
   cannot tell whether a mutation executed says so, and distinguishes
   never-sent work from work that may have executed. Only an operation with an
   explicit, tested v2 deduplication guarantee may replay; everything else
   never auto-replays. Gap detection leads to the one authoritative resync
   path; there is no silent catch-up and no fabricated reconnect on the
   DirectClient path.

## Required destinations

The [Room Workbench](room-workbench.md) hierarchy is retained: three global
destinations, five room destinations, each with exactly one scope and one
canonical entry path.

| Destination | Scope | Required behavior | Platform rows | Evidence ownership |
|---|---|---|---|---|
| **Rooms** | global | Choose or create a room; see every room this identity holds, with recency, unread, and attention under the evidence rules. | shared | #179 (web), #184 (desktop), #193 (Android); qualified #182, #189, #194 |
| **Agent Fleet** | global | Agent liveness and runs across every authorized room at once; renders the closed actionable-attention set. | shared | #180 (web), #184, #193; qualified #182, #189, #194 |
| **Settings** | global | Identity, daemon, diagnostics; device-local self-label editor; language (text and formatting locale) settings; diagnostics redact full identities and never contain the self label. | shared; desktop adds daemon ownership/auth/shutdown and preferences surfaces; Android adds package/storage facts | #180, #185 (desktop prefs), #190 (Android); qualified #182, #189, #194 |
| **Activity** | room | The signed timeline and the composer; the room's canonical landing surface. | shared | #179; qualified #182, #189, #194 |
| **People** | room | The signed roster: members, invites, roles; invitation issue/replace/redeem flows. | shared | #180; qualified #182, #189, #194 |
| **Agents & Runs** | room | Agents *in this room* and their latest signed status — a different destination from the global Agent Fleet. | shared | #180; qualified #182, #189, #194 |
| **Files** | room | Files shared into this room; share and fetch through the platform's file services; the v2 maximum-file-size behavior with its distinctive over-limit error ([shared-file size policy](shared-file-size.md)). | shared; file pickers and export differ: browser download, desktop file dialog, Android SAF and share sheet | #181 (web files), #184, #192 (Android SAF); qualified #182, #189, #194 |
| **Pipes** | room | Pipes exposed and connected in this room. | shared | #181; qualified #182, #189, #194 |

Retained constraints: **Calls stays hidden** until it supports a real
workflow; **Home is removed**; Files and Pipes exist only inside a room. A
navigation entry that only says "Soon" is a promise the product has not
earned.

## Canonical routes

The route family is retained verbatim, identical on every platform. The route
*is* the navigation state; no second state machine may disagree with it.

| Route | Destination |
|---|---|
| `/rooms` | Rooms (no room selected) |
| `/rooms/:roomId/activity` | Room → Activity |
| `/rooms/:roomId/people` | Room → People |
| `/rooms/:roomId/agents` | Room → Agents & Runs |
| `/rooms/:roomId/files` | Room → Files |
| `/rooms/:roomId/pipes` | Room → Pipes |
| `/fleet` | Agent Fleet |
| `/settings` | Settings |

Rules:

1. `/` resolves to `/rooms`; `/rooms/:roomId` resolves to
   `/rooms/:roomId/activity`.
2. `:roomId` is the protocol `room_id`, verbatim and percent-encoded — never
   a name, an index, or a short id.
3. Query and fragment are not navigation state; any canonicalizing redirect
   preserves the query string.
4. **An explicit route always wins over a restored room.** Restoration of the
   last open room happens once per launch, only from the bare root, and loses
   to every explicit route. The restored room is pushed on top of Rooms so
   Back leaves the room for the rooms list.
5. **Back is truthful on every platform:** room destination → Activity →
   Rooms → leave the app. Back never mutates state the user cannot see. On
   Android this binds the platform Back gesture, including predictive Back.
6. A route naming an unreachable room resolves to a **recoverable state**,
   never an error page or a blank panel: room not on this device (Rooms as
   the way out); signed `left`/`removed` fact shown plainly; a failed open
   surfaces the real error code and hint with Retry and Rooms; a still-
   booting session shows the route's loading state — never an empty timeline.

**Changed surface (recorded under Intentional changes):** the legacy
`?tab=members|agents|files|pipes` query affordance and the legacy persistence
keys do not exist in the clean-slate stack. There is no migration reader.

## Responsive shells and viewports

Three shells, one topology; the same information architecture at every width.
The workspace is always Activity; the inspector renders the room tool the
route names.

| Shell | Width | Layout |
|---|---|---|
| **Compact** | `< 900px` | One pane at a time. Global destinations in a bottom bar; room destinations via nested navigation inside Rooms. |
| **Medium** | `900px – 1279px` | Room rail + workspace; the inspector opens as a dismissible drawer over the workspace. |
| **Wide** | `>= 1280px` | Room rail + workspace + inspector, the inspector in flow as a third column. |

Binding rules retained from the Room Workbench record: the inspector is a
view, never a second source of truth; selecting an item and opening or
closing the inspector preserves list and timeline position; connection
status reserves layout space and is announced once through one live region;
the compact bottom bar carries only global destinations; room context stays
visible on every room-scoped surface on every shell.

The medium shell exists to stop paying for a third column before there is
room for one: at 901px the shipped three-column grid leaves a 369px
workspace — narrower than the compact layout it just graduated from. At
900px the medium workspace is 668px; at 1280px the wide workspace is 648px
with the inspector present.

**Required coverage:** 360, 899, 900, 920, and 1280 logical-pixel widths,
plus safe-area insets and 200% text, **in English and French** — French copy
is longer and is where overflow shows up first. The 44px/44dp touch floor
and the 58px/58dp tab-bar minimum (which grows with text scale) are
unchanged.

Platform note: the compact shell is the Android shell; its IME behavior,
predictive Back, rotation, and foreground/background lifecycle are ported
under #193 and qualified under #194.

## Bootstrap and onboarding

Bootstrap is a critical journey with its own truthful-state obligations, not
a splash screen. It is ported under #178 (web shell, routing, fresh browser
preferences), #184 (desktop), and #190–#193 (Android), and qualified per
platform.

- **A still-booting session shows the route's loading state** — never an
  empty timeline, which reads as "no messages", and never an empty room,
  which reads as "you are alone". Booting is *unknown*, not *zero*.
- **First run creates or connects an identity** and offers the optional
  device-label field alongside it (the self-label contract). The
  cryptographic identity id is present and copyable from the start,
  described as the unrecoverable P2P identity.
- **First-room creation is the onboarding terminus**: a new user ends
  onboarding holding a room they created or joined, not a dashboard. Rooms
  is the landing destination for a returning user with no explicit route.
- **The fresh-state/reset policy has a user-facing half.** When the stack
  meets old-generation state — a legacy storage generation, an
  unverified data directory — it **fails closed and shows an actionable
  reset path**; it never deletes, migrates, or reinterprets unverified
  state on the user's behalf. The reset path is shown, not taken
  (#156's clean-slate policy; the Android beachhead fails closed on an
  unverified directory, spike #160).
- **Daemon bootstrap facts are structured, not composed copy**: boot
  stages surface as typed facts the UI narrates, and a failed bootstrap
  surfaces the real failure with a way forward — retry, diagnostics, or
  the reset path — never an infinite spinner.

## Status vocabulary and truthful states

**Every status label names exactly one fact, and that fact is one the daemon
proves.** These six vocabularies are retained as the product contract; they
may never share a word:

| Fact | Vocabulary |
|---|---|
| **Room session** (this daemon has a live session) | **Open** / **Closed** |
| **Signed membership** (this identity's roster status) | **Member** / **Left** / **Removed** |
| **Roster** (a member's signed status and role) | **Member** / **Invited** / **Left** / **Removed**, **Unknown** for an unrecognized status; roles **Owner** / **Member** / **Agent** |
| **Peer reachability** (an observed transport path) | **Direct** / **Relay** / **Connected** / **Connecting** / **Offline**; in aggregate **No peers connected** |
| **Agent liveness** | **Working** / **Online** / **Stale** / **Offline**; the fleet filter spanning the first two is **Live** |
| **Pipe connection** (a local forwarding session) | **Connected** (exposed, forwarding) / **Open** (exposed, nothing connected) / **Closed** |

Retired words, retained: **"Active"** is retired as a display label on every
surface; **"Alone in this room"** is retired (absence of an observed
connection is not evidence of solitude); **"N active"** room-header counts
are retired, including the fallback that silently substituted the total
member count for an unloaded roster. A connected peer whose path is not yet
known is **Connected**, never **Relay** — the path is not claimed until it
is known.

**The six truthful states** are owed by every destination, including the
room list itself:

| State | Rule |
|---|---|
| **Empty** | The daemon answered, and the answer was zero. Never shown before the answer arrives. |
| **Loading** | Asked, no answer yet. Distinct from empty on every surface. |
| **Offline** | No daemon connection. Reads as *unknown*, not as zero; last-known data is labelled stale, not presented as current. |
| **Stale** | Data whose freshness cannot be vouched for. Labelled, never silently aged. |
| **Failed** | The daemon's real error code and hint, plus a way forward. Never a silent partial result. |
| **Unauthorized** | The room is not this identity's to open. Says so; does not render an empty room. |

## Retained product invariants

Each row below is re-expressed in implementation-neutral language against
fresh state. Their v1 bytes and persistence shapes are not retained.

| # | Invariant | Required behavior | Evidence ownership |
|---|---|---|---|
| 1 | **Late invitation works after established conversation history** (#46) | An identity invited into a room that already carries established conversation history — single- or multi-author — joins and receives that history; nothing about the join depends on being present at genesis. | conformance corpus #161; runtime #168; ledger row #195 |
| 2 | **Expired invitations can be replaced for the same identity and then redeemed** (#47) | Replacing an expired ticket for an identity invalidates the old ticket; the fresh ticket redeems; the replacement is durable across daemon restart. | conformance corpus #161; runtime #168; ledger row #195 |
| 3 | **Multiple rooms stay live concurrently and surface independent activity** (#147) | Rooms held open at once each receive and surface their own events with no routing or push loss and no cross-room bleed. | runtime #168; adapter suite #175; ledger row #195 |
| 4 | **Membership, presence, file-provider availability, and Pipe reachability are distinct facts** (#50, #79, #94) | Presence across direct/relay/reconnect is authoritative; a file provider's availability is a protocol fact distinct from membership display; Pipe reachability carries its own distinctive unavailable behavior. None is inferred from another. | conformance corpus #161; runtime #168; ledger row #195 |
| 5 | **Departed rooms can be opened as explicit local read-only historical archives** (#91) | A left or removed room opens as a local, read-only archive: the signed timeline and historical roster render, the signed left/removed fact is stated plainly and permanently, and composer, Invite, Leave, file share/fetch, and Pipe actions are suppressed as typed capabilities — not disabled buttons scattered through UI code. No live networking starts; rejoining requires a new invite. | #91 owns the detailed archive contract; surfaces #179; qualified #182, #189, #194; ledger row #195 |

Invariant 5 is a **deliberate widening** of the retained behavior — see
Intentional changes. Until the archive surface exists, the retained floor
applies: a departed room states the signed fact and does not open.

## Recency, unread, and attention

The [Room attention](room-attention.md) data model is retained as product
contract, re-expressed against the v2 stack:

- **Recency** is the `created_at` of a room's newest signed event, read as a
  daemon projection. Never the wall clock, never the render time.
- **Unread** is `lastEventTs(r) > deviceLastSeen[r]`, both held locally. The
  last-seen mark is one device-local timestamp per room, advanced on view,
  surviving restart, never on the wire, initialized to the room's recency
  when the room first appears on this device. **Unread never implies anyone
  read or received anything**, and copy and accessible labels must never say
  or imply "seen", "delivered", or "they read it".
- **The room list shows a dot, never a count.** A count may appear only on a
  surface holding the individual events after the last-seen mark.
- **Attention is a closed set**: failed work, blocked work, review requested
  (all signed-event attention over the documented label vocabulary — in v2 a
  closed vocabulary, so severity is a lookup, not an inference), and action
  failed (device-local runtime failure). Nothing else is attention; widening
  the set is a decision with its own record.
- **Cross-device divergence is correct.** Unread, pin/archive, and the self
  label are device-local by design; syncing them would manufacture a read
  receipt and is a non-goal.
- **Live activity supplements, never replaces, the store projection.** It
  moves a row's recency only forward, never seeds the unread baseline, and is
  discarded with the connection. Holding rooms open to power the list is
  rejected.

The five fixture cases from the Room attention record survive as required
coverage — unread, attention, offline, stale, no-data — and their clean-slate
guarantee is the single fault-injected adapter contract suite (#175): the
deterministic mock, `WsWeb`, `WsNative`, and `DirectClient` must expose the
same view-level contract while keeping their honest transport-specific
lifecycle differences.

## Identity, aliases, and self label

Retained from [Device-local self label](self-label.md), re-expressed against
fresh state:

- **One device-local alias per identity id, including the self id.** Display
  of self resolves to `alias(selfId) ?? "You"`; the fallback is the localized
  "You", never the raw hex id. Peers resolve `alias(id) ?? suggestion ??
  shortId(id)`, where a suggestion is a daemon-provided display hint — never
  signed, never authoritative.
- **Local only, never signed.** The label lives only on this device, is never
  sent, never appears in a signed event or roster, and is excluded from
  diagnostics. Every editor states this in copy.
- **Validation:** trim surrounding whitespace; an empty or whitespace-only
  value clears the label; a soft 40-character maximum enforced on input; no
  other format constraint.
- **Self is identified consistently** at every self-rendering site — sender
  name and avatar in the timeline, profile card, settings identity surfaces,
  the roster member row (which keeps its distinct "this device" marker), and
  the pipe authorized-peer line. The own-message side and the "this device"
  marker are orthogonal to the label.
- **The cryptographic identity id stays secondary but reachable**: shortened
  by default, fully copyable, described as the unrecoverable P2P identity.
- **Invitation identity inputs start empty** with an example/help state,
  never pre-seeded with the user's own id.
- **First run** offers an optional device-label field alongside the created
  identity; **Settings** exposes the same editor.

**Changed behavior (recorded under Intentional changes):** there is no
migration. A returning user on the replacement stack starts with no label
and sees "You" until they set one.

## Room identity and homonyms

Retained from the Room Workbench record:

- **`room_id` is identity; `name` is a label.** The name carries no
  uniqueness guarantee; two rooms may share a name.
- Homonymous rooms — including two rooms both rendering the untitled
  placeholder — show the short-id disambiguator wherever they are listed.
- **Destructive and sensitive actions always repeat the disambiguator**,
  homonym or not.
- Creating a room whose name collides locally **warns and proceeds**.
- Room search accepts the name and the short id.

## Preferences and device-local persistence

**Preferences persist within the new app only** — new-format values in the
new namespaced storage generation, written through injected platform
services. No legacy key is read, and nothing from the retiring stack is
imported.

| Preference | Scope | Rule |
|---|---|---|
| Last open room | device-local | Restored once per launch, only from the bare root; always loses to an explicit route. |
| Last-seen marks | device-local, per room | One timestamp per room; advanced on view; survives restart; never on the wire. |
| Pin/archive room flags | device-local | Copy says "on this device"; never dressed as shared state. |
| Aliases (incl. self label) | device-local | Per identity id; never signed; excluded from diagnostics. |
| Per-room composer drafts | device-local, per room | Restored across restart; never sent; clearing a room's draft affects that room only. |
| Text locale | device-local | Unset follows the platform's preferred languages, falling back to English; applies live. |
| Formatting locale | device-local | Independent of the text locale from day one; unset follows the platform locale, falling back to the resolved text locale; applies live. |

The namespaces these live in are not named here: #178 fixes the browser key
namespace, #185 the desktop preferences store and its version key, and #173
the Android data directory. Each must be a name no retiring client ever
wrote.

## PlatformServices

The injectable boundary itself is #174's to design. The product rules that
bind it are fixed here:

- **Native capability reaches surfaces only through the injected
  `PlatformServices` boundary** — files, persistence, lifecycle, URLs,
  clipboard and share, navigation, and window actions — never through a
  platform `cfg` fork in a shared component.
- **Every service has a deterministic test implementation**, so every
  behavior in this contract is exercisable without a device.
- **A local file path and a `content://` URI are not interchangeable.** A
  surface that displays or accepts a file location must know which one it
  holds; an Android content URI must never be rendered as if it were a
  filesystem path.
- **Where the platform's affordance differs, the product behavior does
  not**: share, fetch, export, and open-in-browser mean the same thing to
  the user on every platform; only the mechanism is platform-specific.

## Localization — English and French

The decisions of [Internationalization](i18n.md) and
[French glossary](glossary-fr.md) are retained as product contract; the
mechanics (ARB catalog, TypeScript catalogs, both enforcing gates) retire
with the clients they serve, and their replacement is #177's:

- **French ships at desktop launch**, full-catalog in one release.
- **Text locale ≠ formatting locale** from day one.
- **Daemon/CLI output stays English**; the UI maps `{code, message, hint}` to
  translated copy client-side, and raw daemon text appears only in the
  collapsed technical-details disclosure, the Settings diagnostics card, and
  the diagnostics report.
- Tier 1 nouns translate (Rooms → Salons, Files → Fichiers, People →
  Personnes, Activity → Activité); Tier 2 wire tokens (`direct`, `relay`,
  `unavailable`, `unauthorized`, `hash_mismatch`, `daemon`, `jeliyad`,
  `pipe`) never translate.
- French typography follows the glossary's decisions (U+202F before `; ! ?`,
  U+00A0 before `:`, U+2019 apostrophe, sentence case, vouvoiement).
- No sentence assembly in component trees; no wire values as display text;
  all display formatting goes through the shared formatting seam; tests
  assert copy via the shared catalog, not literals.

Evidence ownership: #177 establishes the clean-slate localization foundation
and its replacement gate; #197 records the complete localization release
evidence. Bambara and N'Ko remain the roadmap of record in
[Internationalization](i18n.md) and are unchanged by this contract.

## Accessibility

The accessibility floor of [CONTRIBUTING.md](../CONTRIBUTING.md) and the
manual residue of the [accessibility release checklist](accessibility-checklist.md)
are retained as product contract:

- **WCAG 2.1 AA**: ≥4.5:1 contrast for information-bearing text; status never
  by color alone (dot + label); `prefers-reduced-motion` honored at the OS
  level; full keyboard operability from launch, including onboarding.
- **Semantic structure**: one `main` and one `h1` per destination; named
  landmarks that distinguish panes; skip links as the first two tab stops
  that land focus; the room tab strip behaves as a tablist.
- **Announcements fire once**: a new message is announced once; a connection
  transition is announced once through one live region; liveness and last-
  posted status read as two separate facts (a "Stale" agent whose last label
  was "Working" must not sound like it is working now).
- **Keyboard truths**: focus ring visible everywhere it lands; no focus trap
  outside a dialog; Escape releases a dialog to the control that opened it;
  destructive actions never take initial focus; nothing reachable but
  invisible.
- **Text scale and layout**: no clipped layout at 100/200/320% text in
  English and French; every primary and Cancel action reachable at maximum
  OS text size, by scrolling if necessary.
- **Reading order matches visual order** on the Activity timeline, including
  day dividers and folded agent runs.

Platform rows: web and desktop screen-reader and keyboard behavior is
qualified under #182 and #189 against the system-WebView matrix; Android
screen-reader traversal (TalkBack descendant navigation inside the system
WebView) is an explicit open gap from spike #160 and a required qualification
row under #194 — the spike measured the WebView focusing as one node, and no
Android row of this contract may be marked met until descendant traversal is
proven on a physical device. Release evidence is recorded under #197 as
**enforced evidence, not certification**.

## Platform-specific behavior

Behavior in this section is deliberately **not** shared; each row names its
platform and its evidence owner. The section restates architecture facts
only where they produce user-visible behavior; the
[Dioxus clean-slate architecture](dioxus-architecture.md) record owns the
underlying mechanisms.

| Behavior | Web | Desktop | Android |
|---|---|---|---|
| **Transport and session** | Browser WebSocket with fresh `/api/session` authentication on every attempt; connected only after protocol validation; holds no Iroh dependency and no node identity of its own — the daemon is the room peer | Native WebSocket through the supervisor and resolver on every connection attempt; only verified loopback endpoints dialed; the daemon token stays native, never crosses into WebView script, and is redacted in logs and diagnostics | `DirectClient`: typed `jeliya-core` in process — no socket, token, or portfile; calls execute serially; resume triggers authoritative resync without a fabricated reconnect |
| **Daemon relationship** | Served and authenticated by the trusted local `jeliyad` path per the [first-release distribution boundary](first-release-distribution.md) | The packaged app supervises or adopts a real `jeliyad`: owned versus adopted shutdown is enforced, and an adopted daemon outlives the shell | None — the engine runs in process |
| **Files** | Browser download for fetched files | Native file dialogs and export through `PlatformServices` | Storage Access Framework pickers, fetched-file export, share sheet, clipboard, and safe external actions (#192) |
| **Navigation and lifecycle** | Deep links resolve as URL paths served by the daemon | Clean-install packages enforce daemon ownership/auth/shutdown, fresh storage, and platform services (#184–#187) | Compact shell; IME; predictive Back; rotation; foreground/background lifecycle with truthful resume (#193); protected fresh state with backup exclusion (#190) |
| **Package identity** | n/a | One reserved application or bundle identifier per packaged target, never an identifier a retiring client ships | Same rule, plus release signing (#190) |
| **WebView policy** | n/a (the browser is the WebView) | Navigation, new-window, download, devtools, and storage policies fail closed in the packaged system WebView (#189, #196) | Same fail-closed policy inside the Android system WebView; the WebView version is captured as device evidence with no floor decided (#160, #194) |

One platform using another's evidence is forbidden: every row above is
qualified on its own platform (#182 web, #189 desktop, #194 Android), and a
missing platform gate blocks only that platform's publication row (#199).

## Intentional changes

Every deliberate difference between this contract and the retained records,
with rationale. Each is a change of behavior, not an omission.

| # | Change | Rationale |
|---|---|---|
| 1 | **One client stack, not two.** Every two-client parity premise in the source records is dropped; the equivalence guarantee becomes the single fault-injected adapter contract suite (#175). | The [Dioxus clean-slate architecture](dioxus-architecture.md) replaces React and Flutter with one Rust client stack; parity gates between retiring clients cannot bind a stack that has one client. |
| 2 | **No legacy persistence, no migration.** The legacy `?tab=` URL affordance, all seven legacy browser storage key families, and the Flutter `app_prefs.json` store have no reader and no migration. Preferences persist within the new app only. | The clean-slate policy of #156: old data fails closed with an actionable reset path; silently reinterpreting old state is an explicit non-goal. |
| 3 | **Departed rooms open as read-only archives** — a widening of the Room Workbench record, which chose to state the signed fact without opening the room because no truthful archive surface existed. | #91 designs that surface: typed capabilities that suppress every live action, no live networking, the signed departure fact stated permanently. The invariant is retained from the pre-Dioxus reports; the widening is owned in detail by #91. |
| 4 | **The unread-baseline rule for a no-recency daemon is dropped.** In v2 every listed room carries recency — a room with no stored events fails its own fold and is not listed — so there is no no-recency daemon to baseline against. The "first live event establishes the baseline" clause of the Room attention record describes a v1 mixed-version case. | Protocol v2 is the only generation the clean-slate stack speaks; there is no older daemon to interoperate with. |
| 5 | **Attention severity is a lookup, not an allowlist inference.** Protocol v2 closes the agent-status label vocabulary and adds a `blocked` label, resolving the untyped-label residual the Room attention record carried. | Recorded as the #161 amendment to that record; severity becomes a property of the closed vocabulary rather than a prose match. |
| 6 | **French typography, status vocabulary, and destination names carry forward; the catalogs and gates that enforced them do not.** Their replacement is #177's, and no enforcement may lapse before its replacement is qualified. | Recorded in [known gaps and roadmap](known-gaps-roadmap.md) as the verification the retirement removes. |
| 7 | **The device-local preference set is fixed by this contract** (seven rows in the preferences table). Additions are a contract change, not an implementation detail. | The legacy review finding that the retiring web client grew unlisted storage keys is answered by fixing the set as contract. |

## Evidence ownership and the ledger

This contract is the input to #195, which makes an executable ledger of
every required row, its evidence owner, its command or artifact, and its
first-release status. The mapping this contract asserts:

- **Per-behavior rows** above name their implementing slices (#176–#185 for
  web and desktop surfaces, #190–#193 for Android) and their qualification
  gates (#182 web, #189 desktop, #194 Android).
- **Protocol-level invariants** (the retained-invariants table) are proven by
  the protocol-v2 conformance corpus (#161) and the client runtime (#168),
  and become explicit fresh-state ledger rows in #195.
- **Cross-cutting gates**: accessibility and localization evidence #197;
  system-WebView security review #196; performance budgets #198; reproducible
  per-platform artifacts #199.
- **Nothing ships on one platform's evidence.** A row is met on a platform
  only by evidence produced on that platform.

Rows of this contract that have no automation home yet become child backlog
items or explicit manual gates in #195; marking a required row not-applicable
without rationale is forbidden there, exactly as it is here.

### Evidence review of retained rows

#162's verification ask: every retained row is reviewed against the evidence
that exists on the retiring stack today, and identified as tested-there or
untested. Evidence produced on the retiring stack never certifies the new
one — the "retiring-stack evidence" column is an inventory of intent to
re-prove, not a claim. Rows whose only coverage lives on the retiring stack
are **untested for the new stack** and become #195 child items or manual
gates.

| Retained row | Retiring-stack evidence | Status for the new stack |
|---|---|---|
| Late join after established history (#46) | core loopback join suite; retained direct/relay network runs | untested — v2 conformance corpus (#161) + runtime (#168) |
| Replaced expired ticket redeemed (#47) | core replacement/durability tests | untested — #161 + #168 |
| Multiple rooms live concurrently (#147) | live multi-room activity shipped under #151/#153/#155 on both retiring clients | untested — runtime #168; adapter suite #175 |
| Presence/provider/Pipe distinction (#50/#79/#94) | core negative-RPC and status tests; `peer-status.spec.ts` | untested — #161 + #168; surfaces #179–#181 |
| Departed-room archive (#91) | none — the archive surface does not exist on the retiring stack; only the signed-fact floor ships | untested — #91 designs; surfaces #179; qualified #182/#189/#194 |
| Destinations, routes, shells, truthful states | `rooms.spec.ts`, `room-nav-strip.spec.ts`, `responsive.spec.ts`, `compact-room.spec.ts`, `room-recovery-routes.spec.ts`, `room-open-recovery.spec.ts`; Flutter `a11y_matrix_test.dart` widths | untested — Dioxus Playwright/real-daemon matrix #182; desktop #189; Android #194 |
| Homonym disambiguation | `room-disambiguation.spec.ts` | untested — #179; qualified #182/#189/#194 |
| Recency, unread, attention | shipped under #151/#153/#155; five fixture cases across both retiring mocks held by the conformance harness | untested — adapter suite #175 replaces the two-mock guarantee |
| Self label, aliases | shipped on both retiring clients; validation and privacy covered in widget/component tests | untested — #178 (web prefs) and #185 (desktop store); qualified per platform |
| EN/FR localization | `strings_fr_test.dart`, `l10n_parity_test.dart`, `locale*_test.dart`, `panel_fr_layout_test.dart`, `i18n-layout.spec.ts`; both i18n gates | untested — replacement gate is #177's; evidence #197; **no enforcement may lapse before its replacement is qualified** |
| Accessibility | `a11y.spec.ts`, `a11y-matrix.spec.ts`, `a11y_*_test.dart`; manual checklist residue | untested — re-established per platform (#182/#189/#194); TalkBack descendant traversal is an open physical-device gap (#160 → #194) |
| Files and the v2 size limit | file share/fetch covered by the loopback suite; the 100 MiB policy is decided (#92) with no new-stack surface | untested — #181, #184, #192; qualified per platform |
| Real-network behavior | realnet runbook procedure; signed direct/relay evidence for `v0.6.0` | untested — v2 real-network qualification is owed by #194 (Android) and the M6 first-release gates |

Every "untested" cell above is the row's identification as a child backlog
item or explicit manual gate, as #162 requires.

## What this contract does not decide

- **Protocol v2 itself**, its wire schemas, and its conformance corpus —
  #161 owns them; this contract informs but does not specify the protocol.
- **The archive surface design** — #91 owns the departed-room archive in
  detail; this contract fixes only the retained invariant and its widening.
- **The room-list layout, ranking, and grouping** — search, lifecycle
  filtering, pin/archive placement, and the Agent Fleet attention surface
  layout are surface decisions of #179 and #180, bound by the evidence rules
  here but not laid out here.
- **The replacement storage namespaces** — #178, #185, #173.
- **Performance budgets** — #198.
- **Windows first-release scope** — #188.
- **The localization and accessibility enforcement machinery** — #177 and
  #197; this contract fixes the behaviors they must enforce.
- **Pixel identity** — an explicit non-goal, as in #162.
