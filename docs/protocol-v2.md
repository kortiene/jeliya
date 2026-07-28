---
type: "Reference"
title: "Jeliya protocol v2"
description: "Normative clean-slate contract between the typed Rust core and every Jeliya client: the three-layer handshake and generation gate, the 33 approved operations, the error taxonomy, the sequenced push stream with gap detection and authoritative resync, and the conformance corpus that binds them."
tags: ["clean-slate", "conformance", "protocol", "security"]
timestamp: "2026-07-28T15:26:05Z"
status: "draft"
implementation_status: "planned"
verification_status: "unverified"
release_status: "unreleased"
audience: ["client-authors", "contributors", "maintainers"]
---

# Jeliya protocol v2

**Status: DRAFT 2026-07-28. The contract is complete; its corpus is not yet
normalized to it — see [What this does not yet
specify](#what-this-does-not-yet-specify). Nothing in this record is
built.** Protocol v2
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

## What this does not yet specify

The three gaps that made this record `draft` are closed. It now states
[the shape of every request and reply](#operation-schemas), the
[complete 60-code taxonomy](#errors), and — in
[the corpus's own README](../conformance/v2/README.md) — one normative fixture
DSL. An independent adapter can implement from this document alone.

**One gap remains, and it is the corpus, not the contract.**

| Gap | Consequence while it is open |
|---|---|
| **The 335 committed fixtures are not yet normalized to the DSL** | They were authored against the specification's earlier silence, so they use 178 step verbs where the DSL defines a closed set, and 51 of them assert a field or code this record deliberately does not define. Until they are retranscribed, the corpus is not replayable and cannot be cited as evidence for any adapter |

That work is tracked as **#213**. This record stays `draft` until it lands, for
one reason: a `canonical` contract whose own bundled corpus contradicts it in
fifty-one places is a contract that overclaims. The distinction matters more
than the label — everything an implementer needs is here, and #163 and #164 are
unblocked by the content rather than by the status field.

Where this record and a fixture disagree, **this record is right and the fixture
is a bug**, unless the disagreement is named as a specification defect below.
Each class of fixture bug is named at the point the relevant rule is stated,
rather than gathered into a list that would rot separately from the rules.

Everything is settled in substance — the operation set, the handshake and its
gate, the removals, the push and resync model, the severity derivation, and the
credential rules were decided in #161 and are not reopened here. #212 added
field-level precision to that substance and resolved the twenty-seven
contradictions an earlier five-section parallel drafting attempt produced. That
attempt is why this document was authored by one hand in one vocabulary: five
authors writing five sections of one wire contract produced three incompatible
shapes for one error, the role enum closed twice with different contents, and
three vocabularies for one `truncated` field.

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

**Thirteen fields, every one an integer.** A client MUST read them and MUST NOT
assume a compiled-in default.

The count is stated because the corpus disagrees with itself about it:
`health_limits_object_carries_all_eleven_named_integer_fields` asserts an exact
key set of eleven, omitting `max_subscriptions_per_connection` and
`idle_timeout_ms` — while `close_code_4004_is_emitted_on_idle_timeout` reads
`idle_timeout_ms` out of the very object the other case says cannot contain it.
Both cases cannot be right. The record is right and both fixtures are wrong: the
two omitted fields are each load-bearing, one for `subscription_limit_reached`
and one for a client that must produce activity to stay connected.

| Field | Meaning |
|---|---|
| `max_shared_file_bytes` | `104_857_600`, from [the shared-file size policy](shared-file-size.md). This record owns the spelling and fixes it here |
| `max_message_body_bytes` | Largest message body accepted |
| `max_frame_bytes` | Largest single wire frame accepted before the connection is closed |
| `max_inflight_requests` | Requests one connection may have outstanding |
| `max_subscriptions_per_connection` | Room subscriptions one connection may hold. Exceeding it is `subscription_limit_reached`, never a silent drop |
| `max_connections` | Connections one daemon accepts |
| `max_concurrent_transfers` | File transfers in flight across the daemon |
| `max_transfer_bytes_inflight` | Total transfer bytes the daemon will hold at once |
| `transfer_connect_allowance_ms` | Per-provider connection allowance |
| `transfer_floor_bits_per_second` | The floor the transfer deadline is computed from |
| `transfer_stall_ms` | Zero-forward-progress window before `transfer_stalled` |
| `timeline_page_max` | Largest timeline page |
| `idle_timeout_ms` | Inactivity after which the daemon closes with `4004`. Served because a long-lived client cannot otherwise know how often it must produce activity to stay connected |

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
| `4007` | `malformed_frame` |

`4007` closes only when a frame's `id` cannot be recovered. A frame that decodes
far enough to correlate always gets an error reply instead — closing a
connection over one bad request would punish the other requests in flight on it.

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

**Ticket issuance MUST prove possession of the daemon token.** `POST
/api/session` requires `Authorization: Bearer <token>`, exactly like `/ws`,
and mints a short-TTL single-use ticket bound to one connection.

An earlier draft of this record gated issuance on loopback `Host` and `Origin`
alone. That is not a boundary and the record said so two paragraphs earlier
about `Sec-Fetch-Site`: a local process forges request headers freely. Under
that draft, **any process that could reach the loopback port could mint its own
ticket and pass the credential gate without ever knowing the token** — a
cross-user privilege escalation on a shared machine, since the `0600` portfile
is exactly what a different user cannot read. Header checks are anti-CSRF, not
authentication, and the two must not be confused.

This makes the browser case explicit rather than accidental: **a page cannot
authenticate itself.** Something that already holds the token has to obtain the
ticket and hand it over. In a packaged shell that is the native process, which
holds the token natively and injects a ticket into the page it controls — the
seam [the architecture](dioxus-architecture.md) already requires.

For a page served to an ordinary browser by `jeliyad` itself, no such mediator
exists, and this record does **not** invent one:

> **OPEN — browser-without-a-native-shell credential path (#113).** A browser
> tab has no way to prove possession of a token it must never see. Candidates
> are an operator-pasted one-time code, a token delivered in the launch URL by
> whatever started the browser, or declaring that surface out of scope for the
> first release. This MUST be decided by the first-release distribution
> boundary (#113) before that path ships. Until then, only mediated clients —
> native shells and the packaged WebView — have a specified credential path.

The daemon token itself MUST NOT reach WebView script, a URL, a log, or a
diagnostic, in any form.

## Layer 2 — `hello`

The daemon's first frame after upgrade is exactly one `hello`:

```json
{ "t": "hello",
  "protocol": 2,
  "storage_generation": 1,
  "limits": { "...": "as above" },
  "subject": { "state": "present", "subject_id": "<subject_id>", "device_id": "<device_id>" },
  "resume": { "state": "fresh" } }
```

`hello` carries no `pid`, no `port`, and no `data_dir`. Those are v1's
`daemon.status` transcribed, and nothing in the client seam consumes them; the
adoption check that needs pid and port is Layer 0.

`subject.state` is a tagged variant — `present` or `absent` — never a null.

## The envelope

```json
{ "id": 42, "op": "room.create", "op_id": "<op_id>", "in": { "name": "Build" } }
{ "id": 42, "ok": true,  "out": { "...": "..." } }
{ "id": 42, "ok": false, "err": { "code": "insufficient_standing", "…": "…" } }
```

- `id` correlates a reply to its request and is unique per connection while
  outstanding.
- `op_id` is the **only** envelope field that is optional, and it deduplicates
  the request rather than parameterising the operation. See
  [`op_id` is an envelope field](#request-deduplication-lives-in-the-envelope).
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

## Wire conventions

Every schema below is written in this vocabulary. It is stated once, here, so
that no operation section may define a shape a second way.

### Three discriminants, each with one domain

| Key | Domain |
|---|---|
| `t` | the type of a push frame |
| `kind` | the type of a committed event |
| `state` | a **condition variant** — what the situation is with respect to one thing |

Nothing else discriminates. `type`, `variant`, and `status` are never
discriminant keys, and a discriminant is never spelled two ways.

**Enum or variant, never both.** A closed set whose arms all carry no payload is
a **bare string enum**. A closed set where any arm carries a payload is a
**tagged variant**: an object whose `state` names the arm, with that arm's
payload as siblings. A variant never carries an arm-independent sibling — a
field that is meaningful in one arm and meaningless in another belongs inside
the arm, which is what makes the meaningless case unrepresentable rather than
merely discouraged.

### Request deduplication lives in the envelope

```json
{ "id": 42, "op": "message.send", "op_id": "<op_id>",
  "in": { "room_id": "<room_id>", "body": "…" } }
```

`op_id` deduplicates a **request**; it is not an argument to an operation.
Keeping it out of `in` is what lets the next rule — no optional request fields —
hold without a single exception.

Because it sits in the envelope, **`op_id` is accepted on every operation and
ignored by those that do not deduplicate** — including all three `stream.*`
operations. It is never `unrecognised_field`.

`transfer.cancel` is the one operation that needs to *name* an `op_id` rather
than carry one. Its request field is therefore `transfer_op_id`, and the
envelope `op_id` on a `transfer.cancel` is ignored like any other
naturally-idempotent operation. One wire name never means two things.

### There are no optional request fields

Every field of every `in` object is required. A missing key is
`invalid_argument` with `reason: {"state": "missing"}`, and an undefined key is
`invalid_argument` with `reason: {"state": "unrecognised_field"}`.

This is why `cursor`, `direction`, and `limit` are required on every paging
operation. `{"state": "start"}` and `"forward"` are the caller's explicit
choice, not a daemon default — a default is a second place the contract can
disagree with itself, and a client that did not choose a page size cannot
reason about the page it got.

**The six paging operations are `room.timeline`, `room.archive`,
`status.history`, `invite.list`, `file.list`, and `pipe.list`.** All six take the
same three fields with the same meanings and bounds: `cursor` is
[`<cursor>`](#shared-value-types), `direction` is a bare enum of `forward` and
`backward`, and `limit` is an integer in `1..=timeline_page_max`. A `limit`
outside that range is `invalid_argument` with `reason: {"state": "bound"}` —
**refused, never silently clamped**, and never `resource_exhausted`.

Stating the bound once, here, is deliberate. An earlier draft stated it under
two operations and left the other four to inherit it by implication, which is
the same drift as stating a rule and then writing schemas that violate it.
`timeline_page_max` governs all six despite its name, because one served page
bound is easier to reason about than six.

### Validation order

Every operation validates in this order, and the order is normative because two
of its steps are security properties rather than conveniences.

1. **Structural decode** — does the frame decode into this operation's request
   type, with every required key present, correctly typed, correctly formatted,
   and within bounds? → `invalid_argument`
2. **Subject precondition** → `subject_absent`
3. **Dedup ledger** — a replayed `op_id` returns the original result, and no
   later step runs
4. **Room index** → `room_not_available`
5. **Standing** → `membership_ended`, unless the operation is defined over a
   former membership. Exactly two are: `room.archive` and `room.list`
6. **Role** → `insufficient_standing`
7. **Operation semantics** → the operation's own codes

The step 5 exception is not a convenience. `room.archive` exists *to* open a
room the caller has left, so a pipeline that refused it on standing would make
the operation unreachable in every state it is defined for, and `room.list`
must enumerate left rooms or a client could never find one to archive. Stating
the exception in the pipeline rather than only in the operation's own section is
the difference between a rule with a carve-out and a rule that is quietly false.

Structure comes first because no rule can be applied to a value that has not
been decoded. Putting format and bounds in step 1 does **not** weaken
[the non-oracle property](#the-non-oracle-property): step 1 discloses only the
value formats this record publishes, never any daemon state. The property that
must hold is that steps 4 and later are indistinguishable to a non-member, and
they are — a caller who is not a member cannot reach step 5 at all.

### Shared value types

Every operation draws from these. A type defined here is never redefined in an
operation section.

**Notation.** `<name>` in any schema below means "a value of the type `name`",
whether that type is a scalar domain or a composite. The three row types —
`<room_row>`, `<file_row>`, and `<event>` — are defined by the JSON block that
immediately follows their first use rather than in this table, because each
belongs to exactly one operation.

| Type | Form |
|---|---|
| `<room_id>` `<subject_id>` `<device_id>` `<event_id>` `<invite_id>` `<file_id>` `<pipe_id>` `<op_id>` | opaque strings, each a distinct domain |
| `<ts>` | RFC 3339 UTC instant with a `Z` offset |
| `<uint>` | JSON number, integral, `>= 0` |
| `<bool>` | JSON `true` or `false` |
| `<string>` | JSON string, bounded by the codec |
| `role` | bare enum: `authority`, `member` |
| `standing` | bare enum: `active`, `left`, `removed` |
| `severity` | bare enum: `ok`, `failed`, `review` — **derived, never sent** |
| `liveness` | bare enum: `online-idle`, `working`, `offline`, `stale` — **derived** |
| `reachability` | bare enum: `connecting`, `connected`, `alone`, `offline` — **one room** |
| `link` | variant: `direct {since}`, `relay {since}`, `not_connected {reason}` — **one device** |
| `link.reason` | bare enum: `never_dialed`, `dial_failed`, `no_route`, `closed` |
| `cursor` | variant: `start`, `at {pos}` |
| `truncated` | variant: `complete`, `more {cursor}` |
| `progress` | variant: `absent`, `reported {percent}` where `percent` is `0..=100` |
| `author` | variant: `resolved {subject_id, role, standing}`, `unresolved` |
| `subject` | variant: `present {subject_id, device_id}`, `absent` — `hello` only |
| `resume` | variant: `fresh`, `resumed {from_pos}` — `hello` only |
| `gap.to` | variant: `bounded {pos}`, `open` |
| `gap.reason` | bare enum: `backpressure`, `retention`, `subscription_lapse` |
| `target` | `{ host, port }` — one object, never two sibling fields |
| `audience` | variant: `room`, `subjects {subject_ids}` |
| `redeemability` | bare enum: `outstanding`, `expired`, `revoked`, `redeemed` |
| `last_event` | variant: `present {at, kind}`, `absent` |
| `last_seen` | variant: `present {at}`, `absent` |
| `latest_status` | variant: `present {label, at}`, `absent` |
| `byte_total` | variant: `known {bytes}`, `unknown` |
| `outcome` | variant: `cancelled`, `already_cancelled` — `transfer.cancel` only |

Every variant in this record appears in this table. A variant whose arms are not
enumerated here does not exist — an arm set stated only by example is how an
adapter author guesses, and two adapters guess differently.

`role` is closed on exactly two tokens. **`agent` is not a role** — this record
already states that member and agent are a classification, not a permission, and
agent-ness is derived: a member that has authored at least one `status.post`
event is an agent, which is what `fleet.list` projects. `owner` is v1's spelling
and is removed.

**`standing` is the only vocabulary for a subject's relationship to a room.** It
is what `room.list` reports about the caller, what `room.members` reports about
each member, and what `author.standing` carries on a committed event. There is
no separate room `lifecycle`: this record settles that a room can never be wound
down, so no room state exists that is not somebody's standing. `live` remains a
distinct `<bool>`, because liveness is a fact about this device and standing is a
fact about the room.

**`reachability` and `link` are two types, not one.** `reachability` answers for
a whole room; `link` answers for one device, and every per-device answer uses it
— `room.peers`, the provider rows of `file.list`, and `pipe.list`. A single
per-device type is what keeps `reason` one closed set instead of four.

### The committed event

```json
{ "pos": 58, "event_id": "<event_id>", "at": "<ts>",
  "kind": "message", "content": { "body": "…" },
  "author": { "state": "resolved", "subject_id": "<subject_id>",
              "role": "authority", "standing": "active" } }
```

`author` is a variant because this record removes **the fabricated default
role** — v1 returned `member` for a sender the membership fold could not
resolve. A flat `sender_role` cannot express an unresolvable author without a
null or an invention, and both are forbidden, so the unresolvable case gets an
arm of its own and carries no attribution at all.

**`kind` is closed at ten, and each arm fixes its `content`.** An event whose
`kind` a client does not recognise is not rendered and not counted; it is never
guessed at.

| `kind` | `content` | Authored by |
|---|---|---|
| `room_created` | `{ name }` | `room.create` |
| `message` | `{ body }` | `message.send` |
| `agent_status` | `{ label, progress }` | `status.post` |
| `member_joined` | `{ subject_id, role }` | `invite.redeem` |
| `member_left` | `{ subject_id }` | `room.leave` |
| `member_removed` | `{ subject_id, by }` | `member.remove` |
| `invite_revoked` | `{ invite_id }` | `invite.revoke` |
| `file_shared` | `{ file_id, name, bytes, digest }` | `file.share` |
| `pipe_published` | `{ pipe_id, target, audience }` | `pipe.publish` |
| `pipe_revoked` | `{ pipe_id }` | `pipe.revoke` |

Every operation that returns an `event_id` appears in the right-hand column, and
every kind is authored by exactly one operation. `agent_status.content` carries
**no `severity`**: severity is derived and served on the projection, never
written into signed content, which is what makes a new label a value change
rather than an `iroh-rooms` schema change.

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
| `room.list` | | What rooms are mine, in what standing, what may I do in each — from local evidence, no network |
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

#### Payload contracts

A one-line description is not a contract. These three operations are specified
here because authoring the corpus proved they could not otherwise be tested —
and an operation that cannot be conformance-tested cannot be implemented
either.

[Agent orchestration](agent-orchestration.md) remains the normative source for
**derived liveness**, whose four values are `online-idle`, `working`,
`offline`, and `stale`. Liveness is derived at read time from peer connection
state and the agent's most recent event; it is **never stored** and never
computed client-side. v2 carries that contract unchanged.

#### The label vocabulary is closed, and severity is derived from it

In v1 the label is **free-form, up to 64 bytes**. That is why
[room attention](room-attention.md) records an "untyped-label residual":
attention has to classify tone by matching wire words, an allowlist that "can
miss a real failure phrased in an unlisted way and can misfire on a lookalike".
That record names a typed severity on the agent-status event as the durable
fix.

**v2 fixes it a different way: by closing the vocabulary.** Once the set of
labels is fixed, severity is a lookup rather than an inference, and the failure
mode disappears at its root.

| Label | Severity | Meaning |
|---|---|---|
| `online` | `ok` | Announced, not executing |
| `idle` | `ok` | Not executing, ready |
| `claiming` | `ok` | In claim arbitration. Deliberately not `working` — a claim is not execution |
| `working` | `ok` | Executing |
| `done` | `ok` | Task succeeded |
| `failed` | `failed` | Task failed |
| `blocked` | `review` | Stopped, and needs a person: a decision, a credential, an approval, or an ambiguous instruction |

Any other label is `status_label_unknown`. **v2 does not silently reclassify an
unrecognised label**, which is what v1 does by treating unknown labels as
idle-class — a typo'd label reading as a truthful agent state.

`blocked` is new. The attention model needs four reasons — `failed`, `stale`,
`offline`, and `review` — and the first three already fall out of the
vocabulary and derived liveness. `review` had no label, so agents had nothing
truthful to post when stopped and waiting on a human.

**There is no severity field anywhere**, on the signed event or in a payload.
Severity is served as a derived value, and a client MUST NOT re-derive it. Two
reasons this beats a signed severity field:

- a new permitted label *value* costs nothing on the wire, whereas a new
  *field* in signed event content is an `iroh-rooms` schema change every peer
  must validate — a fourth upstream dependency for a problem a closed
  vocabulary already solves;
- an agent choosing from a fixed set cannot assert a severity that contradicts
  its own label. A self-asserted severity field can, and a buggy or hostile
  agent would be believed.

What v2 changes about the payloads themselves:

**`status.post`** takes a `label` from the closed vocabulary and a `progress`
that is a tagged variant:

```json
{ "label": "working", "progress": { "state": "reported", "percent": 40 } }
{ "label": "claiming", "progress": { "state": "absent" } }
```

v1 carried `progress: null`, which v2's no-null rule forbids. `percent` is an
integer in `0..=100` inclusive; anything else is `invalid_argument`.

**`status.history`** returns `entries`, one per real posted event, in
chronological order:

```json
{ "entries": [ { "at": "<ts>", "label": "working",
                 "severity": "ok",
                 "progress": { "state": "absent" } } ],
  "truncated": { "state": "complete" } }
```

`severity` is the derived value from the table above, served so no client
re-derives it.

The daemon MUST NOT interpolate, smooth, or fabricate a point. A chart drawn
from this plots actual events or it plots a lie. Paging obeys
`timeline_page_max`, and truncation is always stated as a tagged variant rather
than inferred from a full page.

**`fleet.list`** returns the agents with their derived liveness, and **no
tallies**:

```json
{ "agents": [ { "subject_id": "<subject_id>", "room_id": "<room_id>",
                "liveness": "working",
                "latest_status": { "state": "present", "label": "working", "at": "<ts>" },
                "last_seen": { "state": "present", "at": "<ts>" } } ] }
```

v1 served `active`, `working`, `total`, `rooms_total`, and `rooms_covered`
alongside the list. Those are derivable from the list itself, and a served
tally is a second thing that can disagree with the facts it summarizes. **v2
serves the facts, not the tallies.**

Scope is the caller's authorized room set. `fleet.list` MUST NOT be a
membership oracle: a room the caller cannot see contributes nothing, and its
absence is indistinguishable from it not existing.

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

#### The publish-target policy

`pipe.publish` names a local TCP target. The target MUST be loopback — an IPv4
address in `127.0.0.0/8` or IPv6 `::1` — and the port MUST be in `1..=65535`.
Anything else is `pipe_target_refused`, carrying the rejected target — **not**
the generic `policy_refused`, so a client can tell "your target is not allowed"
from "you may not publish in this room". A user responds to those differently.

This is stated normatively because the boundary is not obvious and getting it
wrong is severe: a pipe is a peer-reachable tunnel to whatever it names, so a
non-loopback target turns a room member into a proxy onto the publisher's LAN.
A private-range address such as `192.168.1.10` is **refused**, not accepted —
being unroutable from the internet is not the same as being local, and the
policy is loopback, not "probably safe".

The refusal is a local policy decision made before publication, so it costs no
network round trip and reveals nothing to peers.

### Stream

| Operation | | Purpose |
|---|---|---|
| `stream.subscribe` | M | Subscribe to a room's push stream. **Naturally idempotent** — subscribing twice does not duplicate delivery and is not an error |
| `stream.unsubscribe` | M | Unsubscribe |
| `stream.resync` | M | Authoritative resync from a position |

These replace v1's single global broadcast, in which a client received whatever
the daemon chose to send for whatever rooms it had open.

## Operation schemas

One request and one reply shape per operation, in the vocabulary
[Wire conventions](#wire-conventions) fixes. Types named there are not redefined
here.

Reading the tables: **every `in` field listed is required** — there are no
optional request fields, so the column is omitted rather than filled with "yes"
thirty-three times. `op_id` never appears in an `in` table because it is an
envelope field. The **Errors** row names only the codes specific to that
operation; every operation can additionally answer any cross-cutting code its
[validation order](#validation-order) step reaches.

### `subject.ensure`

| | |
|---|---|
| `in` | `{}` |
| `out` | `{ "subject_id": "<subject_id>", "device_id": "<device_id>", "created": "<bool>" }` |
| Errors | `subject_store_unwritable` |

Naturally idempotent. A second call returns the same subject with
`created: false`, and that is a success — this record removes v1's
`identity_exists` precisely because reporting it as a failure was a lie. No
secret is returned in any form.

### `daemon.stop`

| | |
|---|---|
| `in` | `{}` |
| `out` | `{ "stopping": true }` |
| Errors | `shutdown_in_progress` |

Terminal and single-effect. The reply is flushed before teardown begins; a
second call answers `shutdown_in_progress` rather than a comfortable `true`.

### `room.create`

| | |
|---|---|
| `in` | `{ "name": "<string>" }` |
| `out` | `{ "room_id": "<room_id>", "name": "<string>", "role": "authority", "standing": "active", "event_id": "<event_id>", "pos": "<uint>", "created_at": "<ts>" }` |
| Errors | `room_name_invalid` |

Works with no network. The caller is the room's authority, so `role` is
constant — it is served rather than assumed because a client that infers it is
a client that will get it wrong for a room it did not create.

`name` is `1..=128` bytes after trimming surrounding whitespace, and must
contain at least one non-whitespace character. Outside that it is
`room_name_invalid`, whose `reason` is the same closed variant
`invalid_argument` uses — a `bound` arm for length, a `format` arm for a name
that is only whitespace. The bounds are stated here because a code defined
against unstated bounds is a code no two implementations agree on.

It returns `event_id` and `pos` like every other event-authoring operation.
Room creation is a committed `room_created` event, and `pos` is what anchors the
room's position space at its origin — a client that could not learn it would
have to guess where the timeline starts.

### `room.list`

| | |
|---|---|
| `in` | `{}` |
| `out` | `{ "rooms": [ <room_row> ] }` |
| Errors | `room_index_unreadable` |

```json
{ "room_id": "<room_id>",
  "name": "<string>",
  "standing": "active",
  "live": false,
  "role": "member",
  "member_count": 3,
  "last_event": { "state": "present", "at": "<ts>", "kind": "message" },
  "capabilities": ["room.timeline", "message.send"] }
```

Answered from local evidence with **zero network activity**, whether or not any
room is live.

`last_event` is a variant because a room with no events must be able to say so.
A flat `last_event_ts` could express that only with a null or a sentinel, and
both are forbidden — which is why this record removes v1's nullable
`last_event_ts` and `last_event_kind` rather than renaming them. One variant
un-nulls both and makes a kind without an instant unrepresentable.

`standing` is the caller's own. It is not called `lifecycle`: that was the same
value under a second name, and there is no room state that is not somebody's
standing.

`capabilities` holds **operation-name tokens** — a capability token *is* the
name of the operation it authorises, so the mapping is total and no second
vocabulary exists. A token is present **iff** the operation would not be refused
on membership, standing, lifecycle, **or liveness** grounds at the instant the
reply was composed. Liveness is in that list deliberately: an advertised
capability that is refused right now for want of a transport is exactly the
drift this array exists to prevent. The three operations that require liveness
are `file.fetch`, `pipe.connect`, and `room.peers`; a non-live room's array
omits them.

### `room.activate`

| | |
|---|---|
| `in` | `{ "room_id": "<room_id>" }` |
| `out` | `{ "room_id": "<room_id>", "live": true, "reachability": "connecting", "capabilities": ["…"] }` |
| Errors | `transport_unavailable` |

Returns reachability and capabilities, **not history**. Naturally idempotent.

### `room.deactivate`

| | |
|---|---|
| `in` | `{ "room_id": "<room_id>" }` |
| `out` | `{ "room_id": "<room_id>", "live": false }` |
| Errors | none — [exempt](#one-distinctive-code-per-operation) |

Stops live participation without changing membership. Naturally idempotent.

### `room.leave`

| | |
|---|---|
| `in` | `{ "room_id": "<room_id>" }` |
| `out` | `{ "room_id": "<room_id>", "event_id": "<event_id>", "pos": "<uint>", "standing": "left" }` |
| Errors | `sole_authority_cannot_leave` |

Authors a signed departure every member converges on, so it returns the event it
wrote rather than a bare acknowledgement.

### `room.timeline`

| | |
|---|---|
| `in` | `{ "room_id": "<room_id>", "cursor": <cursor>, "direction": "forward", "limit": "<uint>" }` |
| `out` | `{ "room_id": "<room_id>", "events": [ <event> ], "truncated": <truncated> }` |
| Errors | `cursor_unknown` |

`cursor`, `direction`, and `limit` behave as
[stated for all six paging operations](#there-are-no-optional-request-fields).

**Continuation is `truncated`, and only `truncated`.** When the reply is
`{"state": "more", "cursor": …}` the caller resends that cursor; when it is
`{"state": "complete"}` there is nothing further and no cursor exists to
misuse.

An earlier draft returned a sibling `next_pos` alongside `truncated`, and the
two disagreed: the worked example paired a single event at position 58 with
`next_pos: 59`, which under an exclusive cursor skips exactly one event at every
page boundary. The off-by-one was the symptom; **two continuation mechanisms in
one reply was the defect**, because a client may follow either and they need not
agree. `stream.resync` keeps an explicit position because recovery is defined
over positions rather than pages, and it is the only place one appears.

Reads committed history identically whether or not the room is live. A caller
whose standing is `left` or `removed` gets `membership_ended` and uses
`room.archive`.

### `room.members`

| | |
|---|---|
| `in` | `{ "room_id": "<room_id>" }` |
| `out` | `{ "room_id": "<room_id>", "members": [ { "subject_id": "<subject_id>", "role": "member", "standing": "active", "joined_at": "<ts>" } ] }` |
| Errors | `membership_unresolved`, `member_unknown` |

The authoritative signed answer to who belongs, in what capacity and standing.
Carries **no** presence and **no** reachability — those are `room.peers`, and
conflating them is the confusion (#50/#94) this generation exists to end.

### `room.archive`

| | |
|---|---|
| `in` | `{ "room_id": "<room_id>", "cursor": <cursor>, "direction": "forward", "limit": "<uint>" }` |
| `out` | `{ "room_id": "<room_id>", "standing": "left", "events": [ <event> ], "truncated": <truncated> }` |
| Errors | `room_still_active` |

Normatively **zero network activity and zero durable mutation**. Defined only
over a room the caller has left or been removed from; on an active room it
answers `room_still_active`.

### `room.peers`

| | |
|---|---|
| `in` | `{ "room_id": "<room_id>" }` |
| `out` | `{ "room_id": "<room_id>", "reachability": "connected", "peers": [ { "subject_id": "<subject_id>", "device_id": "<device_id>", "link": <link> } ] }` |
| Errors | `room_not_live` |

Observed transport facts for one live room: which devices this daemon holds a
link to, by what path, and why not when not. Requires liveness, so a non-live
room answers `room_not_live` and `room.peers` is absent from that room's
`capabilities`.

### `member.remove`

| | |
|---|---|
| `in` | `{ "room_id": "<room_id>", "subject_id": "<subject_id>" }` |
| `out` | `{ "room_id": "<room_id>", "subject_id": "<subject_id>", "event_id": "<event_id>", "pos": "<uint>", "standing": "removed" }` |
| Errors | `authority_cannot_be_removed`, `member_unknown` |

Requires `role: "authority"`; a member calling it gets
`insufficient_standing { required: "authority", held: "member" }`.

**Re-removing an already-removed member succeeds**, returning the original
removal's `event_id`, `pos`, and `standing`, and authoring no second event. See
[Idempotency](#idempotency-and-retry) for why this and `invite.revoke` answer
alike.

### `invite.mint`

| | |
|---|---|
| `in` | `{ "room_id": "<room_id>", "subject_id": "<subject_id>", "role": "member", "expires_at": "<ts>" }` |
| `out` | `{ "invite_id": "<invite_id>", "room_id": "<room_id>", "subject_id": "<subject_id>", "role": "member", "expires_at": "<ts>", "capability": "<string>", "redeemability": "outstanding" }` |
| Errors | `invitee_already_member`, `role_not_grantable` |

Mints one **key-bound** capability exactly one named identity can redeem;
`subject_id` is that identity and is required.

`role` accepts **`member` only** today. `authority` answers
`role_not_grantable`, because this record settles that the creator is
permanently the sole authority. The field is kept rather than dropped because
that limitation is recorded as a *product gap, not a design intent* — a required
field with one legal value makes widening it a non-breaking change.

A replayed `op_id` MUST return **the original capability**, never a second
grant.

### `invite.list`

| | |
|---|---|
| `in` | `{ "room_id": "<room_id>", "cursor": <cursor>, "direction": "forward", "limit": "<uint>" }` |
| `out` | `{ "room_id": "<room_id>", "invites": [ { "invite_id": "<invite_id>", "subject_id": "<subject_id>", "role": "member", "expires_at": "<ts>", "redeemability": "outstanding" } ], "truncated": <truncated> }` |
| Errors | `invite_index_unreadable` |

`redeemability` is the **same** bare enum `invite.mint` returns:
`outstanding`, `expired`, `revoked`, `redeemed`. One vocabulary answers "can
this be redeemed, and if not why not" in both places, which a boolean cannot —
a `redeemable: false` that will not say whether the capability expired, was
withdrawn, or was already used forces the client to guess between three
different things to tell the user.

It is **not** called `state`. `state` is the discriminant *inside* a tagged
variant, and using it as an ordinary field name is how a reader stops being able
to tell a variant from a record at a glance.

The capability itself is **never** returned by `invite.list` — only
`invite.mint` ever serves it, and only once.

### `invite.revoke`

| | |
|---|---|
| `in` | `{ "room_id": "<room_id>", "invite_id": "<invite_id>" }` |
| `out` | `{ "invite_id": "<invite_id>", "event_id": "<event_id>", "pos": "<uint>", "revoked_at": "<ts>" }` |
| Errors | `capability_redeemed`, `invite_unknown` |

**Re-revoking an already-revoked capability succeeds**, returning the original
withdrawal, exactly as `member.remove` does. It answers `capability_redeemed`
only when the capability was already converted into membership, which is a
different fact and not a terminal one this operation authored.

### `invite.redeem`

| | |
|---|---|
| `in` | `{ "capability": "<string>" }` |
| `out` | `{ "room_id": "<room_id>", "subject_id": "<subject_id>", "role": "member", "standing": "active", "event_id": "<event_id>", "pos": "<uint>", "joined": "<bool>" }` |
| Errors | `capability_invalid`, `capability_expired`, `capability_revoked`, `capability_redeemed` |

**The only operation reachable by a non-member.** Its authorization object is
the key-bound capability itself, never an identifier — so `in` carries no
`room_id` and no `invite_id`, both of which would be identifiers a non-member
could probe with.

Naturally idempotent: re-redeeming from the same subject reports existing
membership, and **`joined` is how it reports it** — `true` when this call
authored the membership, `false` when it already existed. Without that field the
two outcomes are byte-identical, so a client could not tell a fresh join from a
replay, which is the same defect this record removes from `subject.ensure` by
serving `created`.

### `message.send`

| | |
|---|---|
| `in` | `{ "room_id": "<room_id>", "body": "<string>" }` |
| `out` | `{ "room_id": "<room_id>", "event_id": "<event_id>", "pos": "<uint>", "at": "<ts>" }` |
| Errors | `message_too_large` |

The returned `pos` is in the **same position space** as the room's push stream,
which is what makes a client able to tell that the push it just received is its
own write.

### `status.post`

| | |
|---|---|
| `in` | `{ "room_id": "<room_id>", "label": "working", "progress": <progress> }` |
| `out` | `{ "room_id": "<room_id>", "event_id": "<event_id>", "pos": "<uint>", "at": "<ts>", "severity": "ok" }` |
| Errors | `status_label_unknown` |

`label` is drawn from [the closed vocabulary](#the-label-vocabulary-is-closed-and-severity-is-derived-from-it).
`severity` is served on the reply as the derived value; it is **never sent**, and
a client MUST NOT re-derive it.

**There is no free-text field.** A `message` key — or any other undefined key —
is `invalid_argument` with `field: "in.message"` and
`reason: {"state": "unrecognised_field"}`. Open to any active member: member and
agent are a classification, not a permission.

### `status.history`

| | |
|---|---|
| `in` | `{ "room_id": "<room_id>", "subject_id": "<subject_id>", "cursor": <cursor>, "direction": "forward", "limit": "<uint>" }` |
| `out` | `{ "room_id": "<room_id>", "subject_id": "<subject_id>", "entries": [ { "at": "<ts>", "label": "working", "severity": "ok", "progress": <progress> } ], "truncated": <truncated> }` |
| Errors | `status_subject_unknown` |

**Entries carry no `pos` and no `event_id`.** This is a projection read;
positions are read from `room.timeline`, which is the one position space.

One entry per real posted event, in chronological order. The daemon MUST NOT
interpolate, smooth, or fabricate a point — a chart drawn from this plots actual
events or it plots a lie.

### `fleet.list`

| | |
|---|---|
| `in` | `{}` |
| `out` | `{ "agents": [ { "subject_id": "<subject_id>", "room_id": "<room_id>", "liveness": "working", "latest_status": { "state": "present", "label": "working", "at": "<ts>" }, "last_seen": { "state": "present", "at": "<ts>" } } ] }` |
| Errors | `fleet_projection_unavailable` |

**No tallies.** v1 served `active`, `working`, `total`, `rooms_total`, and
`rooms_covered` alongside the list; all are derivable from it, and a served
tally is a second thing that can disagree with the facts it summarises.

An **agent** is a member that has authored at least one `status.post` event.
Agent-ness is derived here, not declared: it is a classification, not a
permission, so it is not a `role` and appears in no membership row.

Scope is the caller's authorized room set. A room the caller cannot see
contributes nothing, and its absence is indistinguishable from it not existing.

### `file.share`

| | |
|---|---|
| `in` | `{ "room_id": "<room_id>", "name": "<string>", "declared_bytes": "<uint>", "declared_content_type": "<string>" }` |
| `out` | `{ "room_id": "<room_id>", "file_id": "<file_id>", "event_id": "<event_id>", "pos": "<uint>", "bytes": "<uint>", "digest": "<string>" }` |
| Errors | `declared_size_mismatch`, `file_too_large` |

Bytes are **streamed**, combining v1's RPC and its separate HTTP upload edge.
**No filesystem path appears in the request**: v1's `path` is removed, because a
protocol that takes a daemon path cannot serve a browser or an Android
`content://` consumer. `PlatformServices` owns paths.

`declared_bytes` is checked against `max_shared_file_bytes` before any byte is
accepted (`enforced_at: "stage_declared"`) and against the streamed total after
(`enforced_at: "stage_stream"`). A stream that does not match its declaration is
`declared_size_mismatch`, never `digest_mismatch` — accusing an honest peer of
corruption for a size disagreement is the false accusation
[the size policy](shared-file-size.md) forbids.

The field is `declared_content_type` on **every** operation that carries it —
`file.share`, `file.list`, and `file.read` alike. It is peer-declared and
untrusted at each of them, and a value that is untrusted in one place and named
`content_type` in another is a value a client will eventually trust by accident.
See `file.read`.

### `file.list`

| | |
|---|---|
| `in` | `{ "room_id": "<room_id>", "cursor": <cursor>, "direction": "forward", "limit": "<uint>" }` |
| `out` | `{ "room_id": "<room_id>", "files": [ <file_row> ], "truncated": <truncated> }` |
| Errors | `file_index_unreadable` |

```json
{ "file_id": "<file_id>", "name": "<string>", "bytes": 4096,
  "digest": "<string>",
  "declared_content_type": "<string>",
  "shared_by": "<subject_id>", "shared_at": "<ts>",
  "providers": [ { "subject_id": "<subject_id>", "device_id": "<device_id>", "link": { "state": "direct", "since": "<ts>" } } ],
  "fetchable": true,
  "self_hosted": false }
```

v1's `available` boolean and `providers` **count** become named provider devices
carrying evidence: **provider availability is a protocol fact, not an inference
from membership display state** (#50/#94). `local_path` and `local_bytes` are
removed with the rest of the filesystem paths; whether bytes are held locally is
`self_hosted`.

Each provider's `link` is the same per-device type `room.peers` uses. There is
no separate per-provider reachability vocabulary.

`digest` is served here, not only by `file.share`, because a client that fetches
a file it did not share has no other way to learn the digest it must verify
against — and `digest_mismatch` is meaningless to a client that never held an
expected value.

### `file.fetch`

| | |
|---|---|
| `in` | `{ "room_id": "<room_id>", "file_id": "<file_id>" }` |
| `out` | `{ "room_id": "<room_id>", "file_id": "<file_id>", "bytes": "<uint>", "digest": "<string>", "provider": { "subject_id": "<subject_id>", "device_id": "<device_id>" } }` |
| Errors | `provider_unreachable`, `file_unknown`, `file_too_large`, `digest_mismatch`, `transfer_stalled`, `room_not_live` |

**No `save_dir`.** v1's destination path is removed; the daemon holds the bytes
and `file.read` streams them out.

The reply does **not** report local hold state. Whether bytes are held is
answered by `file.read` succeeding or by a `file.list` row's `self_hosted` — one
fact, one place. Requires liveness.

### `file.read`

| | |
|---|---|
| `in` | `{ "room_id": "<room_id>", "file_id": "<file_id>" }` |
| `out` | `{ "file_id": "<file_id>", "bytes": "<uint>", "declared_content_type": "<string>" }`, then the bytes streamed |
| Errors | `file_not_fetched`, `file_unknown` |

Streams bytes rather than serving them over HTTP. v1's never-render-inline
protections were all HTTP *response headers*, and headers do not exist here, so
they become **data**: the peer-declared type is carried in an explicitly
untrusted, distinctly named field — `declared_content_type`, never
`content_type` — and **a client MUST NOT render peer-supplied bytes inline on
the strength of it.**

### `transfer.cancel`

| | |
|---|---|
| `in` | `{ "transfer_op_id": "<op_id>" }` |
| `out` | `{ "transfer_op_id": "<op_id>", "outcome": { "state": "cancelled" }, "transferred_bytes": "<uint>", "total": { "state": "known", "bytes": "<uint>" } }` |
| Errors | `transfer_unknown` |

**The request field is `transfer_op_id`, not `op_id`.** It names the transfer
being cancelled — which the caller knows because it chose it — so a cancel
survives the reconnect that would make a cancel-request identifier meaningless.
Giving it a distinct name is what keeps one wire spelling from meaning two
things; the envelope `op_id` on this operation is ignored like any other
naturally-idempotent operation.

`outcome` is a variant closed on `cancelled` and `already_cancelled`, not a
`cancelled: true` boolean. Cancel is naturally idempotent, and an operation
whose idempotence a caller cannot observe reports a replay as a fresh effect —
the same reason `subject.ensure` serves `created` and `invite.redeem` serves
`joined`.

`total` is `<byte_total>`, the same variant `transfer_stalled` carries. A cancel
that reports "4 MiB transferred" without a total reports a number the user
cannot interpret, and the `unknown` arm exists because a provider that never
declared a size genuinely leaves it unknown.

Authorized by `(session principal, transfer_op_id)`. Cancelling a transfer
belonging to a different principal returns `transfer_unknown`, indistinguishable
from one that never existed, so the operation is not an oracle for other
clients' activity.

### `pipe.publish`

| | |
|---|---|
| `in` | `{ "room_id": "<room_id>", "target": <target>, "audience": <audience> }` |
| `out` | `{ "room_id": "<room_id>", "pipe_id": "<pipe_id>", "target": <target>, "audience": <audience>, "event_id": "<event_id>", "pos": "<uint>" }` |
| Errors | `pipe_target_refused`, `policy_refused` |

`target` is one object, `{ "host": "<string>", "port": "<uint>" }`, rather than
two sibling fields. It is one value — a client sends it, the reply echoes it, and
`pipe_target_refused` returns it verbatim — and splitting it across two fields
would mean the error either echoes a partial target or invents a spelling the
request never used.

`audience` is a variant naming who may connect:

```json
{ "state": "room" }
{ "state": "subjects", "subject_ids": ["<subject_id>"] }
```

It is **required**, like every request field. A pipe is a peer-reachable tunnel,
so who may open it is not a value to leave to a default — `room` has to be said
out loud rather than fallen into. A caller outside the audience answers
`pipe_unknown`, indistinguishable from no such pipe, which is what makes a
separate audience-refusal code unnecessary and a separate `pipe_audience`
capability actively harmful.

The target MUST be loopback — IPv4 in `127.0.0.0/8` or IPv6 `::1` — and
`target.port` MUST be in `1..=65535`. Anything else is `pipe_target_refused`
carrying the rejected `target`, **not** the generic `policy_refused`, because
"your target is not allowed" and "you may not publish in this room" are refusals
a user responds to differently.

A private-range address such as `192.168.1.10` is **refused**: being unroutable
from the internet is not the same as being local, and the policy is loopback,
not "probably safe". The refusal is a local decision made before publication, so
it costs no round trip and reveals nothing to peers.

v1's hardcoded `label` (always `"pipe"`) and `kind` (always `"tcp"`) are
removed.

### `pipe.list`

| | |
|---|---|
| `in` | `{ "room_id": "<room_id>", "cursor": <cursor>, "direction": "forward", "limit": "<uint>" }` |
| `out` | `{ "room_id": "<room_id>", "pipes": [ { "pipe_id": "<pipe_id>", "published_by": "<subject_id>", "device_id": "<device_id>", "published_at": "<ts>", "link": <link>, "connected": "<bool>" } ], "truncated": <truncated> }` |
| Errors | `pipe_index_unreadable` |

v1's single `connected` boolean is **split into two separately named facts**: it
was named as if it were reachability while being a purely local runtime fact.
`link` is whether the publisher's device can be reached — **pipe reachability is
a protocol fact** (#50/#94/#79) — and `connected` is whether this daemon
currently holds a local connection. Both are per-device answers, so `link` is
the same type `room.peers` and `file.list` use.

### `pipe.connect`

| | |
|---|---|
| `in` | `{ "room_id": "<room_id>", "pipe_id": "<pipe_id>" }` |
| `out` | `{ "pipe_id": "<pipe_id>", "connection_id": "<string>", "local": <target> }` |
| Errors | `pipe_unreachable`, `pipe_unknown`, `pipe_revoked`, `room_not_live` |

A caller outside the pipe's audience answers `pipe_unknown`, indistinguishable
from no such pipe. Requires liveness.

### `pipe.release`

| | |
|---|---|
| `in` | `{ "connection_id": "<string>" }` |
| `out` | `{ "connection_id": "<string>", "released": true }` |
| Errors | `connection_unknown` |

Releases a **local** connection, so it names the connection rather than the
pipe.

### `pipe.revoke`

| | |
|---|---|
| `in` | `{ "room_id": "<room_id>", "pipe_id": "<pipe_id>" }` |
| `out` | `{ "pipe_id": "<pipe_id>", "event_id": "<event_id>", "pos": "<uint>", "revoked_at": "<ts>" }` |
| Errors | `pipe_not_publisher`, `pipe_unknown` |

Withdraws a published pipe as a signed fact. Re-revoking succeeds and returns
the original withdrawal, like every other terminal withdrawal in this record.

### `stream.subscribe`

| | |
|---|---|
| `in` | `{ "room_id": "<room_id>", "from": <cursor> }` |
| `out` | `{ "room_id": "<room_id>", "from_pos": "<uint>" }` |
| Errors | `subscription_limit_reached` |

**Naturally idempotent** — subscribing twice does not duplicate delivery and is
not an error. Scoped to the connection that holds it. Exceeding
`max_subscriptions_per_connection` is `subscription_limit_reached`, never a
silent drop.

`from` and `from_pos` are deliberately different types and are not two spellings
of one thing. `from` is a **cursor** — what the caller asks for, including
`{"state": "start"}`, which names no position because the caller does not yet
know one. `from_pos` is the concrete position that cursor **resolved to**, which
the caller needs in order to detect the first gap. A reply that merely echoed
the cursor would leave a client that sent `start` still unable to say where its
stream begins.

### `stream.unsubscribe`

| | |
|---|---|
| `in` | `{ "room_id": "<room_id>" }` |
| `out` | `{ "room_id": "<room_id>", "unsubscribed": true }` |
| Errors | `subscription_unknown` |

### `stream.resync`

| | |
|---|---|
| `in` | `{ "room_id": "<room_id>", "from_pos": "<uint>" }` |
| `out` | `{ "room_id": "<room_id>", "events": [ <event> ], "next_pos": "<uint>", "truncated": <truncated> }` |
| Errors | `resync_required` |

The **authoritative** recovery. The client names the last position it holds and
the daemon returns either the events since it, or `resync_required` naming a
position to discard back to and re-read from. Resync is not best-effort and is
not "call `room.activate` again", which is what v1 clients did.

`from_pos` is a position rather than a cursor because recovery is defined over
positions: the client already holds one and is naming it. `next_pos` is the
position of the last event returned, or `from_pos` itself when `events` is
empty. Positions are exclusive on the low side, so the client resends `next_pos`
to get what follows — and because this reply has exactly one continuation
mechanism, there is no second value it can disagree with.

## Pushes, ordering, gap detection, and resync

### The push frames

`t` is closed at four. A frame type not listed here does not exist — the same
rule the [shared value types](#shared-value-types) apply to every variant, and
for the same reason: the committed corpus contains two mutually incompatible
spellings of the event push, `t: "event"` and `t: "room.event"`, because neither
was ever written down.

| `t` | Payload | Emitted when |
|---|---|---|
| `event` | `room_id`, and the [committed event](#the-committed-event) inline | A room event commits |
| `gap` | `room_id`, `from_pos`, `to`, `reason` | A position discontinuity is detected or forced |
| `peer` | `room_id`, `subject_id`, `device_id`, `link`, `generation` | A peer's link changes. **Depends on U1** |
| `transfer` | `transfer_op_id`, `transferred_bytes`, `total` | A transfer makes progress. **Depends on U2** |

A push carries `t` and never `id`; a reply carries `id` and never `t`. That is
the whole of how the two are told apart, so a frame carrying both, or neither, is
`malformed_frame`.

`peer.generation` is the connection generation U1 must supply, and it is what
makes a stale teardown discardable from the frame alone rather than by
inference. `transfer` frames go **only to the principal that started the
transfer** — a progress frame is otherwise an oracle for another client's
activity, exactly as `transfer.cancel` would be without its principal guard.

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
{ "t": "gap", "room_id": "<room_id>",
  "from_pos": 41,
  "to": { "state": "bounded", "pos": 57 },
  "reason": "backpressure" }
```

**Every room-scoped request names the room `room_id`, and every reply and frame
about one room echoes `room_id`.** One spelling, everywhere — an earlier draft
of this example said `room`, which is v1's spelling and is the sort of drift
that costs an adapter author an afternoon.

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

### The session principal

A principal is **a client's stable identity across reconnects**, and it is not
derivable from the credential. Every native client authenticates with the same
per-start portfile bearer token, so deriving a principal from the bearer would
collapse the WebView, every agent, and the CLI into one scope. A browser ticket
is single-use, so deriving it from the ticket would give a new principal on
every reconnect — destroying exactly the replay-after-reconnect property the
ledger exists to provide.

So v2 states it explicitly: **the client declares a `client_id` on the
upgrade** — a stable, client-generated opaque identifier it reuses across
reconnects and persists for as long as it wants its own `op_id` scope. The
principal is `(credential, client_id)`.

It travels as a query parameter on the upgrade, `GET
/ws?v=2&sg=<storage-generation>&cid=<client_id>`, for the same reason `v` and
`sg` do: a browser `WebSocket` constructor controls only the URL and the
subprotocol list. It is bounded by the codec like any other string, and unlike
`v` and `sg` its **absence is not refusal** — an omitted `cid` yields a fresh
ephemeral principal, which is the documented choice a short-lived CLI makes.
`cid` is not a credential and is never compared in constant time; putting it in
the URL is safe precisely because it grants nothing.

- A client that omits `client_id` gets a fresh ephemeral principal per
  connection and therefore no cross-reconnect replay. That is a legitimate
  choice for a short-lived CLI invocation, and it is refusal of a capability
  rather than an error.
- `client_id` is **not** a credential and grants nothing. It only partitions a
  namespace among clients that have already authenticated. A client that
  reuses another's `client_id` shares its `op_id` scope, which is the same
  trust boundary as sharing the bearer token — which those clients already do.
- Distinct `client_id`s MUST have isolated ledgers, so one local client can
  neither observe, replay, nor cancel another's operations.

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
| Scoped to one connection, `op_id` ignored | `stream.subscribe`, `stream.unsubscribe`, `stream.resync` — subscription state belongs to the connection that holds it, so a retry after a lost reply is a retry on a **new** connection with no prior subscription. Re-issuing is always safe and always correct |
| Scoped to the principal, names its target | `transfer.cancel` — see below |

Because `op_id` lives in [the envelope](#request-deduplication-lives-in-the-envelope), an
operation that does not deduplicate **accepts it and ignores it**. It is never
`unrecognised_field`. The three `stream.*` operations are the case that matters:
refusing an `op_id` there would make a client's uniform envelope-builder an
error, to enforce a rule that only ever existed because `op_id` had been
misfiled as an argument.

`transfer.cancel` names **the transfer being cancelled** in a request field
called `transfer_op_id`. The caller knows that value because it chose it, and
can therefore cancel across a reconnect on which a cancel-request identifier
would be meaningless. Cancel is naturally idempotent — cancelling an
already-cancelled transfer reports the existing cancellation — so it needs no
identifier of its own, and the envelope `op_id` on it is ignored.

Every operation marked `M` appears in exactly one row above.

A replayed `op_id` returns the **original** result and performs no second
effect. `invite.mint` in particular MUST return the original capability, never
a second grant. An `op_id` replayed with a *different* request body is
`op_id_conflict`, not a silent second effect.

`transfer.cancel` is authorized by **`(session principal, transfer_op_id)`** —
the same key as the ledger, not `(subject, …)` and not the connection.

The connection is wrong because a transfer whose originating connection has
dropped could then never be cancelled, which is precisely the case cancellation
exists for. The bare subject is wrong because a daemon has one subject, so any
local client could cancel any other's transfer. The principal is the only scope
that survives a reconnect without becoming daemon-global.

Cancelling a `transfer_op_id` belonging to a different principal returns
`transfer_unknown` — indistinguishable from one that never existed, so the
operation is not an oracle for other clients' activity.

### Repeating a withdrawal

**`member.remove` and `invite.revoke` answer alike: repeating either against an
already-terminal fact succeeds**, returns the original withdrawal's `event_id`,
`pos`, and resulting state, and authors no second event.

They are stated together because an earlier draft made them differ — remove
succeeding, revoke refusing `capability_revoked` — with no reason that
distinguished them. There is none to find: both are re-assertions of a terminal
fact by the party that authored it, and both change nothing. Two operations with
identical shape must not have opposite answers.

`capability_revoked` and `capability_expired` are **redemption-side** codes.
They answer the redeemer, telling them why they cannot join. An authority
repeating its own withdrawal is not their audience.

## Errors

**The taxonomy is 60 codes.** Every code is machine-readable, carries typed
fields rather than prose, and carries no `hint`. The tables below are the whole
of it — a code not listed here does not exist, and an implementation MUST NOT
mint one.

An earlier draft of this record said 55 without listing them, and the count was
unverifiable in both directions: the same draft introduced its distinctive codes
under a sentence saying "nine operations" above a table of fifteen rows. A
stated count that no table supports is how a taxonomy drifts. Every code below
is counted, and the group subtotals sum to the total.

### The error object

```json
{ "id": 42, "ok": false,
  "err": { "code": "invalid_argument",
           "field": "in.limit",
           "reason": { "state": "bound", "min": 1, "max": 500 } } }
```

`code` is always present. Every other key is fixed by the code, per the tables
below — an error carries exactly the fields its row names, no more and no fewer.

### Gate and transport — 7

Returned as a JSON body on a refused upgrade, or as the application close code
[the gate section](#rejections-are-machine-readable) tabulates.

| Code | Fields | Raised when |
|---|---|---|
| `forbidden_origin` | — | `Host`, or a present `Origin`, is not loopback |
| `protocol_unsupported` | `supported`, `client` (variant: `declared {v}` / `absent`) | `v` is absent or names an unsupported generation |
| `storage_generation_mismatch` | `daemon`, `client` (variant: `declared {sg}` / `absent`) | `sg` is absent or does not equal the daemon's |
| `unauthenticated` | — | The credential is absent, wrong, or a spent ticket |
| `not_ready` | — | The daemon is not yet serving, or its subject store cannot be read |
| `frame_too_large` | `limit_bytes` | A frame exceeds `max_frame_bytes`; the connection closes `4005` unparsed |
| `idle_timeout` | `idle_ms` | No activity within `idle_timeout_ms`; the connection closes `4004` |

`not_ready` is also the answer when the subject store exists but cannot be read.
A `hello` cannot carry an error code, and a `hello` degraded into a third
`subject.state` arm forces every client to branch on a condition it can do
nothing about. Failing the connection closed is both simpler and more honest.

### Envelope and structure — 3

| Code | Fields | Raised when |
|---|---|---|
| `malformed_frame` | — | The frame is not JSON, or decodes to no envelope with a usable `id`. Closes `4007` |
| `unknown_operation` | `op` | `op` names no operation in this generation |
| `invalid_argument` | `field`, `reason` | Step 1 of [validation order](#validation-order) refused |

**`invalid_argument` carries exactly two fields.** `field` is a dotted path into
the frame — `in.progress.percent`, not `percent` — so a nested violation is
nameable without a second convention. `reason` is a closed variant:

| Arm | Payload | Meaning |
|---|---|---|
| `missing` | — | A required key is absent |
| `unrecognised_field` | — | A key the operation does not define |
| `type` | `expected` | Wrong JSON type |
| `format` | — | Right type, unparseable as its domain |
| `bound` | `min`, `max` | A numeric or length bound was violated |

Bound information lives **inside the `bound` arm**, never as a top-level `max`.
This is why an over-maximum `limit` is `invalid_argument` and never
`resource_exhausted`: `resource_exhausted` means a served limit was reached *by
consumption*, whereas asking for a page larger than the served maximum is a
malformed argument. It is refused, never silently clamped.

A frame with a usable `id` always gets a correlated error reply. Only a frame
whose `id` cannot be recovered closes the connection, because there is nothing
to correlate a reply to. `4007` is added to the close-code table for it.

### Subject — 2

| Code | Fields | Raised when |
|---|---|---|
| `subject_absent` | — | The operation needs a local subject and none exists |
| `subject_store_unwritable` | — | `subject.ensure` cannot persist the subject it created |

### Room and membership — 8

| Code | Fields | Raised when |
|---|---|---|
| `room_not_available` | `room_id` | No such room, **or** the room exists and the caller is not a member |
| `membership_ended` | `room_id`, `standing` | The caller's standing is `left` or `removed` |
| `insufficient_standing` | `room_id`, `required`, `held` | The caller is an active member whose `role` is below what the operation needs |
| `room_not_live` | `room_id` | The operation requires an active transport and the room is not live |
| `room_still_active` | `room_id` | `room.archive` was called on a room the caller has not left |
| `room_index_unreadable` | — | The accepted-room index cannot be read |
| `membership_unresolved` | `room_id`, `subject_id` | The fold cannot resolve a member's standing |
| `member_unknown` | `room_id`, `subject_id` | The named subject is not a member of this room |

**`insufficient_standing`'s `required` and `held` are `role` tokens**, not
capability tokens. The error is only reachable by an active member — a
non-member gets `room_not_available` and a former member gets `membership_ended`
— so the only thing that can be lacking is role. That makes the field set total
without a second vocabulary and without disclosing a capability set.

**Four operations require `role: "authority"`. Every other operation is open to
any active member.**

| Requires `authority` | Why |
|---|---|
| `member.remove` | Ending someone else's membership is the room's decision, not a peer's |
| `invite.mint` | Admitting a new member is the same decision taken in advance |
| `invite.revoke` | Withdrawing a grant belongs to whoever could make it |
| `invite.list` | The invite index is the authority's own record of what it issued; enumerating it to any member would disclose who has been invited and not yet joined |

Stating this as one list rather than per-operation is deliberate. Step 6 of
[validation order](#validation-order) is normative for all 33 operations, so a
reader who found the rule stated only under `member.remove` would reasonably
conclude the other 32 never raise it — which is how a permission gets
implemented as open by accident.

`pipe.revoke` is **not** on this list. It is restricted to the pipe's publisher,
which is a narrower relation than role and answers `pipe_not_publisher`; an
authority who did not publish a pipe cannot revoke it either.

`room_not_available` **echoes the `room_id` the caller sent**. The non-oracle
guarantee is equality of the response across both causes, not fieldlessness; a
value the caller supplied one frame earlier discloses nothing back to it.

### Idempotency and capacity — 3

| Code | Fields | Raised when |
|---|---|---|
| `op_id_conflict` | `op_id` | The `op_id` was seen before with a different request body |
| `resource_exhausted` | `resource`, `limit` | A served limit was reached by consumption |
| `shutdown_in_progress` | — | A `daemon.stop` is already sequenced |

### Rooms — 5

| Code | Fields | Raised when |
|---|---|---|
| `room_name_invalid` | `reason` (the `invalid_argument` variant) | The name fails the stated bounds |
| `sole_authority_cannot_leave` | `room_id` | The caller is the room's only authority |
| `cursor_unknown` | `cursor` | A well-formed cursor names a position the store can no longer serve |
| `transport_unavailable` | `room_id` | `room.activate` cannot bring the room live |
| `authority_cannot_be_removed` | `room_id`, `subject_id` | `member.remove` names an authority |

`cursor_unknown` is deliberately not `invalid_argument`: a cursor that is
structurally valid but names a pruned position is a fact about the store, not a
defect in the request, and a client responds to it by resyncing rather than by
fixing its code.

### Invitations — 8

| Code | Fields | Raised when |
|---|---|---|
| `invite_index_unreadable` | — | The invite index cannot be read |
| `invite_unknown` | `invite_id` | No such invite for this authority |
| `invitee_already_member` | `room_id`, `subject_id` | The named identity already holds membership |
| `capability_invalid` | — | The presented capability does not verify |
| `capability_expired` | `expired_at` | The capability's absolute expiry has passed |
| `capability_revoked` | `revoked_at` | The capability was withdrawn before expiry |
| `capability_redeemed` | `redeemed_at` | The capability has already been converted into membership |
| `role_not_grantable` | `requested` | `invite.mint` named a role this record does not permit minting |

**`capability_invalid`, `capability_expired`, and `capability_revoked` are
redemption-side codes.** They answer the *redeemer*, telling them why they cannot
join. An authority repeating its own withdrawal is not their audience — see
[Idempotency](#idempotency-and-retry).

`capability_redeemed` is the exception, and it faces **both** ways. To a
redeemer it means "you already used this". To an authority calling
`invite.revoke` it means "there is nothing left to withdraw — this became a
membership, and `member.remove` is the operation that ends one". Those are the
same fact answered to two audiences, not two codes sharing a name, which is why
it is `invite.revoke`'s distinctive code without contradicting the paragraph
above.

`capability_invalid` carries no fields on purpose. It is the answer for a forged
capability, a capability for a room that does not exist, and a capability naming
a different identity, and those must be indistinguishable for the same reason
`room_not_available` is one code: `invite.redeem` is the only operation a
non-member can reach, so it is the one place a membership oracle could be built.

### Timeline — 4

| Code | Fields | Raised when |
|---|---|---|
| `message_too_large` | `declared_bytes`, `limit_bytes` | The body exceeds `max_message_body_bytes` |
| `status_label_unknown` | `label` | The label is outside the closed vocabulary |
| `status_subject_unknown` | `room_id`, `subject_id` | The named agent has no status history |
| `fleet_projection_unavailable` | — | The projection cannot be built |

### Files and transfers — 9

| Code | Fields | Raised when |
|---|---|---|
| `file_index_unreadable` | — | The room's file index cannot be read |
| `file_unknown` | `file_id` | No such file in this room |
| `file_not_fetched` | `file_id` | `file.read` named a file whose bytes are not held locally |
| `file_too_large` | `declared_bytes`, `limit_bytes`, `enforced_at` | Over-limit, naming which enforcement point fired |
| `digest_mismatch` | `expected`, `observed` | Content did not verify. **Never returned for a size refusal** |
| `declared_size_mismatch` | `declared_bytes`, `observed_bytes` | `file.share`'s streamed bytes did not match its declared size |
| `provider_unreachable` | `file_id`, `providers` | No provider holding the file could be reached |
| `transfer_unknown` | `transfer_op_id` | No such in-flight transfer for this principal |
| `transfer_stalled` | `transferred_bytes`, `total` (`<byte_total>`) | No forward progress within the stall window |

`file_too_large.enforced_at` names one of the **five daemon-side** enforcement
points of the six in [the shared-file size policy](shared-file-size.md):
`stage_declared`, `stage_stream`, `authoring`, `fetch_preflight`,
`fetch_stream`. The sixth is the **client preflight**, which by definition
never reaches the daemon and is proven by a client-side case asserting zero
bytes are sent.

### Pipes — 8

| Code | Fields | Raised when |
|---|---|---|
| `pipe_target_refused` | `target` | `target.host` is not loopback, or `target.port` is outside `1..=65535` |
| `pipe_index_unreadable` | — | The room's pipe index cannot be read |
| `pipe_unknown` | `pipe_id` | No such pipe, **or** the caller is outside its audience |
| `pipe_unreachable` | `pipe_id`, `link` | The pipe's publisher device could not be reached |
| `pipe_revoked` | `pipe_id`, `revoked_at` | The pipe was withdrawn |
| `pipe_not_publisher` | `pipe_id` | `pipe.revoke` named a pipe this subject did not publish |
| `connection_unknown` | `connection_id` | `pipe.release` named no local connection |
| `policy_refused` | `room_id` | Publishing is not permitted in this room |

`pipe_unknown` covers the out-of-audience case for the same reason
`room_not_available` covers non-membership: a caller MUST NOT be able to
distinguish a pipe it is not entitled to from one that does not exist. This is
what makes a separate `pipe_audience` capability unnecessary, and a separate
audience refusal code actively harmful.

`pipe_target_refused` replaces the generic `policy_refused` for the
publish-target case specifically, so a client can tell "your target is not
allowed" from "you are not permitted to publish here" — two refusals a user
must respond to differently.

### Stream — 3

| Code | Fields | Raised when |
|---|---|---|
| `subscription_limit_reached` | `limit` | This connection holds `max_subscriptions_per_connection` |
| `subscription_unknown` | `room_id` | No such subscription on this connection |
| `resync_required` | `room_id`, `from_pos` | The named position can no longer be served; discard and re-read from `from_pos` |

### One distinctive code per operation

**Every operation has at least one error code specific to it.** A conformance
corpus that can only assert a shared code proves nothing about the operation it
is supposed to cover: `subject_absent` returned by `room.list` and by
`fleet.list` are the same assertion twice.

| Operation | Distinctive code | Operation | Distinctive code |
|---|---|---|---|
| `subject.ensure` | `subject_store_unwritable` | `status.post` | `status_label_unknown` |
| `daemon.stop` | `shutdown_in_progress` | `status.history` | `status_subject_unknown` |
| `room.create` | `room_name_invalid` | `fleet.list` | `fleet_projection_unavailable` |
| `room.list` | `room_index_unreadable` | `file.share` | `declared_size_mismatch` |
| `room.activate` | `transport_unavailable` | `file.list` | `file_index_unreadable` |
| `room.deactivate` | **exempt — see below** | `file.fetch` | `provider_unreachable` |
| `room.leave` | `sole_authority_cannot_leave` | `file.read` | `file_not_fetched` |
| `room.timeline` | `cursor_unknown` | `transfer.cancel` | `transfer_unknown` |
| `room.members` | `membership_unresolved` | `pipe.publish` | `pipe_target_refused` |
| `room.archive` | `room_still_active` | `pipe.list` | `pipe_index_unreadable` |
| `room.peers` | `room_not_live` | `pipe.connect` | `pipe_unreachable` |
| `member.remove` | `authority_cannot_be_removed` | `pipe.release` | `connection_unknown` |
| `invite.mint` | `invitee_already_member` | `pipe.revoke` | `pipe_not_publisher` |
| `invite.list` | `invite_index_unreadable` | `stream.subscribe` | `subscription_limit_reached` |
| `invite.revoke` | `capability_redeemed` | `stream.unsubscribe` | `subscription_unknown` |
| `invite.redeem` | `capability_invalid` | `stream.resync` | `resync_required` |
| `message.send` | `message_too_large` | | |

**`room.deactivate` is exempt, and the exemption is recorded rather than
papered over.** Every way it can fail is already a cross-cutting refusal —
no subject, no such room, membership ended. Deactivating a room the caller may
act in always succeeds, because it withdraws local participation and asks
nothing of the network. Minting a code that no reachable state produces would
buy a green cell in a coverage table and nothing else. The manifest carries this
as a named exemption with this reason.

### Codes this record refuses to define

The corpus currently asserts fourteen codes that v2 does not have. Each is a
fixture bug, not a taxonomy gap, and each is listed with the code that replaces
it.

| Corpus code | Verdict |
|---|---|
| `room_unknown` | **A defect, not a synonym.** A distinct code for "no such room" is exactly the membership oracle `room_not_available` exists to prevent. Every use becomes `room_not_available` |
| `not_found`, `forbidden`, `conflict`, `already_exists` | v1's generic HTTP-shaped codes. Each becomes the specific code for its operation |
| `invalid_params` | → `invalid_argument` |
| `hash_mismatch` | → `digest_mismatch` |
| `identity_missing`, `subject_missing` | → `subject_absent` |
| `identity_exists`, `subject_exists` | Removed outright. `subject.ensure` is naturally idempotent, so an existing subject is **success with `created: false`**, and this record already removes `identity_exists` for reporting success as failure |
| `subject_unreadable` | → `not_ready`, which closes the connection rather than answering an operation |
| `cursor_invalid` | → `cursor_unknown` for a pruned position, or `invalid_argument` with `reason: {"state": "format"}` for a malformed one. The single corpus code conflated the two |
| `<the case's code>` | Not a code at all — an unsubstituted placeholder left in a fixture |

Eleven codes in the tables above have no corpus case yet:
`authority_cannot_be_removed`, `capability_redeemed`, `cursor_unknown`,
`declared_size_mismatch`, `idle_timeout`, `malformed_frame`,
`pipe_index_unreadable`, `pipe_not_publisher`, `room_still_active`,
`subject_store_unwritable`, and `transport_unavailable`. They are real codes
with no coverage, which is a corpus gap rather than a specification gap, and
they are named in the manifest so the gap cannot read as coverage.

### The non-oracle property

A caller that is not a member of a room MUST NOT be able to distinguish "no
such room" from "that room exists and you are not a member". Both answer
`room_not_available`, echoing the `room_id` the caller supplied, with identical
fields and indistinguishable timing.

This is normative, it applies to every room-scoped operation and to the push
stream, and it is the reason `room_not_available` is one code rather than two.

The property generalises to two other places, and both are stated as
consequences rather than left to be re-derived:

- **`invite.redeem`** is the only operation a non-member can reach, so it is the
  one place a non-member could probe. `capability_invalid` is fieldless and
  covers forgery, an unknown room, and a mismatched identity alike.
- **`pipe_unknown`** covers both "no such pipe" and "you are outside its
  audience", for the same reason.

Timing is part of the guarantee in all three cases. A refusal that is
constant-time in its code but not in its latency is still an oracle.

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
  manifest.json          required kinds per operation, coverage, exemptions with reasons
  README.md              the independence rule, the case shape, how to add one
  handshake.json         Layer 0/1/2, gate order, close codes, envelope, bounded parsing
  subject-daemon.json    subject lifecycle, daemon stop
  rooms.json             rooms, membership, the non-oracle property, archives
  invites.json           mint / list / revoke / redeem, capability failures
  timeline-streams.json  messaging, positions, gap detection, resync, agents
  files.json             sharing, fetching, the size limit and its enforcement points
  pipes.json             publish / connect / release / revoke, reachability
```

One flat file per domain. Case `name` is a **corpus-wide unique identifier**,
not unique-per-file: a harness indexes by it, so a collision silently drops a
case.

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
- How a client renders a severity. v2 states the derivation, not the display.
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
