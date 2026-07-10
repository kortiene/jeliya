# Jeliya daemon protocol (v1)

The contract between `jeliyad` (the resident Rust core, sole consumer of the
`iroh-rooms` SDK) and any Jeliya shell (desktop web UI, scripts, e2e tests).

- Transport: **WebSocket**, JSON text frames, `ws://127.0.0.1:<port>/ws`
  (default port **7420**, `--port` to override; `--port 0` lets the OS pick —
  read the truth back from the ready line or portfile). Local-only: the daemon
  binds `127.0.0.1` and MUST refuse to bind non-loopback interfaces in v1.
  On mobile the same frames travel over an in-process FFI bridge instead of a
  socket — see *In-process transport (FFI)* below.
- Authenticated: every `/ws` connect (and `/api/files/*` request) must present
  the daemon's per-start token — `?token=<hex>` or `Authorization: Bearer`.
  See **Process supervision** below for how each client kind obtains it.
- The daemon is long-running by design: only a live node serves blob fetches
  and pipe forwards to peers.

## Process supervision

The contract for any parent process (the desktop app, an agent script, a
service manager) that owns `jeliyad` as a sidecar — and for second clients
attaching to a daemon someone else started.

### Spawn and the ready line

Spawn `jeliyad --supervised [--port 0] [--data-dir <dir>]`. The **first JSON
line on stdout** is the machine-readable contract (human-readable lines may
follow; parse the first line that starts with `{`):

```json
{ "event": "ready", "pid": 4242, "port": 54443,
  "http": "http://127.0.0.1:54443/", "ws": "ws://127.0.0.1:54443/ws",
  "version": "0.4.3", "protocol": 1,
  "data_dir": "/Users/x/Library/Application Support/Jeliya",
  "portfile": "…/Jeliya/daemon.json" }
```

`--supervised` additionally means: the daemon shuts down gracefully when its
stdin reaches EOF (the portable parent-death signal on all three OSes — hold
the child's stdin pipe open and it dies within seconds of you dying, even on
`kill -9` of the parent), and it never auto-opens a browser.

### The portfile (`<data_dir>/daemon.json`)

Written atomically after bind, removed on graceful shutdown, `0600` on Unix.
The canonical discovery point for native clients (scripts, the desktop app
adopting a daemon it did not spawn):

```json
{ "schema": 1, "pid": 4242, "port": 54443, "http": "…", "ws": "…",
  "version": "0.4.3", "protocol": 1, "data_dir": "…",
  "auth_token": "…64-hex…", "started_at_ms": 1783190000000 }
```

A portfile can be **stale** (crash skipped cleanup): never trust it blind —
health-check before adopting.

### Auth token

Minted fresh per daemon start. Distribution is deliberately split:

- **Native clients** read `auth_token` from the portfile.
- **The browser UI** calls `GET /api/session`. It answers two browser shapes:
  the packaged UI served from the daemon's own origin (a same-origin GET, which
  carries no `Origin` header — accepted via the browser-set, page-unforgeable
  `Sec-Fetch-Site: same-origin`), and the cross-origin dev server on
  `localhost:5173` (accepted via a loopback `Origin`, mirrored back as CORS).
- `GET /api/health` is the one unauthenticated endpoint: liveness + identity
  (`{ ok, pid, port, version, protocol, data_dir }`), secret-free, used for
  adoption checks.
- All of `/ws` and `/api/*` refuse requests whose `Host` header is not
  loopback (DNS-rebinding guard), and `/ws` still refuses any non-loopback
  browser `Origin` (cross-site WebSocket hijacking guard).

**Threat model — read this honestly.** The trust boundary is a **single-user
machine**: any process running as the same user can already read the 0600
portfile, so the token grants such a process nothing it could not otherwise
obtain. The `Origin` / `Sec-Fetch-Site` checks on `/api/session` defend against
hostile *web pages* in a real browser (which cannot forge those headers), **not**
against a local non-browser process (`curl` can set any header). A shared
multi-user machine is therefore **out of scope**: a different local user who can
reach `127.0.0.1` could obtain the token via `/api/session`. The 0600 portfile
is best-effort defense-in-depth, not a hard cross-user guarantee, because the
loopback HTTP surface is inherently readable by any local user. (Peer-credential
checks on the loopback socket would close this and are noted as future
hardening.) Files fetched from room peers are always served as
`Content-Disposition: attachment` with `nosniff` and an inert content-type, so a
peer cannot get script to run in the daemon's origin and lift the token.

### Single instance and adoption

One daemon per data dir, enforced with an OS advisory lock
(`<data_dir>/daemon.lock`) held for the daemon's life — the OS releases it on
any death, so a crash cannot wedge the data dir. A second spawn on the same
data dir health-checks the incumbent and prints (then exits **0**):

```json
{ "event": "already_running", "pid": 4242, "port": 54443, "http": "…",
  "ws": "…", "version": "0.4.3", "protocol": 1, "data_dir": "…", "portfile": "…" }
```

A supervisor's spawn algorithm is therefore: spawn → parse first JSON line →
`ready` means you own it; `already_running` means adopt (read the portfile for
the token). Exit 1 with no JSON line means the data dir is wedged mid-start;
retry briefly.

**Version skew (adopt-vs-respawn rule):** before adopting, compare the line's
`protocol` to the one you were built against. Same `protocol` → adopt
(`version` may differ; the protocol contract is what matters). Different
`protocol` → do NOT adopt and do NOT spawn a second daemon: ask the running
daemon to exit (`daemon.shutdown`, or SIGTERM its `pid`), then respawn your
bundled binary.

### Shutdown

Three equivalent triggers, all running the same graceful teardown (close every
open room session — releasing blob locks — then remove the portfile):

- `SIGTERM` / `SIGINT` (Unix; console Ctrl-C on Windows),
- stdin EOF in `--supervised` mode (parent death),
- the `daemon.shutdown` RPC (authenticated; replies `{ shutting_down: true }`
  first, then exits).

Teardown is bounded (~10s); a hung room cannot turn SIGTERM into a zombie.

### Native file sharing (staging convention)

`file.share {room_id, path}` refuses paths outside the daemon's data dir — the
anti-exfiltration invariant for a loopback daemon. A native client sharing an
arbitrary user file therefore **stages** it: copy into
`<data_dir>/uploads/<unique-name>`, call `file.share` with the staged path,
then delete the staged copy after the call returns (the daemon has imported
the blob by then). This mirrors what the daemon itself does for browser
uploads on `POST /api/files/share`.

## Envelope

Client → server (request):

```json
{ "id": 42, "method": "room.create", "params": { "name": "Build Iroh Rooms MVP" } }
```

Server → client (response — exactly one per request id):

```json
{ "id": 42, "ok": true, "result": { "room_id": "blake3:..." } }
{ "id": 42, "ok": false, "error": { "code": "bad_ticket", "message": "...", "hint": "..." } }
```

Server → client (push — no id):

```json
{ "push": "room.event", "data": { "room_id": "...", "event": { /* TimelineEvent */ } } }
{ "push": "peers.changed", "data": { "room_id": "...", "peers": [ /* PeerStatus */ ] } }
```

`error.code` mirrors the SDK/CLI taxonomy where one exists:
`invalid_params`, `identity_missing`, `identity_exists`, `not_a_member`,
`room_unknown`, `room_not_open`, `bad_ticket`, `ticket_expired`,
`file_unavailable`, `file_unauthorized`, `hash_mismatch`, `pipe_denied`,
`peer_unreachable`, `internal`. `hint` is a next-action line (the CLI's
IR-0303 convention) or `null`.

**Client-synthesized error codes.** Two codes never cross the wire — the
daemon never sends them — but every client MUST mint them for the same
conditions so error handling is portable:

- `connection_lost` — the transport failed before a response arrived. **The
  request may or may not have executed** (at-least-once; see the idempotency
  note under `message.send`). Reserved; do not reuse for anything else.
- `internal` — a client-side fallback when a failure has no better code (e.g.
  a malformed frame). This overlaps the daemon's own `internal`, which is
  intended.
- `file_too_large` / `file_unreadable` — share-staging failures a client
  detects locally before any wire call (picked file exceeds the 100 MiB share
  cap / cannot be read from disk). Distinct codes exist so UI copy keys off
  the code, never off parsing English message text. Never sent by the daemon.

Per-method error notes (which *distinctive* wire codes a method can return) are
inline with each method below. On top of those, several codes are
**cross-cutting** — they can come back from any method that hits the
corresponding precondition, whether or not the method lists them:

- `invalid_params` and `internal` — any method (bad params; unexpected failure).
- `identity_missing` — any method that needs the local identity (everything
  except `daemon.status`, `daemon.shutdown`, `identity.create`).
- `room_unknown` — any method taking a `room_id` for a room this daemon has no
  history of (`room.timeline`, `room.members`, `pipe.list`, … included).
- `room_not_open` — any method that needs a live room session
  (`message.send`, `status.post`, `file.share`, `file.fetch`, `pipe.*`).
- `not_a_member` — any authoring method when this identity is not an active
  member (`message.send`, `status.post`, `file.share`, …).

A client MUST handle these cross-cutting codes on every relevant call; the
inline notes only add codes that are *specific* to a method.

## Protocol version & forward compatibility

The wire contract is versioned by a single **major** integer,
`protocol` (currently `1`). It is surfaced three ways — `daemon.status.protocol`,
`GET /api/health`'s `protocol`, and the `ready` line / portfile `protocol` — so
a client can range-check before assuming this contract. There is **no
connect-time negotiation today**: `/ws` validates only Origin + token and the
server sends no greeting frame, so a client MUST read `daemon.status` once after
connecting and treat a `protocol` it does not support as a hard incompatibility
(for the same-host sidecar case, the adopt-vs-respawn rule under *Process
supervision* applies).

**Forward-compatibility rules (normative — these keep v1 unbreakable as the
protocol grows):**

1. Clients **MUST ignore unknown top-level keys** in the envelope, in `result`
   objects, in `TimelineEvent`, and in push `data`. New optional fields are
   added without a major bump; a client that rejects them is non-conformant.
   (The reference client already does this — it reads only the keys it knows.)
2. Clients **MUST ignore `TimelineEvent` `kind` values they do not recognize**
   rather than erroring — render them as an inert "unsupported event" or skip
   them. This is what lets a lower-`protocol` peer coexist with a higher one in
   the same P2P room: you fold the events you understand and pass over the rest.
3. A higher `protocol` is only assumed backward-compatible across the **same
   major**. A major bump may remove or reshape fields and requires an explicit
   client update.

**Reserved for a future minor (named now so adding them is non-breaking, not
implemented today):**

- `daemon.status` / `ready` line / portfile MAY gain `min_protocol` (the lowest
  major the daemon still speaks) so a forward client can range-check a peer
  without a new endpoint.
- `/ws` MAY accept a `?protocol=<n>` connect param, or the server MAY send a
  first `hello` frame carrying `{ protocol, min_protocol }`, replacing the
  post-connect `daemon.status` round-trip.

## View-models

### TimelineEvent

One validated room event, folded for display. `kind`-specific fields are
present only for that kind.

```json
{
  "event_id": "…64-hex…",
  "room_id": "blake3:…",
  "ts": 1783190000000,
  "sender": { "identity_id": "…64-hex…", "device_id": "…64-hex…", "role": "owner|member|agent" },
  "kind": "room_created | member_invited | member_joined | member_left | message | agent_status | file_shared | pipe_opened | pipe_closed",

  "body": "hello",                                             // message
  "label": "running_tests", "status_message": "…",             // agent_status
  "progress": 60, "artifacts": ["file_…32-hex…"],              // agent_status (optional; artifacts omitted when empty)
  "file": { "file_id": "file_…", "name": "PRD.pdf", "size": 123, "mime": "application/pdf" },  // file_shared
  "pipe": { "pipe_id": "…32-hex…", "target": "127.0.0.1:3000", "authorized_peer": "…identity…|null" }, // pipe_opened / pipe_closed
  "member": { "identity_id": "…", "role": "member" }           // member_invited / member_joined; member_left omits role
}
```

Field notes a second client MUST honor:

- **`pipe`** — on `pipe_closed`, both `target` and `authorized_peer` are
  `null`. On `pipe_opened`, `authorized_peer` is `null` when no peer is scoped,
  or a **comma-joined** identity list for multi-identity authorization.
- **`artifacts`** — present only when non-empty; treat absent as `[]`.
- **`ts`** is the event's signed timestamp in ms, **not** arrival time. It can
  be older than events already delivered (see *Pushes* — late backlog).

**Reserved headroom (not emitted today; named so adding it stays a non-breaking
minor per the forward-compat rules).** Before any golden-frame corpus freezes
this shape, these extension points are declared so future work does not need a
major bump:

- **Queued / offline delivery.** A `TimelineEvent` MAY later carry an optional
  `"delivery": "live" | "queued" | "resent"` (absent ⇒ `"live"`), and the
  `room.event` push `data` MAY carry the same marker, so a future
  store-and-forward relay (or a "home-peer mailbox") can flag replayed frames.
  This is why honesty rule 1 forbids a *delivered confirmation*, not a
  *pending/queued* state — the latter stays addable.
- **Voice notes and rich blobs.** A voice note is a `file_shared` with
  `mime: audio/*`. The `file` object MAY additively gain `kind`
  (e.g. `"voice"`), `duration_ms`, and `waveform`. **Split by origin:**
  daemon-*derived* fields (kind inferred from mime, duration probed from the
  local blob) can appear in the materialized view at any time, non-breaking;
  sender-*authored* fields that must travel P2P require a field on the signed
  `iroh-rooms` `FileShared` content and therefore MUST be reserved/added before
  the corpus freezes or they become a v2-breaking change. Only the reservation
  is made here; nothing is emitted yet.

### PeerStatus

```json
{ "endpoint_id": "…", "state": "connected|connecting|offline", "path": "direct|relay|null", "identity_id": "…64-hex…|null" }
```

`identity_id` is null until the SDK has bound that device to a membership identity (on admit) — expect null before/during admission, not just for strangers.

## Methods

### Daemon & identity

| Method | Params | Result |
|---|---|---|
| `daemon.status` | `{}` | `{ version, protocol, pid, port, data_dir, mode: "loopback"\|"real", identity: {identity_id, device_id} \| null, endpoint: {endpoint_id, addr, relay_url} \| null, rooms_open: [room_id] }` |
| `daemon.shutdown` | `{}` | `{ shutting_down: true }` — replies, then runs the graceful teardown (see Process supervision) |
| `identity.create` | `{}` | `{ identity_id, device_id }` — errors `identity_exists` if one exists |

`endpoint.addr` is a dialable `<endpoint_id>@<ip:port>` string when known
(loopback mode always knows it), else `null`.

### Rooms

| Method | Params | Result |
|---|---|---|
| `room.create` | `{ name }` | `{ room_id }` — name is daemon-local metadata if the protocol has no name field |
| `room.list` | `{}` | `{ rooms: [{ room_id, name, role, status, member_count, open }] }` — `status` is this identity's roster status (`active|invited|left|removed|null`); **`name` is `null`** until the genesis event syncs and **`role` is `null`** when this daemon has no local identity |
| `room.open` | `{ room_id, peers?: ["<endpoint_id>@<ip:port>"] }` | `{ endpoint: { endpoint_id, addr }, members, timeline }` — spawns the room's node session, starts pushes; `peers` are optional dial hints merged into the persisted hint set (same shape as `room.join`); `addr` is the dialable string an inviter shares with joiners. **`room.open` succeeds locally regardless of peer reachability** — unreachable hints surface as an empty/stale timeline that later syncs, *not* as an error (distinctive error: `not_a_member`; plus cross-cutting `room_unknown`) |
| `room.close` | `{ room_id }` | `{}` — closes only this daemon's live session; membership remains active |
| `room.leave` | `{ room_id }` | `{ event_id }` — authors `member.left` for this identity and closes the local session; owners are rejected until ownership transfer exists |
| `room.timeline` | `{ room_id, limit? }` | `{ events: [TimelineEvent] }` (chronological) — see the resync note below |
| `room.members` | `{ room_id }` | `{ members: [{ identity_id, role, status }] }` |
| `invite.create` | `{ room_id, identity_id, role: "member"\|"agent", expiry? }` | `{ ticket }` — `expiry` accepts a **duration string** (`"24h"`, `"90m"`, `"3600"`) **or a number of seconds**; omitted ⇒ single-use, not time-boxed. Minted on a **closed** room too. Errors: `not_a_member` (caller is not the room admin), `invalid_params` (self-invite, bad role, or non-64-hex invitee) |
| `room.join` | `{ ticket, name?, peers?: ["<endpoint_id>@<ip:port>"] }` | `{ room_id }` — errors: `bad_ticket` (malformed or bound to a different identity), `ticket_expired`, `peer_unreachable` (no reachable discovery hint) |

**Timeline resync (normative + reserved).** Live `room.event` pushes are lossy
and never re-sent (see *Pushes*), so a client **MUST** re-sync a room's timeline
after any reconnect. Today `room.open` returns the full `timeline` and
`room.timeline` takes only `limit`, so re-sync pulls the whole log. Reserved for
a future minor (non-breaking): `room.timeline` MAY gain an `after_event_id`
(or `since_ts`) cursor so a reconnecting client fetches only the delta after a
push gap — the name is reserved now because it matters most on metered/flaky
mobile links.

### Messages & agent status

| Method | Params | Result |
|---|---|---|
| `message.send` | `{ room_id, body }` | `{ event_id }` — `body` is 1..=**16384** UTF-8 bytes (16 KiB); see the idempotency note below |
| `status.post` | `{ room_id, label, message?, progress?, artifacts? }` | `{ event_id }` — any active member may post (protocol rule); `progress` is `0..=100`, `artifacts` is at most **16** valid file ids |

**`message.send` has no idempotency key (normative gap).** The params carry no
client-supplied id, so if a send fails with `connection_lost` *after* the daemon
already authored the event, a retry authors a **second** event with a new
`event_id` — a duplicate. A client therefore accepts at-least-once delivery on
retry, or surfaces the ambiguity to the user. Reserved for a future minor
(non-breaking): an optional `client_msg_id` the daemon echoes into the event
for exactly-once reconciliation — but that requires a field on the signed
`iroh-rooms` content, so it is named here, not yet implemented.

### Files

| Method | Params | Result |
|---|---|---|
| `file.share` | `{ room_id, path, name?, mime? }` | `{ file_id, event_id }` — imports into the blob store and authors `file.shared`; `path` MUST resolve inside the daemon data dir (see *Native file sharing*), file size ≤ **100 MiB** (`104_857_600` bytes) |
| `file.list` | `{ room_id }` | `{ files: [{ file_id, name, size, mime, sender_id, ts, available, providers, fetched?, local_path?, local_bytes?, fetched_at_ms? }] }` — see the availability note below |
| `file.fetch` | `{ room_id, file_id, save_dir? }` | `{ path, bytes, verified: true }` — writes into `save_dir`, defaulting to **`<data_dir>/downloads`** when omitted; distinctive errors `file_unavailable` / `file_unauthorized` / `hash_mismatch`, never a silent partial. `hash_mismatch` is a hard stop (discard, no retry) |

**`available` and `providers` (file.list) — read carefully, the meaning is
non-obvious:**

- **`available`** is `true` only when **some *other* provider device is a
  currently-connected peer in this open room**. It deliberately does **not**
  count a copy this daemon holds locally: a file you shared yourself reads
  `available: false` when no other online provider exists, and everything reads
  `available: false` while the room is closed. It answers "can I fetch this
  right now from someone?", not "does a copy exist somewhere".
- **`providers`** is the total number of provider devices recorded in the
  signed history — **not** the online count. `providers: 3, available: false`
  is normal (three known providers, none currently connected).

A client MUST render availability from `available` (with `providers` as
context), never infer "have it / can get it" from `providers` alone.

Browser UI upload helper: because a browser file picker cannot reveal a real
local filesystem path, `jeliyad` also serves `POST /api/files/share` on the
same loopback origin as the UI. Query params are `{ room_id, name, mime? }`; the
request body is the raw file bytes. The endpoint rejects non-local `Origin`s,
stages the bytes under the daemon data dir, calls the same confined
`file.share` import path, then removes the staged copy. Its JSON envelope is
`{ ok: true, result: { file_id, event_id } }` or
`{ ok: false, error: { code, message, hint } }`.

Browser UI local-open helper: `GET /api/files/local?room_id=<room_id>&file_id=<file_id>`
serves a previously fetched local copy from the daemon's loopback origin. The
browser never supplies a filesystem path; the daemon resolves `(room_id,
file_id)` against verified local fetch state under `<data_dir>/downloads`, then
returns the file as a **download** (`Content-Disposition: attachment`,
`X-Content-Type-Options: nosniff`, inert content-type — a peer-supplied file is
never rendered inline in the daemon origin). Missing or stale local copies
return the standard JSON error envelope.

### Pipes

| Method | Params | Result |
|---|---|---|
| `pipe.expose` | `{ room_id, target: "127.0.0.1:3000", peer_identity }` | `{ pipe_id, event_id }` — one authorized peer (runtime rule); `target` MUST be a numeric **loopback** `ip:port`, `peer_identity` MUST be 64-hex. Distinctive errors: a **hostname** (e.g. `localhost:3000`) or otherwise non-`ip:port` target is `invalid_params`; a numeric **non-loopback** target (e.g. `8.8.8.8:80`) is `pipe_denied`; a non-64-hex `peer_identity` is `invalid_params` |
| `pipe.list` | `{ room_id }` | `{ pipes: [{ pipe_id, target, opened_by, authorized_peer, state: "open"\|"closed", connected }] }` — `authorized_peer` is `null` when unscoped or a **comma-joined** list for multi-identity |
| `pipe.connect` | `{ room_id, pipe_id }` | `{ local_addr }` — local forwarded address to point a browser/iframe at. Distinctive errors: `invalid_params` (the pipe **owner** connecting to its own pipe, or an unknown/unsynced `pipe_id`), `peer_unreachable` (the pipe owner is offline), `pipe_denied` (the connection is refused) |
| `pipe.close` | `{ room_id, pipe_id }` | `{ event_id }` |

### Peers

| Method | Params | Result |
|---|---|---|
| `peers.status` | `{ room_id }` | `{ peers: [PeerStatus] }` |

### Agents (fleet reads)

Pure reads over the existing event folds and live peer state — they author
nothing and require no SDK change. Full semantics (liveness derivation, claim
protocol, aggregation rules) live in `docs/agent-orchestration.md`.

| Method | Params | Result |
|---|---|---|
| `agents.fleet` | `{}` | `{ active, working, total, rooms_total, rooms_covered, agents: [FleetAgent] }` — aggregated across all locally known rooms; every count derives from folded events + live `PeerConnState`, never estimated |
| `agent.history` | `{ room_id, identity_id, limit? }` | `{ points: [{ ts, label, progress }] }` — one point per real `agent_status` event by that identity, chronological (`limit` newest, default 100); no interpolation |

`FleetAgent`:

```json
{
  "identity_id": "…64-hex…",
  "rooms": [{ "room_id": "blake3:…", "name": "…" }],
  "liveness": "online-idle | working | offline | stale",
  "latest": { "label": "…", "message": "…", "progress": null, "ts": 1783190000000, "room_id": "blake3:…" },
  "last_seen_ts": 1783190000000
}
```

`liveness` is derived at read time, never stored: **primary** signal is
whether one of the agent's devices is a `connected` peer in an open room
(`peers.status` source); **secondary** is the timestamp of its most recent
event. `working` requires a connected peer AND a fresh working-class latest
status; a working-class latest status with a disconnected peer reports
`stale`/`offline`, never `working`.

Reserved `agent_status` labels: `claiming` (task-claim handshake — a claim's
`status_message` starts with `task:<first 16 hex of the triggering message
event_id>`; the lexicographically lowest claim `event_id` per token wins) and
`idle` (posted by a runner when a task finishes). Claiming is best-effort
eventual coordination, not a lock — see `docs/agent-orchestration.md` §2.

## Pushes

| Push | Data | When |
|---|---|---|
| `room.event` | `{ room_id, event: TimelineEvent }` | a new validated event is ingested (own or remote), at most once per event — a slow/lagging session may miss pushes (bounded broadcast buffer) and they are never re-sent; consumers needing completeness must re-sync via `room.timeline` |
| `peers.changed` | `{ room_id, peers: [PeerStatus] }` | any peer connection state change |

The `timeline` returned by `room.open` (and `room.timeline`) is the
authoritative chronological baseline that live `room.event` pushes splice into.
Two ordering hazards are **normative** for every client — the daemon does not
pre-resolve them, and the reference UI handles both:

1. **Insert by `ts`, not arrival; dedup by `event_id`.** A peer that reconnects
   after a gap has its backlog validated late, so a `room.event` can carry a
   `ts` **older** than events already shown. A client MUST: skip the event if
   its `event_id` is already present (idempotent — the same event can arrive via
   both the live pump and the reconcile scan), then insert it at the position
   its `ts` dictates rather than appending. Equal `ts` ties break by arrival
   order (append after existing equal-`ts` events) for a stable render. Blind
   append renders out of order (and, in the reference UI, emits a stray day
   divider).
2. **The echo of your own write can beat its response.** When you
   `message.send` / `status.post` / `file.share`, the daemon fans out the
   `room.event` for your own event on the same broadcast path — it can arrive
   **before** the method response resolves. Both frames carry the **same
   `event_id`**, which is the only correlation key (the request carries no
   client id). A client MUST converge on exactly one rendered item by matching
   echo↔response by `event_id` at all three points it can observe them: the
   method response, the live push, and the `room.open` backlog on the next open.
   The reference UI's optimistic pending-message lifecycle (below) is one
   implementation.

## Connection lifecycle

The only connection signal is the raw WebSocket transport open/close — the
daemon sends **no** application-level greeting, session, or heartbeat frame, and
never initiates pings (it only answers a client `Ping` with `Pong`). A
conformant client therefore:

- **Reconnects with backoff** on close (the reference client uses 500ms→8s
  exponential backoff + jitter). The four client-side states
  `connecting | connected | reconnecting | disconnected` are the recommended
  model (mirrored in `protocol.ts`), but they are a client concern, not a wire
  message.
- **Re-authenticates every attempt** by fetching a fresh per-start token (a
  daemon restart mints a new one — see *Process supervision*). A stale token is
  refused `401`; re-fetching heals a restart transparently.
- **Re-syncs every open room after any reconnect**, because pushes are lossy and
  never replayed (see *Pushes*). Until the cursor reservation lands, that means
  re-reading `room.open` / `room.timeline` in full.

## In-process transport (FFI)

On mobile there is no sidecar process (iOS forbids spawning one), so the same
core runs **in-process**: the app drives `crates/jeliya-ffi` over `dart:ffi`
(the `FfiClient` in `dart/jeliya_protocol`), and every request, response, and
push travels as the **same JSON envelope frames** defined above. The bridge is
a pure pass-through, and both transports share one dispatch implementation
(`jeliya_core::engine::Engine`), so the golden conformance corpus replays
against this transport unchanged — it is the third oracle next to the daemon
and the mock, and that host replay is what "conformant" means for it today
(host-conformance-verified). Because the bridge never interprets frame
contents, every reserved minor above (`client_msg_id`, the
`after_event_id`/`since_ts` cursor, the `delivery` marker, `min_protocol`)
rides through it with no bridge change.

What changes is everything that presumed a socket and a second process — each
piece reinterpreted truthfully, never simulated:

- **Connection = engine lifecycle.** `connecting` while the engine
  initializes, `connected` once dispatch is servable, `disconnected` only
  after `stop()` (or a failed start). There is **no `reconnecting` state**: no
  transport exists that can drop independently of the app process, and
  fabricating one would break the honesty rules (the state renders in
  Settings).
- **`daemon.status` stays truthful.** `port` is `0`, meaning *no listener* —
  unambiguous, because a bound daemon can never truthfully report 0; `pid` is
  the app's **own** process id (the engine's process *is* the app); `version`
  is the compiled core crate's version; `data_dir` is the app's engine
  directory.
- **No token, portfile, or HTTP surface.** The *Process supervision* contract
  (ready line, portfile, auth token, adopt-vs-respawn) and the `/api/*`
  endpoints do not exist in-process; the trust boundary they defended
  collapses into OS app-process isolation. Native file sharing keeps the same
  staging convention (copy into `<data_dir>/uploads`, `file.share`, delete) —
  pure file I/O, no HTTP. The version-skew rule still binds: read
  `daemon.status` once after start and treat an unsupported `protocol` as a
  hard incompatibility (in-process it guards Dart-package vs compiled-core
  build skew).
- **Re-sync on app resume.** Pushes remain lossy (same bounded broadcast
  buffer) and the mobile OS suspends the process, but the reconnect that
  triggers re-sync on socket transports can never happen here. The client
  therefore re-runs the full re-sync (`room.open` / `room.timeline` re-read,
  as under *Connection lifecycle*) on app-lifecycle **resume** — the honest
  in-process equivalent of a transport gap.
- **`daemon.shutdown` performs real teardown.** It replies
  `{ shutting_down: true }`, then actually stops the push loop, closes every
  open room (releasing blob locks), and drops the engine — never a
  reply-without-teardown.
- **"Long-running by design" does not hold on mobile.** While the OS has the
  app suspended, the node serves no blob fetches or pipe forwards — file
  `available` counts and pipe listeners degrade in background (foreground
  service work is deferred and tracked). The local-open URL
  (`GET /api/files/local`) has no in-process equivalent yet; a native engine
  accessor is the tracked follow-up.

## Honesty rules (bind the UI too)

1. Delivery is best-effort P2P: there is no "delivered" **confirmation** — never
   fabricate one. Files show `available` / provider counts. (A future *pending*
   or *queued* state is not forbidden — see the delivery-headroom reservation
   under TimelineEvent — but a green "delivered" checkmark that outruns the
   truth is.)
2. Peer path (`direct`/`relay`) is shown truthfully from the runtime's
   diagnostics. Gate A has confirmed direct P2P across one different-network
   pair (see `docs/gate-a-result.md`), but relay fallback remains expected on
   NAT pairs that cannot hole-punch — do not hide relay fallback.
3. Fetch failures surface the taxonomy (`unavailable` / `unauthorized` /
   `hash_mismatch`) — a verification failure is a hard stop, not a retry.
4. Agent liveness and fleet counts derive only from real events and real
   peer-connection state: never report `working` for a disconnected peer,
   never fabricate progress or heartbeats, never extrapolate `last_seen`.

## Client conventions (non-normative)

These are **not** wire contract — a second client is free to differ — but they
carry the honesty rules into the UI, so a Jeliya client SHOULD mirror them for
cross-client parity. The reference implementations are cited.

### Agent-status label tone (mirror exactly)

`agent_status.label` is free-form wire data; its color is derived, and **green
must be earned** (honesty rule 4) — a label this contract cannot read renders
neutral, never a reassuring green. Algorithm
(`dart/jeliya_protocol/lib/src/conventions/format.dart` `labelTone`; the
retiring web client's `ui/src/lib/format.ts` is the historical source),
applied to the label lowercased with `_`/`-` collapsed to spaces, in this
precedence:

1. **red** if it *contains the substring* `fail`, `error`, or `block` (substring
   on purpose — a false alarm is the honest direction to over-match);
2. else **blue** on the *word-boundary* set `await(ing)?`, `review(ing|ed)?`,
   `pend(ing)?` (word-boundary is load-bearing: `review` must not match inside
   `preview`, so `preview_ready` stays green not blue);
3. else **green** on the *word-boundary* set `done|working|online|ready|pass|
   passed|success|successful|ok|complete|completed|connected|healthy|active|
   running|verified|live`;
4. else **neutral** (any unknown or non-English label).

### File fetch: "verified" vs "fetched" (never downgrade)

Distinguish two states so the integrity story stays honest (rules 1, 3):

- **verified** — set only from a live `file.fetch` result *this session*
  (`verified: true`); asserts these bytes were hash-checked now.
- **fetched** — reconstructed from `file.list` persisted fields
  (`fetched && local_path`); asserts only that the daemon reports a prior local
  copy.

A `file.list` refresh MUST NOT downgrade an entry that is already `verified` (or
mid-`fetch`) back to plain `fetched`. `hash_mismatch` is a hard stop: discard
the copy, never render it, no retry.

### Optimistic pending-message lifecycle (reference pattern)

The reference UI renders your own message immediately with a client-local id
(never sent on the wire) in phases `sending → syncing → failed`, then reconciles
against the real event by `event_id` at the three points named under *Pushes*.
On the ambiguous `connection_lost`-after-send case it shows a **Retry** that
re-sends — accepting the duplicate risk from the `message.send` idempotency gap.
A client MAY choose differently, but MUST reconcile echo↔response by `event_id`.

### Presentation-only, client-local

Free to differ per client; **not** semantic:

- **5-minute same-sender grouping** and **day dividers** are display-only and
  computed in the client's **local timezone** — two clients in different zones
  legitimately draw different dividers. Only the `ts` ordering (see *Pushes*) is
  semantic; the divider is presentation and depends on that ordering being
  correct.
- **Last-room restore** (precedence: in-memory current → persisted → first
  active, filtered to non-`left`/`removed`), **per-room drafts** (restored if a
  send throws), and **local display aliases** are client-local storage
  (`localStorage` in the web UI; a Dart client uses its own store). Aliases are
  device-local **by necessity** — the protocol has no display-name field, so
  names must never be treated as wire data.
