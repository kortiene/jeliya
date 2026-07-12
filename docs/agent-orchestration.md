---
type: "Reference"
title: "Agent orchestration contract (v1)"
description: "Normative contract for agent liveness, task claims, fleet reads, and UI projections."
tags: ["agents", "fleet", "orchestration", "protocol"]
timestamp: "2026-07-11T21:27:07Z"
status: "canonical"
implementation_status: "implemented"
verification_status: "partial"
release_status: "unreleased"
audience: ["agent-authors", "client-authors", "contributors"]
---

# Agent orchestration contract (v1)

The pinned contract across four implementation surfaces:

- **Rust daemon** (`crates/jeliyad` + `crates/jeliya-core`) — implements the
  fleet read RPCs and the liveness derivation. `jeliya-core` remains the sole
  consumer of `iroh_rooms` (SDK pinned at rev `3cb9bfd`).
- **JS runner** (`scripts/jeliya-agent.mjs`, `scripts/jeliya-fleet.mjs`)
  — implements the status-vocabulary additions and the task-claim protocol.
  Node 22 (global `WebSocket`), zero npm deps.
- **React UI** (`ui/`) — implements the fleet dashboard behind the existing
  `agents` NavKey, sourced only from `agents.fleet` + `agent.history`.
- **Dart client and Flutter app** (`dart/jeliya_protocol`, `app/`) — expose the
  same fleet reads over WebSocket or FFI and implement the equivalent desktop
  and mobile dashboard without deriving liveness client-side.

Ground rules that bind every section below:

- **No new SDK wire event types.** All coordination rides on the existing
  `agent_status` and `message` events. The daemon adds only *pure read* RPC
  methods; nothing here changes what goes on the wire.
- **Wire limits** (enforced by the SDK, truncated by the runner):
  `agent_status` label ≤ 64 bytes, `status_message` ≤ 4096 bytes, message
  body ≤ 16384 bytes.
- **Honesty rules in `docs/PROTOCOL.md` stay authoritative.** Every count,
  liveness state, and history point below derives from a real stored event or
  a real peer-connection state (`peers.status` / `PeerConnState`). Nothing is
  ever synthesized, estimated, or carried forward past what the evidence
  supports.

---

## 1. Liveness — truthful, no heartbeat spam

### 1.1 Status-label vocabulary (runner side)

The runner posts plain `status.post` events with these labels. Two labels are
**reserved** by this contract (`claiming`, and `idle` as the post-task state);
the rest already exist in `jeliya-agent.mjs`:

| Label | When the runner posts it | Class |
|---|---|---|
| `online` | announce, right after `room.open` (exists today) | idle-class |
| `working` | throttled progress during a task (exists today) | working-class |
| `done` | task success, `progress: 100` (exists today) | idle-class |
| `failed` | task failure with the real reason (exists today) | idle-class |
| `idle` | posted after a task finishes and best-effort after losing claim arbitration, meaning the runner is not executing a task | idle-class |
| `offline` | shutdown (exists today, best-effort) | offline-class |
| `claiming` | task-claim handshake, see §2 | idle-class |

In `finishTask`'s caller (`startTask`'s `finally` path), the runner posts
`idle` with `"ready for the next task"` after the terminal `done` or `failed`
status. A runner that loses claim arbitration also posts one best-effort
`idle`; neither path is a heartbeat. **No periodic heartbeat events are ever
posted.** Any other label an agent posts (the label field is free-form up to
64 bytes) classifies as working-class if and only if it is exactly `working`;
unknown labels are idle-class.

`claiming` is deliberately idle-class: a claim is not execution. A runner that
loses a claim posts `idle` best-effort (§2); if that status post fails, its
latest label remains `claiming`, which still must not read as "working".

### 1.2 Derived liveness (daemon side, computed at read time)

An agent's `liveness` is one of exactly four strings:

```
"online-idle" | "working" | "offline" | "stale"
```

It is **derived, never stored**, from two real signals:

1. **Primary — peer connection state.** The agent's identity maps to its
   device keys (from the `member.joined` `device_binding` of that identity,
   plus the `device_id` on any event it has authored). The agent is
   **connected** iff the room is open on this daemon AND any of those devices'
   endpoints is currently `PeerConnState::Connected` on the session node
   (exactly the `peers.status` source; `Connecting`, `Offline`, and
   `Unauthorized` all count as not connected).
2. **Secondary — the agent's most recent event.** `latest_status` = the
   newest `agent_status` event by that identity in the room (label, `ts`);
   `last_seen_ts` = the `ts` of the newest event of *any* kind by that
   identity.

Constant: `STALE_WORKING_MS = 20 * 60_000` (20 minutes — deliberately above
the runner's 15-minute task hard cap plus reporting slack, so a healthy task
can never be misfiled as stale while its peer is connected).

**Decision table (evaluate top to bottom, first match wins):**

| # | Condition | Liveness |
|---|---|---|
| 1 | latest status label is `offline` (and peer not connected) | `offline` |
| 2 | peer NOT connected, latest label is working-class | `stale` |
| 3 | peer NOT connected (any other latest label, or no status yet) | `offline` |
| 4 | peer connected, latest label working-class, `now - latest.ts ≤ STALE_WORKING_MS` | `working` |
| 5 | peer connected, latest label working-class, `now - latest.ts > STALE_WORKING_MS` | `stale` |
| 6 | peer connected (idle-class latest label, or no status yet) | `online-idle` |

**THE RULE (fixes "stale working forever"):** a `working` latest status is
*never* sufficient to report `working`. `working` requires the agent's device
to be a currently-connected peer AND the working status to be fresh (rows 4
vs 2/5). A runner that crashed mid-task (never got to post `failed`) shows
`stale`, then `offline` reasoning applies as evidence dictates — the room is
never left watching a live "working" badge for a dead process. Peer state
always overrides the last posted label.

**Rooms the daemon has NOT open:** there is no live peer state, so the
primary signal is absent and connectivity cannot be verified. Row 1/2/3
semantics apply with "peer not connected" — i.e. the agent reports `offline`,
or `stale` if its latest label is working-class. It is dishonest to report
`online-idle`/`working` for a room we hold no live connection to.

Multi-room aggregation (for `agents.fleet`, §3): compute per-room liveness,
then take the strongest by presence: any room `working` → `working`; else any
`online-idle` → `online-idle`; else any `stale` → `stale`; else `offline`.

---

## 2. Task-claim coordination (over existing `agent_status`)

Goal: two agent runners in one room must not both execute the same `@trigger`
task — **best-effort**. This is eventual coordination over a gossiped event
DAG, **NOT a lock or lease**: a small double-run window remains whenever
gossip latency exceeds the settle window or a partition heals late. That is
the honest framing and the UI/docs must never call it mutual exclusion.

### 2.1 Wire format of a claim

A claim is an ordinary `status.post` (→ `agent_status` event):

- `label`: exactly `"claiming"` (reserved by §1.1).
- `status_message` (`message` param): MUST begin with the task token
  `task:<tok>` where `<tok>` is the **first 16 hex characters of the
  triggering `message` event's bare 64-hex `event_id`**, lowercase. Anything
  after the token must be separated by a single space and is free-form
  (recommended: the trigger sender's short id and the task head), within the
  4096-byte limit.

Example: `status_message = "task:9f3a1c0b7e2d4455 from 5b21ce… — build the report"`.

Parsers MUST match `/^task:([0-9a-f]{16})(\s|$)/` and ignore any `claiming`
status that does not match (fail closed: an unparseable claim is not a claim).

### 2.2 Deterministic winner

Among all `agent_status` events with label `claiming` bearing the **same task
token**, the winner is the claim with the **lexicographically lowest bare
64-hex claim `event_id`**. Event ids are content hashes, so this order is
global, deterministic, and identical on every peer once the events have
gossiped — no coordinator, no clock comparison.

### 2.3 Runner protocol

On an allowed, fresh, non-busy trigger (all existing checks in
`jeliya-agent.mjs` — allowlist, staleness, `current` busy gate — run first
and unchanged):

1. **Address filter (§2.4).** If the trigger is addressed to a different
   agent, ignore it silently. Applies before anything else.
2. **Count eligible agents** from the room's membership snapshot
   (`room.members`): active members with role `agent`. (The runner cannot
   know other runners' trigger config; role-`agent` membership is the honest
   proxy and MUST be documented as such.)
3. **If eligible count ≤ 1: skip claiming entirely.** Execute immediately —
   no claim event, no delay. Single-agent rooms and the existing agent-e2e
   flow are byte-for-byte unchanged.
4. **If eligible count > 1: post the claim** (`label: "claiming"`,
   token per §2.1), keeping the returned claim `event_id`.
5. **Settle window:** wait `CLAIM_SETTLE_MS = 1500` ms from the claim's local
   ack, collecting other agents' `claiming` statuses for the same token from
   `room.event` pushes, plus one `room.timeline` re-poll at the end of the
   window (pushes are lossy; the poll is the safety net).
6. **Decide:** if this runner's claim `event_id` is the lexicographic minimum
   of all observed claims for the token (including its own), it **proceeds**
   (`startTask`, which posts `working` as today). Otherwise it **stands down**:
   no execution and no chat reply. It posts `idle` with the message
   `"stood down - lost claim arbitration"` best-effort so operators can
   distinguish a completed arbitration from a stuck claim. If that status post
   fails, the failure stays in the local log and the latest wire label remains
   `claiming`, which is idle-class (§1.1).
7. A runner that becomes busy (`current` set) during the settle window stands
   down as a loser (never queue).

Honest limits, stated verbatim in the runner header comment: (a) two runners
whose gossip round-trip exceeds `CLAIM_SETTLE_MS` can both see themselves as
the minimum and both run; (b) a lower claim arriving after a winner started
does not abort the winner; (c) the deterministic order guarantees only that
all peers *eventually agree* on which claim won, bounding duplicates to the
propagation window. Duplicate `done` replies are possible and acceptable;
fabricating a stronger guarantee is not.

### 2.4 Addressed triggers

- `@agent <task>` — any agent whose trigger is `@agent` may claim/execute.
- `@agent:<id-prefix> <task>` — only the agent whose **identity id starts
  with `<id-prefix>`** (case-insensitive hex, recommended ≥ 8 chars) may
  claim/execute; every other agent ignores the message silently (no reply, no
  claim). If a short prefix matches more than one agent, all matching agents
  are eligible and the claim protocol (§2.3) arbitrates.

Trigger matching remains prefix-of-body with the existing
word-boundary rule (`matchesTrigger`): the addressed form is
`<trigger>:<prefix>` followed by whitespace-or-end, e.g.
`@agent:5b21ce90 run the tests`.

---

## 3. Fleet read RPCs (daemon; pure reads, no SDK change)

The shared protocol engine exposes two fleet methods over both WebSocket and
the in-process FFI transport. Both are **pure reads** over the existing folds
(`room_event_ids` → `validate_wire_bytes` → materialize) and live session state
(`peer_state` / `peer_entries`) — they author nothing, open no room, and invent
no counts. Full rows live in `docs/PROTOCOL.md`.

### 3.1 `agents.fleet`

Params: `{}`.

Result:

```json
{
  "active": 2,
  "working": 1,
  "total": 3,
  "rooms_total": 5,
  "rooms_covered": 2,
  "agents": [ /* FleetAgent */ ]
}
```

- Scope: the authorized **`room.list` set**, open or not: locally created or
  successfully joined rooms whose local membership remains readable, including
  joined-then-left archives. Foreign synchronized rows, invite-only rooms,
  corrupt rooms, and unsynced provenance entries are excluded. Liveness for
  non-open rooms follows §1.2 (never online).
- An **agent** is an identity whose folded role is `agent` in at least one
  room's membership snapshot.
- `total` = count of distinct agent identities across all rooms.
- `active` = agents whose aggregated liveness is `online-idle` or `working`.
- `working` = agents whose aggregated liveness is `working`.
- `rooms_total` = authorized listed rooms; `rooms_covered` = rooms with ≥ 1
  member of role `agent`. Invariants: `working ≤ active ≤ total`,
  `rooms_covered ≤ rooms_total`.

`FleetAgent`:

```json
{
  "identity_id": "…64-hex…",
  "rooms": [{ "room_id": "blake3:…", "name": "Build Iroh Rooms MVP" }],
  "liveness": "online-idle | working | offline | stale",
  "latest": {
    "label": "working",
    "message": "running: Bash (tool call 3)",
    "progress": null,
    "ts": 1783190000000,
    "room_id": "blake3:…"
  },
  "last_seen_ts": 1783190000000
}
```

- `rooms`: every room where the identity holds role `agent`; `name` is the
  genesis/local room name or `null`.
- `latest`: the newest `agent_status` event by that identity across those
  rooms (`message` = the event's `status_message` or `null`; `progress` =
  the event's progress or `null`; `room_id` = where it was posted), or
  `null` if the agent has never posted a status.
- `last_seen_ts`: `ts` of the newest event of any kind authored by that
  identity across those rooms, or `null`. This is an **event timestamp**,
  never "now", never extrapolated.
- Ordering: `agents` sorted by liveness rank (`working`, `online-idle`,
  `stale`, `offline`), then `last_seen_ts` descending, then `identity_id`.

### 3.2 `agent.history`

Params: `{ room_id, identity_id, limit? }` (`limit` = max points, default
100, most-recent-first selection returned in chronological order).

Result:

```json
{ "points": [ { "ts": 1783190000000, "label": "working", "progress": null } ] }
```

One point per real `agent_status` event authored by `identity_id` in
`room_id`, chronological. `progress` is the event's value or `null` — the
daemon MUST NOT interpolate, smooth, or fabricate intermediate points; a
sparkline drawn from this is a plot of actual events only. Errors:
`room_unknown` for an unknown room, `invalid_params` for a malformed
identity; an identity with no statuses returns `{ "points": [] }`.

---

## 4. Fleet config schema (`scripts/jeliya-fleet.mjs`)

A supervisor script that spawns several `jeliya-agent.mjs` runners from one
JSON file (`node scripts/jeliya-fleet.mjs --config fleet.json`). It only
spawns/monitors child processes and prefixes their logs; all room logic stays
in the runner.

```json
{
  "agents": [
    {
      "name": "builder-1",
      "room_id": "blake3:…",
      "ticket": "…",
      "peer": ["<endpoint_id>@<ip:port,…>"],
      "worker": "claude",
      "trigger": "@agent",
      "allow_sender": ["…64-hex…"],
      "data_dir": "/home/jeliya/.local/share/jeliya/agents/builder-1",
      "port": 7481,
      "loopback": false
    }
  ]
}
```

Per-entry fields (mapping 1:1 onto runner flags):

| Field | Required | Meaning |
|---|---|---|
| `name` | yes | local label for log prefixes; unique within the file |
| `room_id` | one of `ticket`/`room_id` | rejoin mode (`--room`) |
| `ticket` | one of `ticket`/`room_id` | first-join mode (`--ticket`); may be combined with `room_id` as a cross-check, exactly like the runner |
| `peer` | no | array of dial strings (repeatable `--peer`); a bare string is accepted as a one-element array |
| `worker` | yes | `"claude"` or `"echo"` |
| `trigger` | no | default `"@agent"` (`--trigger`) |
| `allow_sender` | no | array of 64-hex identity ids (repeatable `--allow-sender`); absent → runner's owner-default |
| `data_dir` | yes | unique absolute path outside source checkouts per agent — two runners must never share a daemon data dir (identity + stores) |
| `port` | yes | unique per agent, positive integer (each runner spawns its own daemon); tests use 7481–7489 |
| `loopback` | no | default `false` (`--loopback`) |

Validation is fail-fast before any spawn: unique `name`/`port`/`data_dir`, at
least one of `ticket` or `room_id`, and a known `worker`. Crash policy: a runner
that exits is logged and restarted with backoff at most N times (default 3);
restarts reuse `data_dir` (persisted identity) and switch `ticket` → rejoin
(`room_id`) after the first successful join. The fleet script posts no
statuses of its own and fabricates no fleet counts — the dashboard's numbers
come only from `agents.fleet`.

---

## 5. UI fleet dashboard data contract

The top-level Agents surface exists in both React
(`ui/src/components/FleetDashboard.tsx`) and Flutter
(`app/lib/src/screens/fleet_dashboard.dart`). It is distinct from a room's
right-panel Agents tab, which remains room-scoped and timeline-fold based.
Both clients load `agents.fleet` immediately, poll it every 4 seconds, and
debounce a refresh after `room.event` pushes. They fetch `agent.history` when
an agent's latest status or history room changes. The dashboards render
nothing that cannot be traced to those two results.

**Stat tiles** (top row, straight from `agents.fleet`):

| Tile | Value | Source |
|---|---|---|
| Active agents | `active` / `total` | `agents.fleet` |
| Working now | `working` | `agents.fleet` |
| Room coverage | `rooms_covered` / `rooms_total` | `agents.fleet` |

**Per-agent card** (one per `FleetAgent`):

- identity short id (first 12 hex + ellipsis; full id on hover/copy),
- liveness dot + label — the four §1.2 states verbatim (`stale` gets its own
  visual, never rendered as working),
- latest status: `latest.label` (prettified), `latest.message`, and a
  progress bar **only when `latest.progress` is a number** (no default 0%),
- room chips from `rooms[]` (name or short room id), each navigating to that
  room,
- "last seen" as relative time from `last_seen_ts` (or "never"),
- sparkline from `agent.history` points for the `latest.room_id` (label
  class as the y-band, real `ts` on x; points only — no interpolation).

**Add Agent** flow (security boundary — keep it):

1. User picks a room they own and pastes the agent's `identity_id` (obtained
   by running `jeliya-agent.mjs --identity-only` on the agent machine).
2. UI calls existing `invite.create` (`role: "agent"`) and reads the room's
   dialable `addr` from `room.open`/`daemon.status`.
3. UI shows the ticket plus a **copyable runner command**, e.g.
   `node scripts/jeliya-agent.mjs --ticket <T> --peer <ADDR> --worker claude`.

The clients **never spawn runner processes**; the daemon gets no "spawn agent"
RPC. Executing the command on the target machine is deliberately a human step.

---

## 6. Implementation and conformance checklist

These implemented boundaries remain the regression contract:

- **Core and daemon:** `agents.fleet` + `agent.history` dispatch arms;
  identity-to-device mapping and the §1.2 decision table; no writes and no new
  event types.
- **Runner:** `idle` after each task and lost arbitration; the §2.3 `claiming`
  handshake only when more than one active agent-role member is eligible; the
  §2.4 addressed-trigger filter; `CLAIM_SETTLE_MS = 1500`.
- **Fleet script:** the §4 schema, validation, process spawn, and bounded
  restart behavior only. Room-level truth still comes from `agents.fleet`.
- **React and Flutter:** equivalent fleet dashboards backed only by the two
  read RPCs; the room-scoped Agents tabs remain separate.
- **Tests:** `scripts/fleet-e2e.mjs` proves multi-agent claims, fleet reads,
  histories, and crash liveness; Rust, Dart, React, and Flutter suites pin the
  corresponding projections and client behavior.
