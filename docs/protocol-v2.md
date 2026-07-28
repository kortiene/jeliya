---
type: "Reference"
title: "Jeliya protocol v2"
description: "Normative clean-slate contract between the typed Rust core and every Jeliya client: the three-layer handshake and generation gate, the 33 approved operations, the error taxonomy, the sequenced push stream with gap detection and authoritative resync, and the conformance corpus that binds them."
tags: ["clean-slate", "conformance", "protocol", "security"]
timestamp: "2026-07-28T14:21:24Z"
status: "canonical"
implementation_status: "planned"
verification_status: "unverified"
release_status: "unreleased"
audience: ["client-authors", "contributors", "maintainers"]
---

# Jeliya protocol v2

**Status: SPECIFIED 2026-07-28. Nothing in this record is built.** Protocol v2
is the wire contract for the clean-slate client stack in the
[Dioxus clean-slate architecture](dioxus-architecture.md). It replaces
[protocol v1](PROTOCOL.md) outright: one generation at a time, no dual support,
no migration, no rollback artifact.

Read every statement below as a requirement on unwritten code. The released
`v0.6.x` line speaks v1 and keeps doing so until it is retired. This document
satisfies #161; the typed `jeliya-api` crate (#163), the codec (#164), and the
Engine cutover (#165/#166) implement it.

**v1 is inventory, not authority.** Its 24 methods were mined for the
*requirement* each serves, then re-derived. v2 retains 11, renames 10, combines
3 into fewer, and adds 9. It deliberately removes 20 shapes, listed in
[What v2 removes](#what-v2-removes). Nothing here preserves a v1 field name,
null convention, event shape, or storage shape for its own sake.

## What must be true before this can be implemented

Three v2 requirements cannot be produced by `iroh-rooms` at the pinned revision
`a5d98b70`. They are specified here as correct protocol and carried as named
upstream work, because `iroh-rooms` is Jeliya's own upstream.

| # | Upstream change | Without it |
|---|---|---|
| U1 | `ConnEvent` must carry the connection generation and a typed offline reason | A stale-generation teardown can overwrite newer presence state. #79's core criterion is unverifiable |
| U2 | A progress-observing or streaming blob-fetch API — a byte-count channel, or an incremental item consumer Jeliya drives | No `progress` or `transfer_stalled` frame is producible. A 100 MiB transfer is indistinguishable from a hang |
| U3 | A size-distinguishable fetch outcome | An over-limit fetch reports as `digest_mismatch`, which [the shared-file size policy](shared-file-size.md) calls a false accusation against an honest peer |

Until each lands, its conformance cases are declared `blocked_on_upstream` in
the manifest. **They fail, they do not skip** — see
[Conformance corpus](#conformance-corpus). An exemption that silently passes is
how a gap becomes permanent.

## Layer 0 — discovery, unauthenticated

`GET /api/health` serves `{ok, pid, port, version, protocol, min_protocol,
storage_generation, limits}`.

`storage_generation` is served here because [Layer 1](#layer-1--the-generation-gate)
requires the client to declare it on the upgrade, and absence is refusal. A
client connecting for the first time has no other way to learn it, so omitting
it from discovery would make the gate unsatisfiable rather than fail-closed.

`data_dir` is **removed** from this response. It hands an absolute filesystem
path to any unauthenticated local caller and the adoption check does not need
it. It remains in the `0600` portfile, where a caller has already proved it can
read the data dir.

The ready line, the portfile, and this endpoint carry an identical
`{protocol, min_protocol, storage_generation, limits}` object, defined **once**
in `jeliya-api`.
The portfile's separate `schema` field is removed: two version axes on one
artifact is a needless second thing to disagree.

That single definition is the point. The 100 MiB figure exists six times in the
v1 tree, including as literal copy in two locale catalogs
([shared-file size policy](shared-file-size.md)). A served limits object with
one definition is the fix, and no client may compile the value in.

### The limits object

Every field is an integer. A client MUST read them and MUST NOT assume a
compiled-in default.

| Field | Meaning |
|---|---|
| `max_shared_file_bytes` | `104_857_600`, from [the shared-file size policy](shared-file-size.md). This record owns the spelling and fixes it here |
| `max_message_body_bytes` | Largest message body accepted |
| `max_frame_bytes` | Largest single wire frame accepted before the connection is closed |
| `max_inflight_requests` | Requests one connection may have outstanding |
| `max_connections` | Connections one daemon accepts |
| `max_concurrent_transfers` | File transfers in flight across the daemon |
| `max_transfer_bytes_inflight` | Total transfer bytes the daemon will hold at once |
| `transfer_connect_allowance_ms` | Per-provider connection allowance |
| `transfer_floor_bits_per_second` | The floor the transfer deadline is computed from |
| `transfer_stall_ms` | Zero-forward-progress window before `transfer_stalled` |
| `timeline_page_max` | Largest timeline page |

The last four exist because a bounded per-file limit is not by itself a bound
on daemon memory. Fetch buffers roughly twice the file and upload holds one
full body; 64 concurrent requests against a 100 MiB limit is a multi-gigabyte
commitment. `max_concurrent_transfers` and `max_transfer_bytes_inflight` are
the bound, and exceeding either is refused with `resource_exhausted` rather
than absorbed.

## Layer 1 — the generation gate

**The gate runs on the upgrade request, before the WebSocket upgrade is
performed, before any frame is parsed, and before any dispatch.** That is the
only point provably before mutation, which is what #161 requires.

The client requests `GET /ws?v=2&sg=<storage-generation>`. Both travel in the
URL because a browser `WebSocket` constructor controls only the URL and the
subprotocol list.

Checks run in this fixed order:

1. `Host` MUST be loopback, else `forbidden_origin`.
2. `Origin`, if present, MUST be loopback, else `forbidden_origin`.
3. `v` MUST be present and MUST name a supported generation, else
   `protocol_unsupported`. **Absence is refusal, never a default.**
4. `sg` MUST be present and MUST equal the daemon's storage generation, else
   `storage_generation_mismatch`. **Absence is refusal.**
5. Credential, else `unauthenticated`, compared in constant time.

Step 3's absence rule is load-bearing: a v1 client sends no `v` at all, so a
missing generation that defaulted to current would admit every legacy client
through a gate whose entire purpose is to exclude them.

Step 4 exists because the architecture requires that "a protocol **or storage**
mismatch fails closed". Reporting the storage generation and gating only the
protocol discharges half of one sentence. A client carrying state from another
generation is refused before it can write.

### Rejections are machine-readable

A refused upgrade returns the v2 error envelope as a JSON body with a matching
status: `426` for `protocol_unsupported` and `storage_generation_mismatch`,
`401` for `unauthenticated`, `403` for `forbidden_origin`. v1's plain-text
bodies are removed; one error format, everywhere.

A rejection after the upgrade closes with a defined application close code:

| Code | Meaning |
|---|---|
| `4001` | `protocol_unsupported` |
| `4002` | `unauthenticated` |
| `4003` | `not_ready` |
| `4004` | `idle_timeout` |
| `4005` | `frame_too_large` |
| `4006` | `storage_generation_mismatch` |

A client can therefore distinguish "you speak the wrong generation" from "the
daemon died", and present the reset path instead of retrying forever.

### The credential never travels in a URL, and never in script

Native clients send `Authorization: Bearer <token>`, read only from the `0600`
portfile.

A browser cannot set headers on a WebSocket. It therefore performs a
**single-use connect ticket** exchange: `POST /api/session` returns a
short-TTL, single-use ticket which the client presents once as `?ct=`. The
daemon burns it on redemption.

**A session cookie was considered and rejected.** Cookies have no port
isolation — scope is host-only and `SameSite` computes site from the
registrable host, ignoring the port. Any other process listening on a loopback
port is same-site, so a cookie would be readable by every local origin the user
visits. "Loopback-scoped cookie" is not a thing that exists.

The ticket-issuing endpoint is a credential boundary and MUST be gated at least
as strongly as `/ws`: exact loopback `Origin` match, single use, short TTL,
burned on redemption, and rate-limited. v1's `Sec-Fetch-Site` heuristic is
removed — the code's own comment concedes a local non-browser process forges
both headers — and Host+Origin are the boundary.

The daemon token itself MUST NOT reach WebView script, a URL, a log, or a
diagnostic, in any form.

## Layer 2 — `hello`

The daemon's first frame after upgrade is exactly one `hello`:

```json
{ "t": "hello",
  "protocol": 2,
  "storage_generation": 1,
  "limits": { "...": "as above" },
  "subject": { "state": "present", "subject_id": "<64-hex>", "device_id": "<64-hex>" },
  "resume": { "state": "fresh" } }
```

`hello` carries no `pid`, no `port`, and no `data_dir`. Those are v1's
`daemon.status` transcribed, and nothing in the client seam consumes them; the
adoption check that needs pid and port is Layer 0.

`subject.state` is a tagged variant — `present` or `absent` — never a null.

## The envelope

```json
{ "id": 42, "op": "room.create", "in": { "name": "Build" } }
{ "id": 42, "ok": true,  "out": { "...": "..." } }
{ "id": 42, "ok": false, "err": { "code": "insufficient_standing", "…": "…" } }
```

- `id` correlates a reply to its request and is unique per connection while
  outstanding.
- Replies MAY arrive out of order. A client MUST correlate by `id` and MUST NOT
  assume completion order. v1's socket dispatched strictly serially, forwarding
  no pushes while a call was in flight; v2 explicitly does not.
- A push carries `t` and never `id`. That is how the two are told apart.
- Every error carries a machine-readable `code` plus operation-specific typed
  fields. **The v1 `hint` field is removed** — every default hint was
  hardcoded English, one advised ignoring its own error, and localization is
  the client's job.

### Bounded parsing

A frame exceeding `max_frame_bytes` closes the connection with `4005` without
being parsed. Nesting depth, `op` name length, and array lengths are bounded by
the codec, and exceeding any is `invalid_argument` — never a panic and never an
unbounded allocation.

**No JSON `null` carries meaning anywhere in v2.** Absence is expressed as a
tagged variant. This applies to the protocol's own frames, not only to
operation payloads: a `null` that means "unbounded" or "unknown" is exactly the
v1 compatibility-nullability this generation exists to shed.

## The 33 operations

`M` marks a mutating operation. Every mutating operation states a retry policy;
see [Idempotency](#idempotency-and-retry).

### Subject and daemon

| Operation | | Purpose |
|---|---|---|
| `subject.ensure` | M | Establish the local cryptographic subject exactly once; return its public names and no secret. Naturally idempotent — a second call returns the same subject with `created: false` |
| `daemon.stop` | M | Terminate deterministically, reply flushed before teardown |

`identity.create`'s `identity_exists` error is removed: an idempotent operation
whose own hint said "the existing identity is already usable" was reporting
success as failure.

### Rooms

| Operation | | Purpose |
|---|---|---|
| `room.create` | M | Bring a room into existence with the caller as its authority; works with no network |
| `room.list` | | What rooms are mine, in what lifecycle state, what may I do in each — from local evidence, no network |
| `room.activate` | M | Make a room live on this device. Returns reachability and capabilities, **not history** |
| `room.deactivate` | M | Stop live participation without changing membership |
| `room.leave` | M | Author a signed departure every member converges on |
| `room.timeline` | | Read committed history through an explicit cursor, identically whether or not the room is live |
| `room.members` | | The authoritative signed answer to who belongs, in what capacity and standing. Carries **no** presence or reachability |
| `room.archive` | | Open a left or removed room as a local read-only archive. Normatively zero network activity and zero durable mutation |
| `room.peers` | | Observed transport facts for one live room: which devices this daemon holds a link to, by what path, and why not when not |
| `member.remove` | M | Room authority removes a member, as a signed room fact |

`room.open` is **split**. v1 conflated "make this room live" with "give me its
history", so a client that wanted history had to start peer participation, and
a client that wanted liveness got a page of events it did not ask for.
`room.activate` and `room.timeline` are separate operations.

`member.remove` is new. `removed` is already a reachable and displayed member
state fed by other implementations' events, but v1 offers no operation that
causes it and renders its causing event as nothing. **v2 does not ship a state
a client can be in whose cause it cannot see.**

### Invitations

| Operation | | Purpose |
|---|---|---|
| `invite.mint` | M | Mint one key-bound capability exactly one named identity can redeem; return invite id, absolute expiry, role, redeemability |
| `invite.list` | | Enumerate outstanding and recently expired invites, so an authority can manage what it issued |
| `invite.revoke` | M | Withdraw an outstanding capability before expiry, as a signed fact |
| `invite.redeem` | M | Convert a capability into signed membership |

`invite.redeem` is **the only operation reachable by a non-member**, and its
authorization object is the key-bound capability itself, never an identifier.

`invite.revoke` is new: v1 had no way to withdraw a grant once minted.

### Timeline

| Operation | | Purpose |
|---|---|---|
| `message.send` | M | Author a message |
| `status.post` | M | Author an agent status |
| `status.history` | | Read status history |
| `fleet.list` | | Read the agent fleet projection |

`status.post` stays open to **any active member**. Member and agent are a
classification, not a permission — v1's three surfaces disagreed on this, and
v2 states one answer.

### Files

| Operation | | Purpose |
|---|---|---|
| `file.share` | M | Share a file, streamed. Combines v1's RPC and its separate HTTP upload edge |
| `file.list` | | Files in a room, with named provider devices and evidence-backed `fetchable` |
| `file.fetch` | M | Fetch a file's bytes |
| `file.read` | | Read a previously fetched local file, streamed |
| `transfer.cancel` | M | Cancel an in-flight transfer |

**Filesystem paths leave the protocol.** v1's `file.share` took a daemon path,
`file.fetch` took a `save_dir`, and `file.list` returned `local_path`. A
protocol that takes and returns daemon filesystem paths cannot serve a browser
or an Android `content://` consumer. `PlatformServices` owns paths; the
protocol carries bytes and identifiers.

`file.list`'s v1 `available` boolean and `providers` count are replaced by
named provider devices with reachability evidence plus explicit `fetchable` and
`self_hosted`. **Provider availability is a protocol fact, not an inference
from membership display state** (#50/#94).

`file.read` streams bytes rather than serving them over HTTP, so v1's
never-render-inline protections — which were all HTTP *response headers* — must
become **data**. The output carries the peer-declared type in an explicitly
untrusted, distinctly named field, and a client MUST NOT render peer-supplied
bytes inline on the strength of it.

### Pipes

| Operation | | Purpose |
|---|---|---|
| `pipe.publish` | M | Publish a pipe |
| `pipe.list` | | Pipes in a room, with reachability as a stated fact |
| `pipe.connect` | M | Connect to a pipe |
| `pipe.release` | M | Release a local connection |
| `pipe.revoke` | M | Withdraw a published pipe |

v1's `pipe.list.connected` boolean is split into two separately named facts: it
was named as if it were reachability while being a purely local runtime fact.
**Pipe reachability is a protocol fact** (#50/#94/#79). The hardcoded `label`
(always `"pipe"`) and `kind` (always `"tcp"`) are removed.

### Stream

| Operation | | Purpose |
|---|---|---|
| `stream.subscribe` | M | Subscribe to a room's push stream |
| `stream.unsubscribe` | M | Unsubscribe |
| `stream.resync` | M | Authoritative resync from a position |

These replace v1's single global broadcast, in which a client received whatever
the daemon chose to send for whatever rooms it had open.

## Pushes, ordering, gap detection, and resync

**Every push carries a per-room monotonic position.** v1 had no sequence number
and no cursor — only wall-clock timestamps — so a client could not tell that it
had missed anything.

- Within one room, positions are strictly increasing with no gaps in a healthy
  stream.
- Across rooms, no ordering is defined. A client MUST NOT infer one.
- A client detects a gap by observing a position discontinuity, or by receiving
  an explicit `gap` frame.

The `gap` frame states a bounded or open range as a tagged variant, never a
null:

```json
{ "t": "gap", "room": "<id>",
  "from_pos": 41,
  "to": { "state": "bounded", "pos": 57 },
  "reason": "backpressure" }
```

`stream.resync` is the **authoritative** recovery: the client names the last
position it holds, and the daemon returns either the events since, or a
`resync_required` instruction to discard and re-read from a stated position.
Resync is not best-effort and is not "call `room.activate` again", which is
what v1 clients did.

Pushes are delivered only to subscribers who are members of the room. A push
MUST NOT be a membership oracle.

### Presence

The `peer` push carries the connection generation and a typed offline reason,
so a stale-generation teardown cannot overwrite newer state. **This depends on
U1** and its cases are `blocked_on_upstream` until that lands.

## Idempotency and retry

Every mutating operation accepts an `op_id` — a client-generated unique
identifier — and the daemon keeps a dedup ledger.

**The ledger is keyed on `(authenticated session principal, op_id)`, not on the
subject.** A daemon has exactly one subject, so a per-subject ledger would be
daemon-global across the WebView, native agents, and the CLI, letting one
client's `op_id` collide with or replay another's.

The ledger survives reconnection, because the case that motivates retry is a
reply lost to a dropped connection.

| Policy | Operations |
|---|---|
| Naturally idempotent | `subject.ensure`, `room.activate`, `room.deactivate`, `invite.redeem` (re-redeeming from the same subject reports existing membership) |
| Terminal, single-effect | `daemon.stop` — the first call succeeds, a second returns `shutdown_in_progress`. This is deliberately **not** natural idempotence: once teardown is sequenced there is no state in which a caller can be told "done" truthfully, and reporting success for an operation that will not run again is the kind of comfortable lie this generation exists to remove |
| `op_id` deduplicated | `room.create`, `room.leave`, `member.remove`, `invite.mint`, `invite.revoke`, `message.send`, `status.post`, `file.share`, `file.fetch`, `pipe.publish`, `pipe.connect`, `pipe.release`, `pipe.revoke` |
| Unsafe to retry blindly | none — every mutating operation is one of the above |

A replayed `op_id` returns the **original** result and performs no second
effect. `invite.mint` in particular MUST return the original capability, never
a second grant.

`transfer.cancel` is authorized by **`(session principal, op_id)`** — the same
key as the ledger, not `(subject, op_id)` and not the connection.

The connection is wrong because a transfer whose originating connection has
dropped could then never be cancelled, which is precisely the case cancellation
exists for. The bare subject is wrong because a daemon has one subject, so any
local client could cancel any other's transfer. The principal is the only scope
that survives a reconnect without becoming daemon-global.

Cancelling an `op_id` belonging to a different principal returns
`transfer_unknown` — indistinguishable from an `op_id` that never existed, so
the operation is not an oracle for other clients' activity.

## Errors

The taxonomy is 51 codes. Every code is machine-readable, carries typed fields
rather than prose, and carries no `hint`.

Selected codes whose shape is normative:

| Code | Fields | Meaning |
|---|---|---|
| `protocol_unsupported` | `supported`, `client` as a tagged declared/absent variant | The generation gate refused |
| `storage_generation_mismatch` | `daemon`, `client` | The storage gate refused |
| `file_too_large` | `declared_bytes`, `limit_bytes`, `enforced_at` | Over-limit, naming which enforcement point fired |
| `digest_mismatch` | `expected`, `observed` | Content did not verify. **Never returned for a size refusal** |
| `resource_exhausted` | `resource`, `limit` | A served limit was reached |
| `transfer_stalled` | `transferred_bytes`, `total` as a tagged variant | No forward progress within the stall window |
| `capability_expired` | `expired_at` | Invite expired — distinct from invalid and from revoked |
| `capability_revoked` | `revoked_at` | Withdrawn before expiry |

`file_too_large.enforced_at` names one of the **five daemon-side** enforcement
points of the six in [the shared-file size policy](shared-file-size.md):
`stage_declared`, `stage_stream`, `authoring`, `fetch_preflight`,
`fetch_stream`. The sixth is the **client preflight**, which by definition
never reaches the daemon and is proven by a client-side case asserting zero
bytes are sent.

**Every operation has at least one error code specific to it.** A conformance
corpus that can only assert a shared code proves nothing about the operation it
is supposed to cover: `subject_absent` returned by `room.list` and by
`fleet.list` are the same assertion twice.

Authoring the corpus found nine operations still resting entirely on
cross-cutting codes. Each therefore gains one:

| Operation | Distinctive code | Raised when |
|---|---|---|
| `daemon.stop` | `shutdown_in_progress` | A stop is already sequenced |
| `stream.unsubscribe` | `subscription_unknown` | No such subscription on this connection |
| `room.list` | `room_index_unreadable` | The accepted-room index cannot be read |
| `room.create` | `room_name_invalid` | The name fails the stated bounds |
| `room.members` | `membership_unresolved` | The fold cannot resolve a member's standing |
| `invite.mint` | `invitee_already_member` | The named identity already holds membership |
| `invite.list` | `invite_index_unreadable` | The invite index cannot be read |
| `status.history` | `status_subject_unknown` | The named agent has no status history |
| `fleet.list` | `fleet_projection_unavailable` | The projection cannot be built |
| `file.list` | `file_index_unreadable` | The room's file index cannot be read |
| `transfer.cancel` | `transfer_unknown` | No such in-flight transfer for this principal |

These are the taxonomy's last additions; the total is 51 codes, not the 40 the
mined design projected.

### The non-oracle property

A caller that is not a member of a room MUST NOT be able to distinguish "no
such room" from "that room exists and you are not a member". Both answer
`room_not_available`, with identical fields and indistinguishable timing.

This is normative, it applies to every room-scoped operation and to the push
stream, and it is the reason `room_not_available` is one code rather than two.

## What v2 removes

Twenty v1 shapes are deliberately gone. The full list with reasons lives in the
[conformance corpus](#conformance-corpus) manifest; the ones that change client
code most:

- **`daemon.status`** — folded into the handshake. Serving `protocol`
  *after* the socket could already execute operations was a version gate that
  ran too late to gate anything.
- **`peers` dial-hint arrays** on `room.open`/`room.join` — a loopback
  workaround leaked into the public request.
- **Filesystem paths** — `file.share.path`, `file.fetch.save_dir`,
  `file.list.local_path`/`local_bytes`.
- **`room.join.name`** — one parameter was simultaneously signed into the
  member display name and written as the local room-name override.
- **The error `hint` field** — hardcoded English in a protocol.
- **The pre-identity `room.list` carve-out** — `room.list` answered `{rooms:
  []}` with no subject while `agents.fleet` answered `identity_missing`: one
  precondition, two answers.
- **Compatibility-nullability** across room, timeline, and status projections —
  nullable `role`, `status`, `last_event_kind`, `last_event_ts` exist so an
  older daemon can omit them. Under one generation there is no older daemon.
- **The fabricated default role** — v1 returned `member` for a sender the
  membership fold could not resolve. Attribution a UI uses to decide how much
  to trust something must not be invented.
- **The two-vocabulary role naming** — log `admin` versus wire `owner`. One
  name, both places.
- **The legacy device-key collision handling** — it can silently close a
  different open room as a side effect of opening one.
- **Token-in-URL and token-in-body credential shapes.**

## Conformance corpus

The corpus is hand-authored, language-neutral JSON, replayed by an independent
harness against every adapter. **It is not generated from the implementation
under test** — a corpus derived from the code under test proves only that the
code agrees with itself.

The method that made the QR encoders safe applies here: build and validate a
throwaway reference first, then transcribe. A fixture must be a transcription
of something independently known to be right.

Layout:

```
conformance/v2/
  manifest.json          required kinds per operation, and exemptions with reasons
  handshake/             gate order, absence-is-refusal, close codes, rejection bodies
  envelope/              correlation, out-of-order replies, malformed frames, bounds
  subject/  rooms/  invites/  timeline/  files/  pipes/  streams/
  invariants/            the four retained scenarios
```

Required per operation: at least one success case, and at least one error case
using **that operation's own most specific code** — a generic
`invalid_argument` does not satisfy it.

Required beyond operations: push ordering, gap detection, authoritative resync,
protocol mismatch, storage mismatch, malformed envelope, oversize frame,
authorization refusal, and the non-oracle property.

### The four retained invariants

Expressed in v2 terms and fresh state; their v1 bytes are not retained.

| Invariant | Case |
|---|---|
| #46 late join | With an authority, two non-authority authors, and a joiner: after multi-author history exists, `invite.redeem` succeeds and the joiner reads the full prior history. **Asserted as an outcome**, not as a claim about what the membership closure contains — the closure does cite content events at the pinned transport, and that is the mechanism that makes late join work |
| #47 expired reissue | After a capability expires, the authority mints a fresh one for the **same** identity and that identity redeems it |
| #147 multi-room routing | One subject keeps several rooms live; every room's pushes arrive with no cross-room loss and no position discontinuity |
| #50/#94/#79 facts | Provider availability and pipe reachability are read as stated facts; membership, presence, availability, and reachability are four distinct answers |

### Exemptions fail

`blocked_on_upstream` cases (U1, U2, U3) **fail** until the upstream change
lands. A skipped case reads as coverage; a failing case reads as work. The
manifest names every exemption with its reason and its unblocking issue.

## What this record does not decide

- The Rust type shapes — `jeliya-api` (#163) owns them.
- The codec's framing details — #164.
- Transfer budget constants. `transfer_floor_bits_per_second` and
  `transfer_stall_ms` are served, so they are tunable without a wire change;
  #198 measures and sets them. The only samples that exist span 0.1–8.6 Mbit/s
  and are not measurements of Jeliya's path.
- Whether the exact recency projection is affordable. v2 specifies exact
  max-by-signed-instant and drops v1's undisclosed 64-row scan window, because
  a window that silently hides a newer event is a correctness bug. If #198
  shows it is too costly, the window MUST be served as a limit rather than
  hidden.
- Room-authority transfer. The creator is permanently the sole authority and
  cannot leave, so a room can never be wound down. Retained for the first
  release and **recorded as a product gap**, not a design intent.
- Whether typed status severity lives on the signed event or in the daemon
  projection. [Room attention](room-attention.md) names it as the durable fix
  for classifying tone by English substring matching. It is a wire change and
  MUST be decided before the corpus freezes.
- Anything about v1. It keeps its behavior until it is retired.

## Citations

- [Protocol v1](PROTOCOL.md) — the contract this replaces, mined as inventory.
- [Dioxus clean-slate architecture](dioxus-architecture.md) — the client seam,
  the fail-closed generation rule, and the trust boundaries this implements.
- [Shared-file size policy](shared-file-size.md) — the retained maximum and the
  four obligations it places on this record.
- [Room attention](room-attention.md) — the evidence taxonomy separating
  membership, presence, availability, and reachability.
- [Agent orchestration](agent-orchestration.md) — the agent liveness and fleet
  contract the status and fleet operations serve.
- [Security and threat model](security-threat-model.md) — the non-oracle
  guard, token custody, and the boundaries the handshake enforces.
