# Bantaba daemon protocol (v1)

The contract between `bantabad` (the resident Rust core, sole consumer of the
`iroh-rooms` SDK) and any Bantaba shell (desktop web UI, scripts, e2e tests).

- Transport: **WebSocket**, JSON text frames, `ws://127.0.0.1:<port>/ws`
  (default port **7420**, `--port` to override). Local-only: the daemon binds
  `127.0.0.1` and MUST refuse to bind non-loopback interfaces in v1.
- The daemon is long-running by design: only a live node serves blob fetches
  and pipe forwards to peers.

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
  "kind": "room_created | member_invited | member_joined | message | agent_status | file_shared | pipe_opened | pipe_closed",

  "body": "hello",                                             // message
  "label": "running_tests", "status_message": "…",             // agent_status
  "progress": 60, "artifacts": ["file_…32-hex…"],              // agent_status (optional)
  "file": { "file_id": "file_…", "name": "PRD.pdf", "size": 123, "mime": "application/pdf" },  // file_shared
  "pipe": { "pipe_id": "…32-hex…", "target": "127.0.0.1:3000", "authorized_peer": "…identity…" }, // pipe_opened / pipe_closed
  "member": { "identity_id": "…", "role": "member" }           // member_invited / member_joined
}
```

### PeerStatus

```json
{ "endpoint_id": "…", "state": "connected|connecting|offline", "path": "direct|relay|null" }
```

## Methods

### Daemon & identity

| Method | Params | Result |
|---|---|---|
| `daemon.status` | `{}` | `{ version, mode: "loopback"\|"real", identity: {identity_id, device_id} \| null, endpoint: {endpoint_id, addr, relay_url} \| null, rooms_open: [room_id] }` |
| `identity.create` | `{}` | `{ identity_id, device_id }` — errors `identity_exists` if one exists |

`endpoint.addr` is a dialable `<endpoint_id>@<ip:port>` string when known
(loopback mode always knows it), else `null`.

### Rooms

| Method | Params | Result |
|---|---|---|
| `room.create` | `{ name }` | `{ room_id }` — name is daemon-local metadata if the protocol has no name field |
| `room.list` | `{}` | `{ rooms: [{ room_id, name, role, member_count, open }] }` |
| `room.open` | `{ room_id }` | `{ endpoint: { endpoint_id, addr }, members, timeline }` — spawns the room's node session, starts pushes; `addr` is the dialable string an inviter shares with joiners |
| `room.close` | `{ room_id }` | `{}` |
| `room.timeline` | `{ room_id, limit? }` | `{ events: [TimelineEvent] }` (chronological) |
| `room.members` | `{ room_id }` | `{ members: [{ identity_id, role, status }] }` |
| `invite.create` | `{ room_id, identity_id, role: "member"\|"agent", expiry? }` | `{ ticket }` |
| `room.join` | `{ ticket, name?, peers?: ["<endpoint_id>@<ip:port>"] }` | `{ room_id }` |

### Messages & agent status

| Method | Params | Result |
|---|---|---|
| `message.send` | `{ room_id, body }` | `{ event_id }` |
| `status.post` | `{ room_id, label, message?, progress?, artifacts? }` | `{ event_id }` — any active member may post (protocol rule) |

### Files

| Method | Params | Result |
|---|---|---|
| `file.share` | `{ room_id, path, name?, mime? }` | `{ file_id, event_id }` — imports into the blob store and authors `file.shared` |
| `file.list` | `{ room_id }` | `{ files: [{ file_id, name, size, mime, sender_id, ts, available, providers }] }` |
| `file.fetch` | `{ room_id, file_id, save_dir? }` | `{ path, bytes, verified: true }` — errors use `file_unavailable` / `file_unauthorized` / `hash_mismatch`, never a silent partial |

### Pipes

| Method | Params | Result |
|---|---|---|
| `pipe.expose` | `{ room_id, target: "127.0.0.1:3000", peer_identity }` | `{ pipe_id, event_id }` — one authorized peer (runtime rule) |
| `pipe.list` | `{ room_id }` | `{ pipes: [{ pipe_id, target, opened_by, authorized_peer, state: "open"\|"closed", connected }] }` |
| `pipe.connect` | `{ room_id, pipe_id }` | `{ local_addr }` — local forwarded address to point a browser/iframe at |
| `pipe.close` | `{ room_id, pipe_id }` | `{ event_id }` |

### Peers

| Method | Params | Result |
|---|---|---|
| `peers.status` | `{ room_id }` | `{ peers: [PeerStatus] }` |

## Pushes

| Push | Data | When |
|---|---|---|
| `room.event` | `{ room_id, event: TimelineEvent }` | a new validated event is ingested (own or remote), at most once per event — a slow/lagging session may miss pushes (bounded broadcast buffer) and they are never re-sent; consumers needing completeness must re-sync via `room.timeline` |
| `peers.changed` | `{ room_id, peers: [PeerStatus] }` | any peer connection state change |

## Honesty rules (bind the UI too)

1. Delivery is best-effort P2P: there is no "delivered" state. Files show
   `available` / provider counts; never fabricate sync confirmation.
2. Peer path (`direct`/`relay`) is shown truthfully from the runtime's
   diagnostics; real-NAT traversal (Gate A upstream) is unproven — do not
   hide relay fallback.
3. Fetch failures surface the taxonomy (`unavailable` / `unauthorized` /
   `hash_mismatch`) — a verification failure is a hard stop, not a retry.
