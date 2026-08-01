# Protocol v2 conformance corpus

Hand-authored, language-neutral fixtures for
[protocol v2](../../docs/protocol-v2.md). Intended to be replayed by an
independent harness against every adapter: the codec, the typed daemon, the
Rust adapters, and the agent cutover.

This directory is data. It contains no TypeScript, no Rust, and no test runner,
because a corpus that only one language can replay is not a conformance corpus.

## The independence rule

**These fixtures were authored from the specification, never from an
implementation.** No daemon was run to produce an expected value. Every one was
derived by reading `docs/protocol-v2.md` and reasoning about it.

That is not pedantry. A corpus generated from the code under test proves only
that the code agrees with itself — it cannot catch a case where the
implementation and the specification disagree, which is the only thing a
conformance corpus is for. #161 states it as an acceptance criterion.

The v1 surface was consulted **only** to know what to avoid transcribing.

## Status — replayable

The specification is canonical and the fixtures are normalized to the DSL
(#213). Every case conforms; the corpus can be replayed by an independent
harness and may be cited as evidence for any adapter.

| | |
|---|---|
| Cases | 342 |
| Cases conforming to the DSL | **342** |
| Distinct step verbs in use | 7, the closed set (`call`, `http`, `upgrade`, `send`, `await`, `control`, `assert`) |
| Codes in the taxonomy without a case | **0** |
| Blocked on upstream | 10 |

Two cases were retired in the files pass-3 re-transcription: `declared_bytes` is a
required `<uint>` and there are no optional request fields, so an *unknown*
declared size is not expressible in v2, and both cases collapsed onto ones that
already exist.

Normalization was tracked as **#213** and landed with the promotion of the
specification to canonical: the fourteen refused codes retired, the fixture
bugs #212 catalogued fixed, every case retranscribed, the uncovered codes
authored, and the manifest recomputed from the fixtures.

**Ten cases are blocked on upstream work** (U1, U2, U3 in the spec). They
**fail**; they do not skip. A skipped case reads as coverage and quietly
becomes permanent. A failing case reads as work.

### What normalization did not do

The corpus's value rests entirely on the independence rule, and the fastest way
to normalize the fixtures would have destroyed it: a fixture rewritten by pure
pattern substitution is no longer a transcription of something independently
known to be right. Retranscription therefore re-expressed each assertion in the
DSL and folded every construct that had no DSL home — client-side rendering
assertions, corpus-literal scans, frame-order pinners, repeat/collect loops —
into a step note a reviewer can audit, rather than silently dropping it. No
expected value was derived from an implementation.

## Layout

```
manifest.json        required kinds per operation, coverage, exemptions with reasons
handshake.json       Layer 0/1/2, gate order, absence-is-refusal, close codes, envelope
subject-daemon.json  subject lifecycle, daemon stop
rooms.json           rooms, membership, the non-oracle property, archives
invites.json         mint / list / revoke / redeem, capability failures
timeline-streams.json messaging, positions, gap detection, resync
files.json           sharing, fetching, the size limit and its enforcement points
pipes.json           publish / connect / release / revoke, reachability as fact
```

One flat file per domain. Case `name` is a **corpus-wide unique identifier**,
not unique-per-file: a harness indexes by it, so a collision silently drops a
case.

---

# The fixture DSL

Normative. A harness implements exactly this, and a fixture that uses anything
not defined here is invalid rather than interestingly extended.

The design constraint is that it must lose no assertion the corpus currently
expresses while replacing 178 ad-hoc verbs and three `assert` dialects — string,
object, and array — with one language. Every construct below exists because some
committed fixture needs it.

## The case object

```json
{
  "name": "room_list_reports_standing_and_capabilities_from_local_evidence",
  "kind": "success",
  "operation": "room.list",
  "intent": "Proves room.list answers from local evidence with zero network activity, catching any change that makes the room list depend on liveness.",
  "requires": ["subject", "room:live", "observe:network"],
  "steps": [ … ],
  "blocked_on_upstream": "U1"
}
```

| Key | Required | Value |
|---|---|---|
| `name` | yes | `snake_case`, corpus-wide unique |
| `kind` | yes | one of the eight below |
| `operation` | yes | one of the 33 operation names, or `null` |
| `intent` | yes | prose naming the breaking change this case would catch |
| `requires` | yes | array of preconditions, closed vocabulary below |
| `steps` | yes | non-empty array |
| `blocked_on_upstream` | no | `"U1"`, `"U2"`, or `"U3"` |
| `blocked_on_record` | no | Named settled record/corpus contradiction retained as an expected failure until the stale case is retired |

Both block forms **fail, never skip**. A passing blocked case is reported as a
surprise; a setup/runner error is still an error rather than an expected block.

`kind` is closed: `success`, `error`, `malformed`, `boundary`, `authorization`,
`handshake`, `push`, `ordering`.

**`operation: null` is legal and means the case is not about one operation** —
gate behaviour, envelope framing, cross-room ordering. 103 of the 335 committed
cases are in this class, and the manifest's coverage table counts only the
attributed ones, which is why its rows sum to 232 rather than 335. A case may
not use `null` merely because attributing it is inconvenient.

`intent` is required and is not decoration: a case whose intent cannot name the
breaking change it would catch is not worth running.

## Steps

A step has **exactly one verb**. The verb set is closed at seven.

| Verb | Value | Meaning |
|---|---|---|
| `call` | operation name | Invoke an operation on the current session |
| `http` | `{method, path, headers, body}` | A Layer 0 or `/api/session` request |
| `upgrade` | `{query, headers}` | A Layer 1 `/ws` upgrade attempt |
| `send` | raw frame value | Write bytes that may not be a valid frame |
| `await` | `{push}`, `{frame}`, or `{reply}` | Wait for a specific frame, by type or by correlating `id` |
| `control` | `{do, …}` | Drive the harness, not the daemon |
| `assert` | array of assertions | Everything else |

Any step may additionally carry:

| Key | Meaning |
|---|---|
| `in` | the request body, for `call` |
| `op_id` | the envelope `op_id`, for `call` — **never inside `in`** |
| `on` | which session this step runs on |
| `expect` | the reply matcher |
| `save` | capture values from this step's result into variables |
| `stream` | the bytes the operation streams, for `call` |
| `note` | prose for a human; a harness ignores it |

`save` is an **auxiliary key, not a verb**. It always captures from the step it
sits on, so making it a verb would force every capture into a second step with
nothing to capture from.

### `stream` — the bytes an operation carries

Two operations carry bytes beside their request, because the record folds v1's
separate HTTP upload edge into the operation itself: `file.share` streams bytes
**to** the daemon, and `file.read` streams them **back**.

```json
{ "call": "file.share",
  "in": { "room_id": "$rid", "name": "design.pdf",
          "declared_bytes": 4096, "declared_content_type": "application/pdf" },
  "stream": { "send_bytes": 4096 } }

{ "call": "file.read", "in": { "room_id": "$rid", "file_id": "$fid" },
  "stream": { "receive_bytes": 4096 } }
```

`stream` is **auxiliary, not a verb**, for the same reason `save` is: the bytes
and the declaration are one operation, so a separate verb would leave a step
holding a stream with nothing to stream for — and it would let a fixture put
them in the wrong order, which is exactly the ambiguity the record removes by
combining the two edges.

It takes exactly one key, fixed by the operation: `send_bytes` for `file.share`,
`receive_bytes` for `file.read`. A `stream` on any other operation is invalid,
because no other operation streams. The value is a `<uint>`, a `$variable`, or a
computed node, so a boundary case can say "exactly the served limit" without
compiling the number in.

**`stream.send_bytes` is deliberately independent of `in.declared_bytes`.**
Declaring one size and sending another is not a malformed fixture — it is the
only way to reach `declared_size_mismatch`, and it is what separates the size
policy's `stage_declared` enforcement point from `stage_stream`. A DSL that
forced them equal would make three of the record's five daemon-side enforcement
points untestable.

Without this key the files domain is not transcribable: `stage_stream`,
`fetch_stream`, `declared_size_mismatch`, and the never-render-inline
obligations on `file.read` all describe what happens to bytes in flight, and
`observe: bytes_streamed` can only assert the outcome, never cause it.

`note` is the only annotation key. The committed corpus also uses `why`,
`comment`, `intent_note`, and `meaning` for the same thing.

### `control` — driving the harness

`control` is discriminated by `do`, closed at ten. It is the one verb that does
not touch the daemon's protocol surface, so leaving it as an open object would
have let every harness invent its own dialect — which is what the committed
corpus already did, spelling this idea four ways (`harness`, `control`, `fault`,
`trigger`) split cleanly by authoring file.

| `do` | Keys | Effect |
|---|---|---|
| `advance_clock` | `ms` | Move the harness clock forward |
| `idle` | `ms` | Wait without producing activity |
| `disconnect` | `on` | Drop a session's transport without a close frame |
| `reconnect` | `on` | Re-establish it, same principal |
| `inject_fault` | `fault` | Force a named fault condition |
| `set_limit` | `limit`, `value` | Override a served limit for this case |
| `stop_daemon` | `daemon` | Terminate a daemon process |
| `start_daemon` | `daemon` | Start a previously stopped daemon — restart cases cannot be written without it; the pair expresses one restart, never a fresh daemon |
| `start_transfers` | `count` | Begin N concurrent transfers |
| `pause_link` | `between` | Suspend transport between two daemons |

`inject_fault`'s `fault` is a **taxonomy code**, so a fault a case wants that
names no code is a signal the taxonomy is incomplete — that is how
`room_index_unreadable` and its four siblings were found. Two conditions are
deliberately not codes and are named directly: `backpressure` and
`subscription_lapse`, which are the two `gap.reason` arms a harness must be able
to force in order to test gap detection at all.

`set_limit` exists because several boundary cases need a limit small enough to
reach — `max_connections` and `max_subscriptions_per_connection` cannot be
exercised against production values in a test.

### `on` — which session

`on` names a session established by `requires`, defaulting to `subject:self`'s
primary connection when omitted:

```json
{ "call": "room.create", "on": "subject:self",   "in": { "name": "Build" } }
{ "call": "room.timeline", "on": "subject:second", "in": { "…": "…" } }
{ "call": "room.list",   "on": "subject:self#2", "in": {} }
```

**Nothing else selects an actor**, and without this key roughly a third of the
corpus is inexpressible. The committed fixtures spell the same idea four ways —
`as` (504 uses, 25 distinct actor labels), `conn`, `session`, and `client` — and
the cases that need it are not marginal: the non-oracle property needs a
non-member, `op_ids_do_not_collide_across_session_principals` needs two
principals on one daemon, and every reconnect case needs two connections for one
subject. `#2` names a second connection for the same principal, which is what
distinguishes a per-connection scope from a per-principal one.

## `expect` — one reply matcher

`expect` is a single form discriminated by `ok`, exactly as the wire is:

```json
{ "expect": { "ok": true,  "out": { "room_id": "<room_id>", "live": true } } }
{ "expect": { "ok": false, "err": { "code": "room_not_available" } } }
```

For `http` and `upgrade` steps the matcher instead carries `status`, `headers`,
and `body`:

```json
{ "expect": { "status": 426, "body": { "code": "protocol_unsupported" } } }
```

The record makes the refusal statuses normative — `426`, `401`, `403` — and
requires that `POST /api/session` prove possession of the daemon token. Neither
is assertable without a status and a header slot, so both exist.

**Matching is a subset match by default.** Keys named in `out` or `err` must be
present and equal; keys not named are not constrained. This is what makes a
fixture robust against fields added later without making it vacuous.

To pin a key set exactly, use the `exact_keys` assertion. To require a key be
**absent**, use `absent`. Silence in `expect` means "not asserted", never
"asserted absent" — the distinction is load-bearing, and conflating the two is
how a fixture starts passing against an implementation that leaks a field.

`expect` replaces **all 42 `expect_*` verbs** the corpus currently uses. They
divide three ways, and the division is the point — a flat list of replacements
would hide that a third of them are not reply matchers at all:

| Corpus verbs | Become |
|---|---|
| `expect_error`, `expect_subset`, `expect_ok`, `expect_envelope`, `expect_body`, `expect_body_shape`, `expect_final_body`, `expect_hello`, `expect_hello_subset`, `expect_status`, `expect_content_type`, `expect_upgrade`, `expect_upgrade_error` | `expect`, directly |
| `expect_absent`, `expect_no_null`, `expect_identical_to`, `expect_one_of`, `expect_any_of`, `expect_all`, `expect_each_subset`, `expect_every_element`, `expect_across`, `expect_at_least_one_error`, `expect_all_replies`, `expect_hello_assert`, `expect_rendering` | an `assert` predicate — they are assertions wearing an `expect_` prefix |
| `expect_frame`, `expect_push`, `expect_no_push`, `expect_frame_order`, `expect_each_frame`, `expect_close`, `expect_no_further_frames_of_type`, `expect_transport`, `expect_process`, `expect_connect_failure`, `expect_timing_indistinguishable`, `expect_reply_for_id`, `expect_reply_for_id_2`, `expect_error_for_id`, `expect_no_reply_for_id` | `await` or an `observe` assertion — they are about frames and processes, not about one reply |

`expect_reply_for_id` and its three siblings deserve their own note: they exist
because **replies may arrive out of order**, which the record makes normative.
A harness that could only match replies in request order would silently pass an
implementation that violated it. `await {reply: "$id"}` is how a case names the
reply it means.

## `assert` — one assertion form, two families

`assert` is **always an array of objects**. It is never a bare string and never a
single object; the committed corpus uses all three, which is the single largest
source of the dialect problem.

### Value assertions

```json
{ "path": "out.rooms[*].role", "op": "member_of", "value": ["authority", "member"] }
```

`path` is a dotted path rooted at `out`, `err`, `frame`, or a `$variable`.
`[*]` is a wildcard over an array: the assertion must hold for **every** element.
This is what lets one predicate replace the corpus's `every_row_has_fields`,
`every_row_has_non_null`, `every_push_has_non_null`, `all_eq`, `every_has_key`,
and `every_row_has_value`.

`op` is closed at nineteen:

| `op` | `value` | Holds when |
|---|---|---|
| `eq` | any | Deep equality. `value` may be `"$var"` |
| `ne` | any | Deep inequality |
| `lt` `lte` `gt` `gte` | number | Numeric comparison |
| `member_of` | array | The value is one of these |
| `type` | type tag | The value inhabits that domain |
| `present` | — | The path resolves |
| `absent` | — | The path does not resolve |
| `exact_keys` | array | The object's key set is exactly this |
| `len` | `{op, value}` | The array/string length satisfies a nested comparison |
| `unique` | — | All values at a wildcard path are distinct |
| `increasing` | — | Strictly increasing |
| `non_decreasing` | — | Increasing or equal — **not** the same as `increasing` |
| `contiguous` | — | Strictly increasing by exactly 1 |
| `no_nulls` | — | No JSON `null` anywhere in the subtree |
| `byte_len` | `{op, value}` | Length **in bytes**, for the size-boundary cases |
| `eq_except` | `{path, keys}` | Deep equality with another path, ignoring named keys |

`eq_except` exists for **the non-oracle property**, which is the corpus's
flagship assertion and which `eq` alone cannot express: two refusals must be
identical *except* for the envelope `id` that correlates them. Without it, every
non-oracle case degrades into asserting the codes match, which proves half the
property.

`byte_len` is separate from `len` because the size limits are byte limits and
`max_message_body_bytes` is not a character count. Collapsing them would make
every multi-byte boundary case silently wrong.

`non_decreasing` is separate from `increasing` because `transferred_bytes` on
successive progress frames may legitimately repeat, and asserting strict
increase there would fail a correct implementation.

`no_nulls` deserves its own predicate rather than being a convention because
[the specification's no-null rule](../../docs/protocol-v2.md#bounded-parsing) is
normative for every frame, and a corpus that cannot assert it cannot test it.

These fourteen replace the corpus's `eq`, `equals`, `equal_modulo`, `all_eq`,
`same_value_as`, `codes_equal`, `identical_to`, `errors_identical`, `ne`, `neq`,
`not_equal`, `not_equals`, `distinct_from`, `len`, `len_eq`, `array_len`,
`byte_len`, `len_lte`, `len_gte`, `no_null`, `no_nulls_deep`, `no_null_values`,
`absent_key`, `no_keys`, `present_key`, `greater_than`, `strictly_increasing`,
`monotonic_non_decreasing`, `pos_strictly_increasing`, and
`contiguous_increasing`.

### Observation assertions

Some cases assert facts about the daemon's *behaviour*, not about a value in a
reply. Those are a separate family, because no path names them:

```json
{ "observe": "no_network_activity", "scope": "step" }
{ "observe": "close_code",  "value": 4004 }
{ "observe": "push_count",  "value": { "op": "eq", "value": 3 }, "room_id": "$r" }
{ "observe": "timing_indistinguishable", "between": ["step:3", "step:5"] }
```

`observe` is closed at ten. Each row states the keys it takes, because an
observation with no slot for its argument cannot be written down:

| `observe` | Additional keys | Holds when |
|---|---|---|
| `no_network_activity` | `scope` | No packet left the host during `scope` |
| `no_durable_mutation` | `scope` | No byte of the data dir changed during `scope` |
| `no_event_authored` | `scope` | No room event was written during `scope` |
| `bytes_streamed` | `value` | Bytes sent satisfies a nested comparison |
| `connection_open` | `on` | That session is still open |
| `close_code` | `value`, `on` | That connection closed with this code |
| `push_count` | `value`, `room_id` | Pushes received for a room satisfies a comparison |
| `no_push` | `room_id`, `scope` | No push arrived for that room |
| `timing_indistinguishable` | `between` | Two named steps are not separable by latency |
| `process_exited` | `value` | The daemon process exited with this status |

`scope` is `step`, `case`, or `step:<n>..step:<m>`.

Steps are addressable as `step:<n>`, one-indexed within the case. That is what
makes `timing_indistinguishable` writable at all — it is a claim about a *pair*
of steps, and the non-oracle property is not provable without it.

`timing_indistinguishable` exists because
[the non-oracle property](../../docs/protocol-v2.md#the-non-oracle-property)
requires indistinguishable *timing*, not merely an identical code. A corpus that
asserts only the code proves half the property.

## Variables

```json
{ "call": "room.create", "in": { "name": "Build" }, "save": { "r": "out.room_id" } }
{ "call": "room.timeline", "in": { "room_id": "$r", "…": "…" } }
```

`save` maps variable names to paths and rides on the step that produces them. A
`$name` reference resolves to the captured value anywhere a literal is legal.
Variables are scoped to the case.

This replaces `save`, `save_out`, and `save_error`, which differed only in which
root they read from — now expressed as the path's root.

### Computed values

A boundary case must say "exactly the served limit" and "one past it" without
compiling either number in — that is the whole point of serving limits. So a
value may also be a single-key computed node:

| Node | Meaning |
|---|---|
| `{"$add": ["$max", 1]}` | Arithmetic on captured values |
| `{"$sub": ["$max", 1]}` | |
| `{"$bytes_of_len": "$max"}` | A string of exactly that many bytes |
| `{"$concat": ["a", "$b"]}` | Concatenation |
| `{"$expires_in_ms": 3600000}` | An absolute RFC 3339 `Z` timestamp that many milliseconds from run time |
| `{"$unknown": "<room_id>"}` | A well-formed value of that domain naming nothing that exists |

Without these, **every case that probes a served limit would have to hard-code
the limit**, which is precisely the failure the record's served-limits object
exists to prevent — and the corpus already embeds thirteen ad-hoc operators to
avoid it.

`{"$unknown": …}` is separate from a literal because the non-oracle cases need a
value that is syntactically valid and semantically absent, and a fixture cannot
know a real identifier that does not exist.

## Type tags

A dynamic value is asserted as a **type tag** rather than a literal, so a fixture
pins a shape without pinning a clock or a random identifier.

**A tag names a value domain, not an encoding.**

`<room_id>` `<subject_id>` `<device_id>` `<event_id>` `<invite_id>` `<file_id>`
`<pipe_id>` `<op_id>` `<ts>` `<uint>` `<bool>` `<string>` `<pos>` `<capability>`
`<daemon_sg>` `<port>` `<object>` `<any>` `<version>` `<standing>` `<link_connected>`
`<link_reason>`

The second line is the domain additions #213's normalization minted, each named
here in the same change that uses it, as the rule above requires:

| Tag | Domain |
|---|---|
| `<pos>` | a room position — stricter than `<uint>` because positions are per-room and monotonic |
| `<capability>` | an invite capability string |
| `<daemon_sg>` | the daemon's storage generation, discovered at Layer 0 |
| `<port>` | a TCP port |
| `<object>` | any JSON object, shape unconstrained — used only where the record fixes no shape |
| `<any>` | any JSON value — used only for "the key is present, its value is unconstrained" |
| `<version>` | a daemon version string |
| `<standing>` | the `standing` bare enum |
| `<link_connected>` | a `link` variant in its `direct` or `relay` arm |
| `<link_reason>` | the `link.reason` bare enum |

- **`<hex64>` is not a tag.** It names an encoding shared by four distinct
  domains, so asserting it where `<device_id>` belongs would pass against a
  subject id. The corpus uses it 
  in exactly that ambiguous way today.
- `<u64>`, `<number>`, and `<int>` all collapse into `<uint>`.
- **`<variant>` and `<array>` are not tags.** `{"state": "<variant>"}` asserts
  that a discriminant exists without asserting which arms are legal — it asserts
  nothing at all. Every such site becomes an explicit token set via `member_of`.
- A new tag is minted only when a value's domain is not already named, and it
  must be added to this table in the same change.

## `requires`

A closed vocabulary of preconditions the harness must establish. The committed
corpus uses **133 flat tokens**, many of them synonyms (`daemon`,
`authority_daemon`, `member_daemon`, `fresh_daemon`, `disposable_daemon`), which
no harness can implement as a closed set.

The replacement is a `namespace:argument` form. Namespaces are closed at nine:

| Namespace | Arguments | Establishes |
|---|---|---|
| `subject` | `self`, `none`, `second`, `outsider` | Local cryptographic subjects |
| `daemon` | `self`, `second`, `fresh`, `restartable` | Daemon processes |
| `room` | `plain`, `live`, `quiescent`, `left`, `removed`, `foreign`, `with_history` | A room in a named state |
| `member` | `b`, `c`, `agent`, `non_agent` | Additional members of the current room |
| `link` | `up`, `down`, `relay`, `slow` | Transport conditions between daemons |
| `resource` | `tcp_service`, `large_file`, `shared_file`, `fetched_file` | Fixtures the case operates on |
| `observe` | `network`, `store`, `frames`, `timing`, `process` | Observation capabilities |
| `control` | `clock`, `limits`, `reconnect`, `concurrency` | Harness controls |
| `fault` | a taxonomy code, e.g. `room_index_unreadable` | Fault injection |

A bare `subject` is shorthand for `subject:self`; a bare `daemon` for
`daemon:self`. Nothing else may be bare.

`fault:` takes **a code from the taxonomy**, so a fault a fixture wants to inject
that names no code is a signal the taxonomy is incomplete — which is how
`room_index_unreadable` and its four siblings were found in the first place.

## Adding a case

1. Read the specification. If it does not state what you want to assert, the
   specification is the thing to fix — not the fixture.
2. Do not run anything to obtain an expected value.
3. Prefer a case that would fail against a plausible wrong implementation. The
   repository's own precedent is to verify this by deliberately breaking the
   implementation and confirming the case goes red.
4. Give it an `intent` that names the breaking change. If you cannot, the case is
   not worth running.
