---
type: "Decision"
title: "Room Workbench — information architecture, routes, and status vocabulary"
description: "Decision record defining Jeliya's global-versus-room hierarchy, the canonical navigation state for every client, the wide/medium/compact shell contract, and the status vocabulary that keeps each surface bound to one provable fact."
tags: ["architecture", "ia", "navigation", "ux"]
timestamp: "2026-07-17T12:29:12Z"
status: "canonical"
implementation_status: "planned"
verification_status: "unverified"
release_status: "unreleased"
audience: ["contributors", "maintainers", "product"]
---

# Room Workbench — information architecture, routes, and status vocabulary

**Status: DECIDED 2026-07-17.** Jeliya adopts the *Room Workbench*
hierarchy: a small set of global destinations, and a workbench of
room-scoped tools that only exists once a room is selected. This record
defines that hierarchy, the canonical navigation state every client must
keep, the responsive shell contract, and the status vocabulary each
surface may use. It is the contract the UX 1 implementation issues test
against.

This document is normative for clients. It changes no protocol method and
no wire value; `docs/PROTOCOL.md` remains the daemon contract.

## Why

Two problems forced a decision.

**Scope was ambiguous.** Files and Pipes sat in global navigation, but
neither tool can do anything without a room — the daemon requires a
`room_id` for `file.list`, `file.share`, `pipe.list`, `pipe.expose`, and
`pipe.connect`. A global Files destination is therefore a destination that
is always secretly about one room, chosen elsewhere. Meanwhile the in-room
Agents tab and the global fleet dashboard both called themselves "Agents"
while answering different questions.

**One word described five incompatible facts.** "Active" meant, depending
on the surface: this daemon has a live session for the room
(`room.list.open`); this identity's signed roster status is not left or
removed (`room.list.status`); a member's signed roster status
(`room.members[].status`); an agent is live (`FleetAgent.liveness`); or a
count that silently falls back to the room's *total* member count when the
roster has not loaded. The last one is not a wording problem — it is the
screen asserting a fact it does not have.

## Decision 1 — the hierarchy

**Global destinations** (no room required):

| Destination | Job |
|---|---|
| **Rooms** | Choose or create a room; see every room this identity holds. |
| **Agent Fleet** | See agent liveness and runs across every authorized room at once. |
| **Settings** | Identity, daemon, diagnostics. |

**Room destinations** (a room is selected; the Room Workbench):

| Destination | Job |
|---|---|
| **Activity** | The signed timeline and the composer. The room's home. |
| **People** | The signed roster: members, invites, roles. |
| **Agents & Runs** | Agents *in this room* and their latest signed status. |
| **Files** | Files shared into this room. |
| **Pipes** | Pipes exposed and connected in this room. |

Consequences, each of which an implementation issue must satisfy:

- **Files and Pipes leave global navigation.** They are room tools; they
  are reachable only inside a room. This is not a demotion — it is where
  they always were.
- **Global Agent Fleet and room Agents & Runs are different
  destinations** with different names. "Agent Fleet" answers *are my
  agents alive, anywhere*; "Agents & Runs" answers *what has run here*.
- **Calls is hidden** until it supports a real workflow. A navigation entry
  that only says "Soon" is a promise the product has not earned.
- **Home is removed.** It duplicated Rooms. When it has a distinct user
  job it can come back with one.
- **Each destination has exactly one scope and one canonical entry path.**
  If a surface is reachable two ways, both ways resolve to the same route.

## Decision 2 — the canonical route contract

**The route is the navigation state.** Not a mirror of it, not a
serialization of it: the route *is* it. A client may not keep a second
state machine (a `tab`, a `mobileView`, a selected-pane enum) that can
disagree with the route. Where a client needs derived values, they are
derived from the route on read.

The route family, identical on every client:

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

1. **`/` resolves to `/rooms`.** The bare root is not a destination.
2. **`/rooms/:roomId` resolves to `/rooms/:roomId/activity`.** A room's
   canonical landing surface is its timeline.
3. **`:roomId` is the protocol `room_id`** (`blake3:…`), verbatim and
   percent-encoded. It is never a name, an index, or a short id.
4. **The web keeps these as URL paths.** `crates/jeliyad/src/serve.rs`
   already falls back to `index.html` for extensionless unknown paths, so
   deep links work in production without a daemon change.
5. **Flutter uses the same strings as named routes.** One spelling, two
   clients; a route in this table means the same destination in both.
6. **Query and fragment are not navigation state.** `?daemon=`, `?mock…`
   are transport and fixture inputs. Any canonicalizing redirect
   **must preserve `location.search`** — dropping it silently unfixtures
   the e2e suite and re-points the client at a different daemon.

### Unreachable and invalid rooms

A route naming a room this identity cannot open is not an error page and
not a blank panel. It resolves to a **recoverable state** that says which
fact is true:

| Condition | Resolution |
|---|---|
| Room id not in `room.list` | Stay on the route; show "That room isn't on this device", with Rooms as the way out. |
| `status` is `left` or `removed` | Show the signed fact ("You left this room" / "You were removed from this room"); do not open it. See the note below — this is a choice, not a limit. |
| `room.open` fails | Keep the route, surface the daemon's real error code and hint, offer Retry and Rooms. |
| Session still booting | Show the route's loading state — never an empty timeline, which reads as "no messages". |

**The archive is readable, and we are choosing not to read it.** A departure
keeps the subject in the member set, so `require_local_room_access` still
passes and `room.open` on a left or removed room is authorized — it returns
the roster and the full local timeline (`supervisor.rs`; `room.list`
deliberately keeps "joined-then-left archives"). Both clients nonetheless
stop at the signed fact, which is what shipped before this record and what
it keeps.

That is a product decision, not a protocol constraint, and it is worth
naming as one: reading an archive needs a surface that cannot lie about
what you may still do in it — no composer, no invite, no leave — and that
surface does not exist yet. Until it does, the honest thing is to state the
departure rather than open a room whose every action would fail. Filed as
follow-up work rather than smuggled in here.

### Legacy links

`?tab=members|agents|files|pipes` was read once at startup and never
written back. It migrates: the value maps onto the corresponding room
destination of the restored room (`members` → `people`), or to `/rooms`
when no room can be restored. It is a shipped URL surface, so it redirects
rather than 404s, and the redirect preserves the rest of the query string.

### Restoration

`localStorage['jeliya.lastRoom']` (web) and `prefs.lastRoomId` (Flutter)
restore *which room*, and **only from the bare root** — `/` on the web,
a cold start with no route on Flutter. Every route in the table above is an
explicit destination and is honored as one: `/rooms`, `/fleet`, and
`/settings` name no room *on purpose*, and restoring one into them would
make a direct link to Settings open a room, and a deliberate "Back to Rooms"
undo itself.

**An explicit route always wins over a restored room.** A bootstrap that
re-picks the last room while the route names a different one — or names none
— is a race, and the route is the authority. Restoration therefore happens
**once per launch**, not on every reconnect: re-running it is how a client
drags the user back into the room they just left.

The restored room is *pushed* on top of Rooms rather than replacing it, so
Back leaves the room for the rooms list instead of leaving the app.

## Decision 3 — the responsive shell contract

Three shells, one topology. The same information architecture; only the
mechanics change.

| Shell | Width | Layout |
|---|---|---|
| **Compact** | `< 900px` | One pane at a time. Global destinations in a bottom bar; room destinations via nested navigation inside Rooms. |
| **Medium** | `900px – 1279px` | Room rail + workspace. The inspector opens as a dismissible drawer over the workspace. |
| **Wide** | `>= 1280px` | Room rail + workspace + inspector, the inspector in flow as a third column. |

**The workspace is always Activity**; the inspector is where a room's tools
render. So the route decides the inspector, at every width:

| Route | Compact | Medium | Wide |
|---|---|---|---|
| `/rooms/:id/activity` | The room pane | Inspector closed | Inspector closed |
| `/rooms/:id/people` (and `agents`, `files`, `pipes`) | A pane pushed over the room | Inspector open as a drawer | Inspector open as a column |

Collapsing the inspector *is* navigating to `activity`, and opening it *is*
navigating to a tool. That is what makes one destination mean one thing on
all three shells, and it is why `activity` is a real destination rather
than a synonym for "a room is selected".

Why 1280 and not the current 901: at 901px the shipped three-column grid
(`232px` rail + `1fr` + `300px` inspector) leaves the workspace **369px** —
narrower than the phone layout it just graduated from. The medium shell
exists precisely to stop paying for a third column before there is room for
one. At 900px the medium workspace is 668px; at 1280px the wide workspace
is 648px with the inspector present.

Binding rules:

- **The inspector is a view, never a second source of truth.** It renders
  the room destination it was opened for. Collapsing it changes what is
  visible, never what is selected.
- **Selecting an item, and opening or closing the inspector, preserves
  list and timeline position.** Panes are hidden, not unmounted.
- **Connection status reserves layout space.** It never overlays Back, a
  header, or list content. Today's absolutely-positioned banner violates
  this and changes.
- **A connection transition is announced once**, through one live region.
  Long descriptions wrap or truncate accessibly; they do not push chrome
  off-screen.
- **The compact bottom bar carries only global destinations.** Room
  destinations are never bottom-bar tabs — that is the ambiguity this
  record exists to remove.
- **Room context stays visible on every room-scoped surface**, on every
  shell.
- **Back is truthful on every client.** Room destination → Activity →
  Rooms → leave the app. Back never mutates state the user cannot see.

Coverage: 360, 899, 900, 920, and 1280 logical pixels, plus safe-area
insets and 200% text. Flutter runs its coverage in English and French,
because French copy is longer and is where overflow shows up first.

The existing 44px/44dp touch floor and the 58px/58dp tab-bar *minimum*
(a minimum, not a cap — it grows with text scale) are unchanged.

## Decision 4 — the status vocabulary

**Every status label names exactly one fact, and that fact is one the
daemon proves.** These five are distinct and may never share a word.

| Fact | Source of truth | Vocabulary |
|---|---|---|
| **Room session** — this daemon has a live session for the room | `room.list.open`, `daemon.status.rooms_open` | **Open** / **Closed** |
| **Signed membership** — this identity's roster status | `room.list.status` (`active\|left\|removed`) | **Member** / **Left** / **Removed** |
| **Roster** — a member's signed status and role | `room.members[].status` (`active\|invited\|left\|removed`), `.role` | **Member** / **Invited** / **Left** / **Removed**, and **Unknown** for a status this client does not recognize; roles **Owner** / **Member** / **Agent** |
| **Peer reachability** — an observed transport path | `PeerStatus.state` (`connected\|connecting\|offline`) + `.path` (`direct\|relay\|null`) | **Direct** / **Relay** / **Connected** / **Connecting** / **Offline**; in aggregate **No peers connected** |
| **Agent liveness** — a fold over signed status events plus live peer state | `FleetAgent.liveness` (`online-idle\|working\|offline\|stale`) | **Working** / **Online** / **Stale** / **Offline**; the fleet filter spanning the first two is **Live** |
| **Pipe connection** — a local forwarding session | `pipe.list` `state` + `connected` | **Connected** (exposed, forwarding) / **Open** (exposed, nothing connected) / **Closed** |

### Retired words

- **"Active" is retired as a display label**, on every surface that used it.
  It described a live local session in the room rail, signed membership in
  the roster (which title-cased the wire value straight onto the screen), a
  live forwarding session on a pipe chip, and reachable agents in the fleet
  filter. Room session state reads **Open**/**Closed**; roster status reads
  **Member**; a pipe reads **Connected**; the fleet filter reads **Live**.
- **"Alone in this room" is retired.** It rendered whenever zero peer
  connections were observed — including a five-member room whose peers are
  merely offline. Absence of an observed connection is not evidence of
  solitude. The honest label is **No peers connected**, which is what the
  daemon actually reported.

`PeerStatus.path` is nullable **while `state` is `connected`** — the SDK
knows the peer is reachable before it knows how. That is why the peer
vocabulary carries a bare **Connected**: a connected peer with no path yet
is not a relay peer, and labelling it one would invent the very fact
(`direct` vs `relay`) the honesty rules exist to protect. Green is earned
here — the link is real — but the path is not claimed until it is known.
- **"N active" in the room header is retired**, and so is its fallback.
  It counted signed-active members but silently substituted the room's
  *total* member count whenever the roster had not loaded — asserting a
  fact from data it did not have. The header shows the roster count only
  once the roster is loaded, and its loading state until then.

### The wire is not the display

`room.list.status` carries the wire value `active`, and `agents.fleet`
returns a field literally named `active`. **This record renames no wire
value.** Display labels and wire values are never the same constant
(`docs/i18n.md`, rule 3); wire tokens route through the existing
`wire*`/`WireDisplay` seam. Renaming a label does not change
`labelTone` — tone still keys off the wire word, and the documented
long-term fix (a typed severity field on the agent-status event,
`docs/glossary-fr.md` decision 3) remains out of scope here.

### French

"Room Workbench" is an internal architecture name, not shipped copy — no
translation is owed. The shipped destination names follow
`docs/glossary-fr.md`: Tier 1 nouns translate (Rooms → Salons, Files →
Fichiers, People → Personnes, Activity → Activité); Tier 2 wire tokens
(`direct`, `relay`, `unavailable`, `unauthorized`, `hash_mismatch`,
`daemon`, `jeliyad`, `pipe`) never translate. New French copy follows the
existing typography decisions (U+202F before `; ! ?`, U+00A0 before `:`,
U+2019 apostrophe, sentence case, vouvoiement).

## Decision 5 — truthful states per destination

Every destination defines all six. A destination that renders its empty
state while it is actually loading, offline, or unauthorized is lying, and
the empty state is the most common such lie: "No files yet" and "we have
not asked yet" are different sentences.

| State | Rule |
|---|---|
| **Empty** | The daemon answered, and the answer was zero. Never shown before the answer arrives. |
| **Loading** | Asked, no answer yet. Distinct from empty on every surface. |
| **Offline** | No daemon connection. Reads as *unknown*, not as zero; last-known data is labelled stale, not presented as current. |
| **Stale** | Data whose freshness cannot be vouched for — a disconnected peer's last signed status. Labelled, never silently aged. |
| **Failed** | The daemon's real error code and hint (`unavailable`, `unauthorized`, `hash_mismatch`), plus a way forward. Never a silent partial result. |
| **Unauthorized** | The room is not this identity's to open. Says so; does not render an empty room. |

## Decision 6 — room identity and homonyms

**`room_id` is identity; `name` is a label.** The name is daemon-local
metadata, is `null` until the genesis event syncs, and carries no
uniqueness guarantee — `room.join { name? }` even lets two members label
the same room differently. Two rooms may share a name, and this record
does not change that.

Because the name cannot identify a room, any surface where acting on the
wrong room matters must show a **disambiguator**: the short room id, via
the existing `shortId` helper — the same primitive already used for
unbound peer endpoints, not a second id-shortening rule. Rules:

- Homonymous rooms (including two rooms both rendering the untitled
  placeholder, the most likely case for a new user) show the
  disambiguator wherever they are listed.
- **Destructive and sensitive actions always repeat it**, homonym or not.
  Leaving the wrong room publishes a signed departure that cannot be taken
  back.
- Creating a room whose name collides locally **warns and proceeds**. The
  name is a label; the product does not own the user's vocabulary.
- Room search accepts the name and the short id.
- Both clients use the same rule and the same facts.

## What this record does not decide

- **No protocol change.** No method, field, or wire value moves.
- **No new dependency.** The web router is the History API; Flutter stays
  on Navigator 1.0 with an explicit route model. Adding a routing library
  to a project whose entire runtime dependency set is `react` +
  `react-dom` would need its own decision, with the rationale and
  provenance that `app/pubspec.yaml` records for Flutter plugins.
- **`labelTone`'s dependence on wire prose.** Recorded above as a known
  residual, not fixed here.
- **Calls.** Hidden, not designed.

## Implementation

| Issue | Slice |
|---|---|
| #59 | Web: routes become canonical navigation state. |
| #61 | Web: scoped compact navigation and the room app bar. |
| #60 | Flutter: the nested Room Workbench. |
| #62 | Both: adaptive wide/medium/compact shells. |
| #49 | Both: homonymous room disambiguation. |

Each slice tests against this record. Where an implementation and this
document disagree, one of them is a bug — say which in the pull request.
