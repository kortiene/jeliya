# Protocol v2 conformance corpus

Hand-authored, language-neutral fixtures for
[protocol v2](../../docs/protocol-v2.md). They define cases that independent
adapters can consume; the status matrix below states which execution slices
actually exist.

The fixture JSON is language-neutral data. The repository also contains a Node
structural validator and a partial live replay harness. Their existence is not
evidence that every fixture or adapter is executable.

## The independence rule

**These fixtures were authored from the specification, never from an
implementation.** No daemon was run to produce an expected value. Every one was
derived by reading `docs/protocol-v2.md` and reasoning about it.

That is not pedantry. A corpus generated from the code under test proves only
that the code agrees with itself — it cannot catch a case where the
implementation and the specification disagree, which is the only thing a
conformance corpus is for. #161 states it as an acceptance criterion.

The v1 surface was consulted **only** to know what to avoid transcribing.

## Status

The specification is canonical and the fixtures use the normative DSL. Shape
validation, implemented execution, and adapter applicability are different
claims:

| Slice | Status | What the claim means |
|---|---|---|
| Structural validation | Implemented for all 341 cases | `scripts/check-v2-corpus.mjs` parses every fixture and validates the closed DSL vocabulary, strengthened file-domain assertion/error semantics (including per-case `$variable` binding in `files.json`), and manifest ledgers. It does not establish every case's semantic correctness and is not protocol evidence. |
| Selected JSON-envelope/subject slice | Partial (14 CI-selected cases) | The Node harness runs this selected JSON-envelope and subject-lifecycle slice against `jeliyad`; this is neither corpus coverage nor file-stream evidence. It is not a smoke, E2E, or Dart execution claim. |
| Binary byte-stream executor | Unimplemented | No harness codec/runtime executes Binary OPEN/DATA/CREDIT/END/ABORT/ACK records, so file-stream cases are declarative, not live evidence. |
| Adapter-target executors | Unimplemented / declarative | Cases may name adapters to which they apply, but no executor proves an in-process-core or client-adapter obligation. A target mismatch is not a pass. |

| Computed corpus fact | Value |
|---|---:|
| Cases | 341 |
| Attributed to an operation | 237 |
| `operation: null` | 104 |
| Untargeted / in-process-core / client-adapter targeted | 335 / 1 / 5 |
| Distinct step verbs in use | 7 |
| Taxonomy codes / without a direct canonical operation case or verified transport representation | 64 / 9 (`forbidden_origin`, `pairing_code_invalid`, `protocol_unsupported`, `role_not_grantable`, `session_expired`, `storage_generation_mismatch`, `stream_aborted`, `unauthenticated`, `unknown_operation`) |
| Blocked on upstream | 14 (U1: 5, U2: 8, U3: 1) |
| Blocked on a settled record contradiction | 1 |

Per-file case totals are: `files.json` 43, `handshake.json` 69,
`invites.json` 37, `pipes.json` 43, `rooms.json` 65,
`subject-daemon.json` 24, and `timeline-streams.json` 60.

Corpus values are recomputed by the validator from every fixture JSON and
reconciled against `manifest.json`; they are not execution results. Distinctive
code coverage requires a literal, canonical `expect.err.code` on a direct
`call` whose operation equals the case's `operation`; notes, intents, setup
calls, nested generic objects, and `authoring_notes` never count. The general
codes-without-a-case ledger uses the same direct canonical operation-case rule,
with only the four verified close/status fixture-name exceptions for
`frame_too_large`, `idle_timeout`, `malformed_frame`, and `not_ready`. The selected live-slice count
comes from every explicit CI `--case` selector, each of which must resolve
exactly once; it is status metadata, not corpus coverage.

The files pass-3 re-transcription retired cases about an unknown declared size:
`declared_bytes` is a required `<uint>` and there are no optional request fields,
so that state is not expressible in v2 and collapsed onto existing cases.

The aggregate no-path case
(`no_file_operation_carries_a_filesystem_path_in_any_direction`) was retired
as inexecutable evidence. Its original transcription required
`remote_provider` — the precondition that would have established the second,
remotely provided file its `$fid2` named — and asserted over `all_frames`, a
scope the normative DSL does not carry. Normalization replaced
`remote_provider` with `link:up` and folded the assertions onto the final
frame, so the fetch step named a file no surviving precondition or save
established, the read step read the freshly shared `$fid` rather than a
fetched file, and the path assertions ran only against the `transfer.cancel`
reply. Its obligations live in the operation-specific share, list, fetch,
read, and cancel cases, which assert the forbidden path-bearing keys `absent`
on their actual reply roots. The validator now rejects an unbound `$name` in
`files.json` — including a documented precondition variable whose binding
precondition the case does not declare — closing this regression class apart
from the two U2-blocked fixtures named in the validator's exemption ledger
(see "Runner-provided variables").

Top-level `authoring_notes`, where retained in unchanged domains, are historical,
non-normative transcription notes. Validators never use them as taxonomy-code
or coverage evidence.

Normalization was tracked as **#213** and landed with the promotion of the
specification to canonical: refused codes were retired, the fixture bugs #212
catalogued were fixed, cases were retranscribed, uncovered codes were authored,
and the manifest was recomputed from the fixtures.

Blocked cases **fail**; they do not skip. A skipped case reads as coverage and
quietly becomes permanent. A failing case reads as work.

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
expresses while replacing the earlier ad-hoc verbs and `assert` dialects with
one language. Every construct below exists because some fixture needs it.

## The case object

```json
{
  "name": "room_list_reports_standing_and_capabilities_from_local_evidence",
  "kind": "success",
  "operation": "room.list",
  "intent": "Proves room.list answers from local evidence with zero network activity, catching any change that makes the room list depend on liveness.",
  "requires": ["subject", "room:live", "observe:network"],
  "targets": ["daemon"],
  "steps": [ … ],
  "blocked_on_upstream": "U1"
}
```

| Key | Required | Value |
|---|---|---|
| `name` | yes | `snake_case`, corpus-wide unique |
| `kind` | yes | one of the closed values below |
| `operation` | yes | one of the closed operation names, or `null` |
| `intent` | yes | prose naming the breaking change this case would catch |
| `requires` | yes | array of preconditions, closed vocabulary below |
| `targets` | no | non-empty unique array whose values are from `daemon`, `in_process_core`, `client_adapter` |
| `steps` | yes | non-empty array |
| `blocked_on_upstream` | no | `"U1"`, `"U2"`, or `"U3"` |
| `blocked_on_record` | no | Named settled record/corpus contradiction retained as an expected failure until the stale case is retired |

Omitting `targets` means every adapter to which the case is applicable. An
adapter executes cases whose `targets` contain it, plus applicable untargeted
cases. A target mismatch is an applicability decision only: it is not a pass or
skip and contributes no such claim to global coverage.

Both block forms **fail, never skip**. A passing blocked case is reported as a
surprise; a setup/runner error is still an error rather than an expected block.

`kind` is closed: `success`, `error`, `malformed`, `boundary`, `authorization`,
`handshake`, `push`, `ordering`.

**`operation: null` is legal and means the case is not about one operation** —
gate behaviour, envelope framing, and cross-room ordering are examples. The
manifest lists every such case by file, and its coverage table counts only
operation-attributed cases. A case may not use `null` merely because attributing
it is inconvenient.

`intent` is required and is not decoration: a case whose intent cannot name the
breaking change it would catch is not worth running.

## Steps

A step has **exactly one verb**. The table below is the closed verb set.

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
| `defer` | boolean `true`, for `call` only; send the request without awaiting its terminal reply |
| `note` | prose for a human; a harness ignores it |

`save` is an **auxiliary key, not a verb**. It always captures from the step it
sits on, so making it a verb would force every capture into a second step with
nothing to capture from.

### `defer` — keep a call outstanding

A deferred call has `"defer": true`, sends its request, and saves a request
handle without waiting for the terminal reply:

```json
{ "call": "file.fetch", "in": { "room_id": "$rid", "file_id": "$fid" },
  "op_id": "op-fetch", "defer": true,
  "save": { "fetch_request": "$request" } }
{ "await": { "reply": "$fetch_request" },
  "expect": { "ok": false, "err": { "code": "stream_aborted" } } }
```

`defer` is legal only on `call`, has no false form, and the deferred call does
not carry `expect`; its later `await {reply: "$handle"}` receives and matches the
terminal reply. The special save path `$request` captures the harness request
handle, not a protocol reply field.
`save: {"handle": "$request"}` is legal only on a deferred call. Every saved
handle has exactly one later terminal path on the same effective session: either
one `await {"reply": "$handle"}`, or one `control.disconnect` that names that
session and explicitly abandons its connection-local handles. A disconnect on a
different session does not terminate the handle. Duplicate awaits, an await on
the wrong session, and a live deferred handle at case end are invalid. This
defines the DSL contract; the current live harness does not implement it.

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
combining the two edges. `stream` and `defer` cannot share a call step.

It takes exactly one key, fixed by the operation: `send_bytes` for `file.share`,
`receive_bytes` for `file.read`. A `stream` on any other operation is invalid,
because no other operation streams. The value is a `<uint>`, a `$variable`, or a
computed node, so a boundary case can say "exactly the served limit" without
compiling the number in.

A fresh admitted daemon `file.share` or `file.read` requires its matching stream.
A terminal refusal before OPEN has no stream and records zero receiver-accepted
bytes. A faithful replay of a completed result also has no stream and opens no
second one. Validators may require stream presence only when admission is safely
decidable from the case's explicit `expect` result or replay shape; they must not
guess from prose or from operation name alone.

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

`note` is the only annotation key. The retired forms `why`, `comment`,
`intent_note`, and `meaning` are invalid. Domain files accept only `domain`,
`note`, `cases`, and optional `authoring_notes`; case objects accept only the
keys in the table above. `authoring_notes` are historical and non-normative and
never count as coverage or error-code evidence.

### `control` — driving the harness

`control` is discriminated by `do`; the table below is closed. It is the one
verb that does not touch the daemon's protocol surface, so leaving it open
would have let every harness invent its own dialect — which is what the committed
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
| `set_link_rate` | `between`, `bits_per_second` | Cap the harness link between exactly two session/provider labels; the rate is a positive value node |
| `set_provider_response_bytes` | `file_id`, `bytes` | Make the named provider serve the nonnegative byte count |
| `client_preflight` | `source_bytes`, `source_reports_size` | Exercise client preflight with a nonnegative byte-count value node and a boolean size-report flag |
| `client_render_limit` | `served_bytes` | Render a limit from a nonnegative served-byte value node |
| `client_render_file` | `declared_content_type`, `body_kind` | Exercise client rendering with two non-empty strings |
| `stop_daemon` | `daemon` | Terminate a daemon process |
| `start_daemon` | `daemon` | Start a previously stopped daemon — restart cases cannot be written without it; the pair expresses one restart, never a fresh daemon |
| `start_transfers` | `count`, `aggregate_bytes`, `op_id_prefix` | Begin a positive number of file transfers reserving a nonnegative aggregate byte count under a non-empty prefix |
| `cancel_transfers` | `op_id_prefix` | Cancel the harness-started file-transfer set under a non-empty prefix |
| `pause_link` | `between` | Suspend transport between two daemons |

Each file-domain form above is closed and includes `do`: exactly
`{do,file_id,bytes}`, `{do,source_bytes,source_reports_size}`,
`{do,served_bytes}`, `{do,declared_content_type,body_kind}`,
`{do,count,aggregate_bytes,op_id_prefix}`, or `{do,op_id_prefix}` respectively.
The two pre-existing handshake request-concurrency fixtures retain their exact
legacy `start_transfers` shape `{do,count}`; it drives requests rather than file
transfers. `set_link_rate` remains exactly `{do,between,bits_per_second}`. These
controls make fixtures declarative; they do not add a protocol or executor.

`inject_fault`'s `fault` is a **taxonomy code**, so a fault a case wants that
names no code is a signal the taxonomy is incomplete — that is how
`room_index_unreadable` and its four siblings were found. Two conditions are
deliberately not codes and are named directly: `backpressure` and
`subscription_lapse`, which are the two `gap.reason` arms a harness must be able
to force in order to test gap detection at all.

`set_limit` exists because several boundary cases need a limit small enough to
reach — `max_connections` and `max_subscriptions_per_connection` cannot be
exercised against production values in a test.

`set_link_rate` is harness control, not a protocol operation. Its object has
exactly `do`, `between`, and `bits_per_second`; `between` contains exactly two
distinct non-empty session/provider labels, and `bits_per_second` is a positive
`<uint>`, a `$variable`, or a computed node. It makes the size-aware deadline
cases declarative, but the U2 cases still fail until executable progress/rate
support exists.

### `on` — which session

`on` names a session established by `requires`, defaulting to `subject:self`'s
primary connection when omitted:

```json
{ "call": "room.create", "on": "subject:self",   "in": { "name": "Build" } }
{ "call": "room.timeline", "on": "subject:second", "in": { "…": "…" } }
{ "call": "room.list",   "on": "subject:self#2", "in": {} }
```

**Nothing else selects an actor.** `principal:self` and `principal:second`
explicitly name distinct authenticated session principals — distinct
`client_id`/session credentials — on one daemon and one cryptographic subject.
They are the labels principal-isolation cases must use. By contrast,
`subject:second` establishes another daemon/subject; it is not evidence of
same-daemon principal isolation. The other cases that need actor selection are
not marginal: the non-oracle property needs a non-member, reconnect cases need
two connections for one subject, and `#2` names a second connection for the same
principal, distinguishing per-connection from per-principal scope.

## `expect` — one reply matcher

`expect` is a single form discriminated by `ok`, exactly as the wire is:

```json
{ "expect": { "ok": true,  "out": { "room_id": "<room_id>", "live": true } } }
{ "expect": { "ok": false, "err": { "code": "room_not_available" } } }
```

In `files.json`, a literal error code also closes the canonical error matcher:
`invalid_argument` has exactly `code,field,reason`; `room_not_available`
`code,room_id`; `membership_ended` `code,room_id,standing`; `file_unknown` and
`file_not_fetched` `code,file_id`; `provider_unreachable`
`code,file_id,providers`; `transfer_unknown` `code,transfer_op_id`;
`declared_size_mismatch` `code,declared_bytes,observed_bytes`;
`transfer_stalled` `code,transferred_bytes,total`; `transfer_deadline_exceeded`
adds `budget_ms`; `stream_aborted` uses `code,transferred_bytes,total,reason`;
`file_too_large` uses `code,declared_bytes,limit_bytes,enforced_at`;
`resource_exhausted` uses `code,resource,limit`; `digest_mismatch` uses
`code,expected,observed`; `room_not_live` uses `code,room_id`;
`op_id_conflict` uses `code,op_id`; and `subject_absent` and
`file_index_unreadable` are code-only. The non-empty
`provider_unreachable.providers` array contains rows with exactly
`subject_id,device_id,link`.
Every `files.json` error matcher carries a literal code and uses one of these
schemas; they never apply to prose notes.

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

`expect` replaces the earlier `expect_*` forms. They divide into reply
matchers, assertions, and frame/process observations; the division is the point:

| Corpus verbs | Become |
|---|---|
| `expect_error`, `expect_subset`, `expect_ok`, `expect_envelope`, `expect_body`, `expect_body_shape`, `expect_final_body`, `expect_hello`, `expect_hello_subset`, `expect_status`, `expect_content_type`, `expect_upgrade`, `expect_upgrade_error` | `expect`, directly |
| `expect_absent`, `expect_no_null`, `expect_identical_to`, `expect_one_of`, `expect_any_of`, `expect_all`, `expect_each_subset`, `expect_every_element`, `expect_across`, `expect_at_least_one_error`, `expect_all_replies`, `expect_hello_assert`, `expect_rendering` | an `assert` predicate — they are assertions wearing an `expect_` prefix |
| `expect_frame`, `expect_push`, `expect_no_push`, `expect_frame_order`, `expect_each_frame`, `expect_close`, `expect_no_further_frames_of_type`, `expect_transport`, `expect_process`, `expect_connect_failure`, `expect_timing_indistinguishable`, `expect_reply_for_id`, `expect_reply_for_id_2`, `expect_error_for_id`, `expect_no_reply_for_id` | `await` or an `observe` assertion — they are about frames and processes, not about one reply |

The retired reply-by-id forms deserve their own note: they existed because
**replies may arrive out of order**, which the record makes normative.
A harness that could only match replies in request order would silently pass an
implementation that violated it. `await {reply: "$id"}` is how a case names the
reply it means.

## `assert` — one assertion form, two families

`assert` is **always an array of objects**. It is never a bare string or a
single object.

### Value assertions

```json
{ "path": "out.rooms[*].role", "op": "member_of", "value": ["authority", "member"] }
```

`path` is a dotted path rooted at `out`, `err`, `frame`, or a `$variable`.
`[*]` is a wildcard over an array: the assertion must hold for **every** element.
This is what lets one predicate replace the corpus's `every_row_has_fields`,
`every_row_has_non_null`, `every_push_has_non_null`, `all_eq`, `every_has_key`,
and `every_row_has_value`.

`op` is closed:

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

For `files.json`, value assertions are additionally shape-checked so they cannot
be empty claims. `present`, `absent`, `unique`, `increasing`, `non_decreasing`,
`contiguous`, and `no_nulls` take no `value`. `eq`, `ne`, numeric comparisons,
`member_of`, `type`, `exact_keys`, `len`, `byte_len`, and `eq_except` require the
value forms in the table; `ne` cannot take an array. A `type` value is one of the
bare names in the type-tag vocabulary (for example `uint`, not `<uint>`).
Assertion objects admit only `path`, `op`, `value`, and the optional historical
`note` used by current file fixtures. These stronger file-domain checks are not
a claim that every fixture's intended semantics have been proved.

`no_nulls` deserves its own predicate rather than being a convention because
[the specification's no-null rule](../../docs/protocol-v2.md#bounded-parsing) is
normative for every frame, and a corpus that cannot assert it cannot test it.

These predicates replace the corpus's `eq`, `equals`, `equal_modulo`, `all_eq`,
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

`observe` is closed. Each row states the keys it takes, because an observation
with no slot for its argument cannot be written down:

| `observe` | Additional keys | Holds when |
|---|---|---|
| `no_network_activity` | `scope` | No packet left the host during `scope` |
| `no_durable_mutation` | `scope` | No byte of the data dir changed during `scope` |
| `no_event_authored` | `scope` | No room event was written during `scope` |
| `bytes_streamed` | `value`, optional `call` | Receiver-accepted payload bytes satisfy a nested comparison; `call` is `step:<n>` |
| `connection_open` | `on` | That session is still open |
| `close_code` | `value`, `on` | That connection closed with this code |
| `push_count` | `value`, `room_id` | Pushes received for a room satisfies a comparison |
| `no_push` | either `room_id`, `scope`; or `on`, `match`, `scope` | No room-scoped push arrived, or no push matching a non-empty object arrived on the named principal |
| `timing_indistinguishable` | `between` | Two named steps are not separable by latency |
| `process_exited` | `value` | The daemon process exited with this status |

The closed sets above are also enforced by the validator; their sizes are not
coverage claims.

`scope` is `step`, `case`, or `step:<n>..step:<m>`. `no_push` is exactly either
the legacy room form `{observe,room_id,scope}` or the principal matcher form
`{observe,on,match,scope}`. In the latter, `on` is non-empty and `match` is a
non-empty object; `room_id` is neither required nor allowed.

`bytes_streamed.value` is the nested comparison object. Its metric is bytes the
receiver accepted into its bounded sink, never bytes merely generated, read, or
queued to a socket. Optional `call: "step:<n>"` selects the call whose counter is
observed; omission selects the most recent call on the same session. A terminal
pre-OPEN refusal records zero accepted bytes, and a completed faithful replay
has no stream and therefore also records zero.

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

`save` maps variable names to string paths and rides on the step that produces
them. A `$name` reference resolves to the captured value anywhere a literal is
legal. Variables are scoped to the case. Ordinary paths root at `out`, `err`,
`frame`, or `$` for the whole step value; `$request` is the deferred-call handle
capture. Retired `$result` and `$error` paths are invalid: use `out` and `err`.

This replaces `save`, `save_out`, and `save_error`, which differed only in which
root they read from — now expressed as the path's root.

### Runner-provided variables

`save` is the only in-case binder. Beyond it, the replay runner pre-seeds a
small documented set before step 1 — `$op_id_new`, `$op_id_fixed`, `$limits`,
`$daemon`, `$daemon_sg` — and the preconditions a case **declares** in
`requires` bind the fixture identifiers the case operates on: room, member,
or file-resource setup binds the case's `$rid` and `$self_sid` (a file exists
only in a room, so a file resource establishes the room too; `room:left`
additionally binds `$rid_left`); `room:foreign` binds `$foreign_rid` and
`$foreign_fid`; `member:b`/`member:c` bind `$member_b_sid`/`$member_c_sid`;
`subject:second` binds `$sb` and `subject:outsider` `$sc`;
`resource:tcp_service` binds `$svc_port` and `$svc_port_v6`; and the file
resource preconditions (`resource:shared_file`, `resource:fetched_file`,
`resource:large_file`) bind `$fid`, with the large-file companions
`$fid_unsized` and `$fid_one_byte` bound by `resource:large_file`. Like the
deferred-call contract, this states the DSL contract; the current live
harness pre-seeds the unconditional set and implements the
`room:plain`/`live`/`quiescent`/`with_history`/`left`, single-letter member
(`member:b`/`member:c`), second-subject/outsider, and tcp-service bindings,
while `room:removed`, the agent-member variants, and the file-resource and
foreign-room bindings await their executor.

In `files.json` the validator enforces the contract: every `$name` a step
reads must be captured by a `save` on an **earlier** step of the same case,
or be documented above **with its binding precondition declared** — a
documented name alone is never evidence, because the historical failure was
exactly a `requires` rewrite that dropped a binding while the reference
survived. A whole-string `$`-prefixed value that is not a well-formed
reference (`$fid-2`, `$fid[0]` — shapes the harness cannot resolve) is
invalid outright, and a `$`-rooted assertion or save path must be the bare
variable name followed by well-formed dot segments, each optionally carrying
`[*]` (`$rid[0].secret`, `$rid..secret`, and `$rid.` all resolve against
nothing, so an `absent` on them would silently pass). `http`/`upgrade`
header values and the `http` path are scanned for embedded references, since
the harness substitutes `$name` mid-string there (`Bearer $token`); `http`
bodies and `upgrade` query values resolve whole, so only whole-string
references count there. A `save` on a `send` or `control` step is invalid —
the runner applies captures only where a step produces a result. One
reply-position builtin exists: `await {reply: "$id"}` names the connection's
most recent request id and is not a variable. `$request` is likewise
positional — legal only as a deferred call's save source, and an ordinary,
never-bound name anywhere else. A dotted tail on a documented binding other
than `$limits`/`$daemon` is invalid: those bindings are scalar identifiers,
so the tail resolves against nothing. Binding validation proves the capture
statement exists and gates which names a step may read; whether a save's
*source path* resolves at replay is executor behavior — the corpus-wide
bracket-index notation (`out.files[0].file_id`) predates this contract and
is settled with the executor work, not here. An unknown reference is invalid rather than an
interestingly-named literal: the harness resolves an unbound `$name` to its
literal string form, which satisfies subset matching and quietly turns the
step into no evidence at all. Two pre-existing U2-blocked fixtures
(`a_transfer_with_no_forward_progress_fails_with_transfer_stalled` and
`fetch_completed_result_replays_without_a_second_transfer`) read the `$fid`
family without declaring a file resource precondition; they are exempted by
name in the validator's ledger until their `requires` are repaired under
#233. The legacy domains still carry pre-normalization placeholder
conventions (`$op1`-style tokens), so enforcement extends to them only with
their own re-transcription pass.

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
| `{"$transfer_budget_ms": ["$total", "$allowance", "$floor_bps"]}` | `allowance + ceil(total * 8 * 1000 / floor_bps)`, the record's size-aware transfer budget |
| `{"$unknown": "<room_id>"}` | A well-formed value of that domain naming nothing that exists |

Without these, **every case that probes a served limit would have to hard-code
the limit**, which is precisely the failure the record's served-limits object
exists to prevent.

`{"$unknown": …}` is separate from a literal because the non-oracle cases need a
value that is syntactically valid and semantically absent, and a fixture cannot
know a real identifier that does not exist.

## Type tags

A dynamic value is asserted as a **type tag** rather than a literal, so a fixture
pins a shape without pinning a clock or a random identifier.

**A tag names a value domain, not an encoding.**

`<room_id>` `<subject_id>` `<device_id>` `<event_id>` `<invite_id>` `<file_id>`
`<pipe_id>` `<op_id>` `<request_id>` `<ts>` `<uint>` `<bool>` `<string>` `<pos>`
`<capability>` `<daemon_sg>` `<port>` `<object>` `<any>` `<version>` `<standing>` `<link_connected>`
`<link_reason>`

The table names the additional domains beyond the obvious identifier and scalar
tags; each new tag is added here in the same change that uses it:

| Tag | Domain |
|---|---|
| `<pos>` | a room position — stricter than `<uint>` because positions are per-room and monotonic |
| `<request_id>` | the request envelope's exact integer correlation domain |
| `<capability>` | an invite capability string |
| `<daemon_sg>` | the daemon's storage generation, discovered at Layer 0 |
| `<port>` | a TCP port |
| `<object>` | any JSON object, shape unconstrained — used only where the record fixes no shape |
| `<any>` | any JSON value — used only for "the key is present, its value is unconstrained" |
| `<version>` | a daemon version string |
| `<standing>` | the `standing` bare enum |
| `<link_connected>` | a `link` variant in its `direct` or `relay` arm |
| `<link_reason>` | the `link.reason` bare enum |

- **`<hex64>` is not a tag.** It names an encoding shared by distinct domains,
  so asserting it where `<device_id>` belongs could pass against a subject id.
- `<u64>`, `<number>`, and `<int>` all collapse into `<uint>`.
- **`<variant>` and `<array>` are not tags.** `{"state": "<variant>"}` asserts
  that a discriminant exists without asserting which arms are legal — it asserts
  nothing at all. Every such site becomes an explicit token set via `member_of`.
- A new tag is minted only when a value's domain is not already named, and it
  must be added to this table in the same change.

## `requires`

A closed vocabulary of preconditions the harness must establish. Preconditions
use a `namespace:argument` form rather than adapter-specific synonyms. The
namespace table is closed:

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
