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

## Status — not yet replayable

The specification is now complete: it states every request and reply shape, the
whole 60-code taxonomy, and the DSL below. **The fixtures have not yet been
normalized to that DSL**, and until they are, this corpus cannot be replayed and
must not be cited as evidence for any adapter.

| | |
|---|---|
| Cases | 335 |
| Cases conforming to the DSL below | **0** |
| Distinct step verbs in use | 178, against a closed set of 8 |
| Cases carrying at least one off-DSL step | 332 of 335 |
| Cases asserting a field or code the specification does not define | 51 |
| Blocked on upstream | 10 |

Normalization is tracked as **#213**. It is deliberately a separate change from
the specification work (#212): deciding whether a fixture is wrong requires the
schema it is wrong against, so the schemas had to land first.

**Ten cases are blocked on upstream work** (U1, U2, U3 in the spec). They
**fail**; they do not skip. A skipped case reads as coverage and quietly
becomes permanent. A failing case reads as work.

### What normalization must not do

The corpus's value rests entirely on the independence rule, and the fastest way
to normalize 335 fixtures would destroy it. A fixture rewritten by pattern
substitution is no longer a transcription of something independently known to be
right — it is a transcription of whatever the substitution produced.

Each case is therefore re-derived from the specification by hand, and a case
whose intent no longer names a breaking change it would catch is **deleted**
rather than mechanically translated.

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

A step has **exactly one verb**. The verb set is closed at eight.

| Verb | Value | Meaning |
|---|---|---|
| `call` | operation name | Invoke an operation on the current session |
| `http` | `{method, path, headers, body}` | A Layer 0 or `/api/session` request |
| `upgrade` | `{query, headers}` | A Layer 1 `/ws` upgrade attempt |
| `send` | raw frame value | Write bytes that may not be a valid frame |
| `await` | `{frame}` or `{push}` | Wait for a server-initiated frame |
| `control` | `{…}` | Drive the harness, not the daemon |
| `save` | `{var: path}` | Capture values into variables |
| `assert` | array of assertions | Everything else |

Any step may additionally carry:

| Key | Meaning |
|---|---|
| `in` | the request body, for `call` |
| `op_id` | the envelope `op_id`, for `call` — **never inside `in`** |
| `on` | which session this step runs on |
| `expect` | the reply matcher |
| `note` | prose for a human; a harness ignores it |

`note` is the only annotation key. The committed corpus also uses `why`,
`comment`, `intent_note`, and `meaning` for the same thing.

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

An `expect` on a `call` step replaces all thirteen of the corpus's `expect_*`
verbs: `expect_error`, `expect_subset`, `expect_frame`, `expect_status`,
`expect_body`, `expect_hello`, `expect_upgrade`, `expect_absent`,
`expect_one_of`, `expect_identical_to`, `expect_no_null`, `expect_all`, and
`expect_each_subset`.

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

`save` maps variable names to paths. A `$name` reference resolves to the captured
value anywhere a literal is legal. Variables are scoped to the case.

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
`<pipe_id>` `<op_id>` `<ts>` `<uint>` `<bool>` `<string>`

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
