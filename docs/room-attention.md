---
type: "Decision"
title: "Room attention — evidence-backed recency, unread, and actionable state"
description: "Decision record defining how Jeliya's clients derive room recency, device-local unread, and actionable attention from provable facts, and the evidence rule each displayed field must satisfy."
tags: ["architecture", "attention", "room-list", "ux"]
timestamp: "2026-07-17T19:47:25Z"
status: "canonical"
implementation_status: "planned"
verification_status: "unverified"
release_status: "unreleased"
audience: ["contributors", "maintainers", "product"]
---

# Room attention — evidence-backed recency, unread, and actionable state

**Status: DECIDED 2026-07-17.** A returning user cannot see which rooms
changed or need action. The fix is a badge, a timestamp, an unread dot — and
every one of those is a claim. This record defines the data model and the
projection rules that let the room list carry recency, unread, and actionable
attention *without ever asserting a fact Jeliya cannot prove*: no delivery, no
read receipt, no fabricated progress, no invented presence.

It is the companion contract to [the Room Workbench record](room-workbench.md):
that document decided the room-list surface and the status vocabulary
(decision 4) and the six truthful states every destination owes (decision 5);
this one decides what recency, unread, and attention data may sit on a room
row and where each value is allowed to come from. It changes no protocol
method and no wire value. It identifies exactly one new, read-only, optional
`room.list` field and scopes its daemon work as deferrable follow-up, not a
release gate.

This record is normative for clients. It is the contract the room-list
recency/unread slice of #64 and the actionable-attention surface of #69 test
against.

## Why

Two failures are equally unacceptable, and they pull in opposite directions.

**The screen stays silent.** Today the room row shows a name, a member count,
and one of Open / Closed / Left / Removed — nothing about whether the room
changed since you last looked. A user with six rooms has no way to tell, from
the list, which one an agent just failed in. That is a real gap the milestone
exists to close.

**The screen comforts with a lie.** The naive fix — an unread count, a "2
new", a green "up to date" — invents facts the product does not hold. Jeliya
has no delivery receipt and no read receipt anywhere in the protocol
([PROTOCOL.md](PROTOCOL.md)); an unread badge sourced from "messages we
haven't shown you" is honest, but an unread badge that implies *the other
person saw yours* is the exact fake-state failure
[CONTRIBUTING.md](../CONTRIBUTING.md) forbids. A "last active 2m ago" read from
the wall clock rather than a signed event timestamp is the same failure in a
smaller font.

So the rule of this record is not "add badges." It is: **every field on a room
row names its evidence, and no field is rendered whose evidence the client
does not hold.** The rest of the document is that rule, made specific.

## Decision 1 — the evidence taxonomy

Every value a room row can display belongs to exactly one of four evidence
classes. The class decides what the value may claim, whether it survives the
room being closed, and whether it is shared or private.

| Class | What it is | May claim | Survives close? | Shared? |
|---|---|---|---|---|
| **Signed event** | A fact proven by an entry in the room's signed event log — its existence and its `created_at` timestamp are non-repudiable. | That the event happened, when its author dated it, and who signed it. | Yes — the log is persisted locally. | Yes — every member can verify it. |
| **Runtime fact** | A fact about *this daemon's current process*: whether a live room session exists (`open`), an observed peer path. | Only the present, and only as this device observes it. | No — a closed room has no session. | No — it is this node's view. |
| **Daemon projection** | A value the daemon computes by reading the *local persisted store with no live session* — `member_count`, and the `last_event_ts` this record adds. | A summary derived from signed events, as fresh as this device's last sync. | Yes — it reads the store, not a session. | Derived from shared data, delivered as a local summary. |
| **Device-local state** | A value stored only on this device, never on the wire, never protocol-backed: the unread last-seen mark, pin/archive, the display-only self label. | Only what *this device* has recorded about its own use. Never a claim about another participant. | Yes — it is this device's own storage. | Never. |

Consequences the implementation issues must honor:

- **A displayed field's tone may never outrun its class.** A daemon
  projection is only as current as the last sync; when the room's peers are
  unreachable it is *stale*, not *live*, and must be labelled so (decision 5
  of the Room Workbench record). Green is earned by a runtime fact
  (a live session, a connected peer), never by a projection.
- **Device-local state is never dressed as shared state.** Unread, pin, and
  archive get copy that says "on this device". The same identity on a second
  device will legitimately show different unread and different pins, and that
  is correct, not a bug to be "fixed" by syncing — see decision 3.
- **The wire is unchanged for everything except one identified projection
  field.** `last_event_ts` (decision 2) is the sole new value, and it is a
  read-only summary, not a new method or a new event kind.

## Decision 2 — recency: `last_event_ts`

**Last activity is the `created_at` of a room's newest signed event.** Not the
wall clock, not the moment the row rendered, not "now" — the timestamp the
event's author signed. This is the only recency source that is meaningful for
a room you are not currently in, and it is honest for one because it is a fact
the log already carries.

- The value is a **daemon projection**: the newest event's `created_at`, read
  from the local event store.
- It is delivered as a new, **optional, read-only** field `last_event_ts`
  (Unix milliseconds, nullable) on each `room.list` row, alongside the
  existing `room_id`, `name`, `role`, `status`, `member_count`, `open`. It is
  compatibility-nullable: an older daemon omits it, and a client renders no
  recency rather than a fabricated one.
- An optional companion `last_event_kind` (the newest event's kind) may
  accompany it so the list can say *what* last happened (a message, a member
  joined, an agent posted) without opening the room. It is subject to the same
  nullable, no-inference rule.
- Recency read from the local store reflects only events **this device has
  synced**. A room whose peers are offline shows a truthful-but-stale last
  activity; it is labelled **Stale**, never presented as current (decision 5).

Why a projection and not a live feed is decided in decision 5.

## Decision 3 — unread: a device-local last-seen projection

**Unread is defined as: this room has a signed event newer than the last-seen
mark this device recorded for it.** Formally, for a room `r`:

```
unread(r)  :=  lastEventTs(r) > deviceLastSeen[r]
```

Every term is provable and local. `lastEventTs(r)` is the recency projection
of decision 2. `deviceLastSeen[r]` is a **device-local** timestamp, stored
only on this device, that advances when you view the room.

- **Unread never implies anyone read or received anything.** It is a
  statement about *your own last look at your own device*, and nothing else.
  Jeliya has no delivery or read receipt; unread here is the absence of one,
  named honestly. The copy and the accessible label must never say or imply
  "seen", "delivered", or "they read it".
- **The last-seen mark initializes to when the room first appeared on this
  device** (its local first-seen time), not to zero. A room whose entire
  backlog synced before you ever opened it does not retroactively flag every
  historical message as unread; genuinely new activity after the room arrived
  does flag. This initialization is a deliberate choice to keep unread
  meaningful rather than noisy, and it is stated so it is not later changed by
  accident.
- **Clearing unread advances `deviceLastSeen[r]` to the newest event
  timestamp known for `r`, and affects that room only.** It is written to
  device-local storage and survives restart. The persistence rule is fixed:
  web `localStorage` under a single namespaced key
  (`jeliya.lastSeen`, `{ [room_id]: ts }`), Flutter `PrefsStore` (the same
  atomic JSON file that already holds `lastRoom`, aliases, and drafts). This
  mirrors the existing device-local `jeliya.lastRoom` / `prefs.lastRoomId`
  precedent exactly.
- **Cross-device divergence is correct.** Last-seen is per device by design;
  the same identity on a phone and a laptop will show different unread. Syncing
  last-seen would re-introduce the read-receipt this record refuses to invent,
  so it is out of scope now and named as a non-goal.

### Dot, not count, on the room list

An unread **dot** requires one comparison: `lastEventTs(r) > deviceLastSeen[r]`.
The `last_event_ts` projection carries exactly the datum that comparison needs,
so the room list can show an honest unread dot for any room, current or not.

An unread **count** is a different, stronger claim: it asserts *how many*
events you have not seen, and that number is only honest if the client is
holding the individual events after `deviceLastSeen[r]` and counting them. The
list-time projection deliberately carries only the newest timestamp, not the
tail, so **the room list shows a dot, never a count.** A count may appear only
on a surface that has the events themselves in hand (the open room's loaded
timeline). No estimate, no "9+" derived from anything but real, held events —
that is decision 6, applied.

## Decision 4 — actionable attention: a closed set over documented evidence

"Attention" is not "unread with urgency." It is a **closed set** of states,
each meaning that a specific, provable thing needs a human:

| Attention state | Meaning | Evidence |
|---|---|---|
| **Failed work** | An agent's work failed. | An `agent_status` event whose label maps, through the existing `labelTone` convention, to the failed (red) tone. |
| **Blocked work** | An agent is blocked awaiting input it cannot get itself. | An `agent_status` label in the documented blocked/waiting allowlist. |
| **Review requested** | An agent is waiting on a human review. | An `agent_status` label in the documented awaiting-review (blue/waiting) allowlist. |
| **Action failed** | A local action you took failed — a file transfer that errored, a pipe that dropped. | A **device-local runtime** failure this device observed. |

Nothing else is attention. A new message is unread, not attention. A healthy
running agent is *working*, not attention. An offline peer is stale, not
attention. Widening this set later is a decision with its own record, not an
inference an implementer may reach for.

### Two evidence classes, two reaches

The first three states are **signed-event** attention: they are proven by the
room's log and are therefore meaningful for a room you are not in — *if the
evidence reaches the list*. The fourth is **device-local runtime** attention:
it is only ever about an action taken on this device, and it does not travel.

This splits by reach, and the split must be stated so no one fakes the gap:

- **On the current (open) room**, the full timeline is loaded, so all four
  states are derivable client-side today, with no daemon change.
- **On a non-current room**, the list holds only the recency projection
  (decision 2). `last_event_ts` alone cannot prove *which tone* the newest
  actionable event carried. Signed-event attention on non-current rooms
  therefore needs the daemon projection to carry the evidence — the tone (or a
  typed severity) of the latest actionable `agent_status` — as an additional,
  read-only, nullable `room.list` field. That is **identified, deferrable
  daemon work** (decision 5), of the same shape and cost as `last_event_ts`.
  Until it lands, the room list flags attention only for rooms whose evidence
  it holds, and shows nothing — not a guess — for the rest.

### The untyped-label residual

Attention leans on `agent_status.label`, which is **free-form prose**: the
daemon folds the agent's status string verbatim, and `labelTone` classifies it
by matching wire words, not a typed severity field (the Room Workbench record's
"the wire is not the display", and [glossary-fr.md](glossary-fr.md) decision
3). So attention here is **evidence-bounded, not sentiment-inferred**: it fires
on a documented allowlist of labels, which can miss a real failure phrased in
an unlisted way and can misfire on a lookalike. This is a known limitation, not
a defect to paper over. The durable fix is a typed severity on the
agent-status event; it remains out of scope, and this record must not be read
as inferring attention from arbitrary prose.

## Decision 5 — the non-current-room data path (the identified finding)

This is the finding the issue asks for: **is daemon or protocol work required
because non-current-room pushes are unavailable or discarded?** Yes — and this
record identifies the minimal work rather than smuggling it in.

**Live pushes cannot back non-current recency.** Two independent facts make
this certain:

- The daemon's push fan-out iterates only open rooms
  (`crates/jeliya-core/src/engine.rs`, `push_loop` over `sup.open_room_ids()`):
  a closed room emits no `room.event` at all.
- Even for a non-current room that *is* open, the client drops the event
  (`ui/src/App.tsx` — `if (room_id !== roomIdRef.current) return;`; the Flutter
  `RoomStore` is likewise scoped to the current room). Non-current-room events
  are unavailable at the source and discarded at the sink.

"Keep every room open so every room pushes" is rejected: opening a room spawns
a per-room node session (`supervisor.rs`, `open_room`), and holding one open
for every known room to power a list badge is a cost far out of proportion to a
recency dot.

**The minimal honest mechanism is a read-only projection, not a live feed.**
`room.list` already enumerates every locally-known room and opens each room's
store to read its genesis name and departure sets (`supervisor.rs`,
`list_rooms`); a room's persisted events are readable with no live session, as
`timeline` and `agent_history` already do for closed rooms via
`readable_snapshot` + `room_tail`. So the newest event's `created_at` — the
`last_event_ts` of decision 2 — is **one more bounded read on the store
`list_rooms` already opens**, per room, at list time. No new method, no new
wire event, no transport change; one read-only field on an existing result.

**Scope and sequencing.** This mechanism is *identified* here as the #63
deliverable. Its daemon implementation is a small, self-contained follow-up PR
(extend `list_rooms` to emit `last_event_ts`, plus conformance vectors) that
may land in the opening slice of #64. It is explicitly **not** coupled to the
open Rust release-gate bugs (#46/#47/#50); it is new read-only work, not a bug
fix. Clients and fixtures adopt the field as compatibility-nullable now
(see [the fixtures section](#fixtures-and-parity)); the daemon fills it in
when the follow-up merges.

## Decision 6 — the per-affordance evidence rule

No badge, count, completion, progress, availability, or read state may be
inferred without a documented evidence rule. Each affordance the room list can
show is enumerated here with the evidence it requires and what it may not do.

| Affordance | Required evidence | Forbidden |
|---|---|---|
| **Last-activity time** | `last_event_ts` (daemon projection). Rendered relative or absolute; labelled Stale when the room's data cannot be vouched for. | The wall clock; the render time; any time not sourced from a signed event's `created_at`. |
| **Unread dot** | `lastEventTs(r) > deviceLastSeen[r]`, both held locally. | Any implication of delivery or that another party read/received; a dot when `last_event_ts` is absent. |
| **Unread count** | The individual events after `deviceLastSeen[r]`, actually held by the client. | An estimate; a count derived from `last_event_ts` alone; a count on the room list, where the tail is not held. |
| **Attention flag** | An `agent_status` label on the documented allowlist (signed-event attention) or a device-local runtime failure (action-failed). Dot **plus** label, never colour alone. | Sentiment inference over arbitrary prose; an attention flag for a non-current room whose evidence the client does not hold. |
| **"Up to date" / no-activity** | The daemon answered and there is genuinely no newer event. | Rendering the settled state while still loading, offline, or before `last_event_ts` arrives — that is decision 5 of the Room Workbench record ("we have not asked yet" is not "there is nothing"). |

## Fixtures and parity

The two clients keep hand-ported parallel mocks (`ui/src/lib/mock.ts`,
`dart/jeliya_protocol/lib/testing/mock_client.dart`) held to one envelope shape
by the conformance harness. Both must gain fixture rooms exercising the five
cases this record introduces, so React and Flutter render identical decisions:

- **Unread** — a room with a signed event newer than its device-local
  last-seen mark.
- **Attention** — a room whose newest `agent_status` maps to a failed / blocked
  / awaiting-review tone.
- **Offline** — a room with no daemon connection: recency reads as *unknown*,
  last-known data labelled stale.
- **Stale** — a projected recency whose freshness cannot be vouched for
  (a disconnected peer's last signed status).
- **No data** — a room with no events yet: no recency, no unread, no
  attention, and the settled empty state distinguished from loading.

The conformance harness normalizes the new `last_event_ts` field the way it
already normalizes timestamps and ids, so the mock and the real daemon stay
comparable. Because the two mocks are duplicated rather than shared, the five
cases doubled across two languages are a real drift risk; the conformance
vectors are the guard, and a shared fixture manifest is a reasonable later
consolidation.

## Truthful states on the room-list surface

The room list owns all six truthful states from the Room Workbench record
(decision 5). This record's fields slot into them so freshness is never faked:

- **Loading** — `room.list` asked, no answer yet: no recency, no unread, no
  attention. Distinct from a room that answered with no events.
- **Empty** — the daemon answered and the room genuinely has no events: no
  recency dot, an honest "no activity yet".
- **Offline** — no daemon connection: recency and unread read as *unknown*;
  any last-known values are labelled stale, not presented as current.
- **Stale** — a projection whose freshness cannot be vouched for: shown,
  labelled, never silently aged into looking current.
- **Failed** — `room.list` failed: its real error code and hint, not a blank
  or an invented "up to date".
- **Unauthorized** — unchanged from the Room Workbench record.

## What this record does not decide

- **No delivery or read receipt, ever.** The protocol has none, and unread is
  defined precisely so it can never be mistaken for one. Syncing device-local
  last-seen across devices is a non-goal, because it would manufacture the
  receipt this record refuses.
- **No new protocol method or wire event.** The single new datum is a
  read-only, nullable `last_event_ts` (and optional `last_event_kind`, and the
  later attention-severity projection) on the existing `room.list` result.
- **The `labelTone` dependence on wire prose** (Room Workbench record;
  glossary-fr.md decision 3). Attention is evidence-bounded to a documented
  allowlist as a result; the typed-severity fix stays out of scope and is named
  above as a residual.
- **The room-list UI itself.** Search, lifecycle filtering, pin/archive, and
  the row layout are #64. This record supplies the data contract those surfaces
  render; it does not lay them out.
- **The Agent Fleet attention surface's layout.** #69 renders the actionable
  set defined here; how it ranks and groups is that issue's decision.

## Implementation

| Issue | Slice |
|---|---|
| #63 | This record; the `last_event_ts` shape + device-local unread fixtures; the identified daemon projection. |
| #64 | The room list renders recency (dot) and consumes the projection; search/filter/pin are independent of this record. |
| #69 | The Agent Fleet renders the actionable-attention set defined here. |
| (follow-up) | Daemon: `list_rooms` emits `last_event_ts`; later, the attention-severity projection field. Deferrable; not gated on #46/#47/#50. |

Each slice tests against this record. Where an implementation and this document
disagree, one of them is a bug — say which in the pull request.
