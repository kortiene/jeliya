# Protocol v2 conformance corpus

Hand-authored, language-neutral fixtures for
[protocol v2](../../docs/protocol-v2.md). Intended to be replayed by an
independent harness against every adapter: the codec, the typed daemon, the
Rust adapters, and the agent cutover.

**Not yet replayable.** The specification is `draft` and does not state
per-operation wire schemas, so these fixtures encode a contract the document
does not yet contain. They also use several dialects for the same concepts —
`assert` appears as an array, as an object, and as a predicate string with
sibling operands — so a harness cannot interpret them consistently. Both are
tracked as **#212**. Treat the corpus as **recorded intent**, precise
about behaviour and not yet precise about shape.

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

## Status

`manifest.json` is the machine-checkable answer, and today it says:

| | |
|---|---|
| Cases | 332 |
| Operations satisfying the required kinds | **33 of 33** |
| Quarantined | 0 |
| Blocked on upstream | 10 |

Every operation has a success case and an error case using **its own most
specific code**. A shared code does not satisfy the rule, because
`subject_absent` asserted against `room.list` and against `fleet.list` is the
same assertion twice.

**Ten cases are blocked on upstream work** (U1, U2, U3 in the spec). They
**fail**; they do not skip. A skipped case reads as coverage and quietly
becomes permanent. A failing case reads as work.

### How the quarantine was cleared

An earlier round quarantined eleven cases for asserting payload contracts the
specification did not state — they had fallen back on v1 shapes (`points`,
`rooms_total`, a nullable `progress`). That was a finding about the
*specification*: an operation described only as "read the agent fleet
projection" cannot be conformance-tested, and cannot be implemented either.

The fix was to write the missing contracts, not to bless the v1 shapes. Twelve
superseded cases were **deleted** rather than un-quarantined, and replacements
were authored against the new contracts.

One decision remains open and is recorded in the specification: whether typed
status severity lives on the signed event or in the daemon projection. It is a
wire change, and the corpus must not freeze before it is settled.

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

## Case shape

```json
{
  "name": "snake_case_unique",
  "kind": "success|error|malformed|boundary|push|ordering|authorization|handshake",
  "operation": "room.create",
  "intent": "what this proves, and what breaking change it would catch",
  "requires": ["subject", "live_room"],
  "steps": [{ "call": "room.create", "in": {}, "expect": {} }],
  "blocked_on_upstream": null,
  "quarantined": null
}
```

Dynamic values are asserted as type tags (`<hex64>`, `<room_id>`, `<ts>`,
`<uint>`), never as literals, so a fixture pins a shape without pinning a clock
or a random identifier. This convention is inherited from the v1 corpus.

`intent` is required and is not decoration: a case whose intent cannot name the
breaking change it would catch is not worth running.

## Adding a case

1. Read the specification. If it does not state what you want to assert, the
   specification is the thing to fix — not the fixture.
2. Do not run anything to obtain an expected value.
3. Prefer a case that would fail against a plausible wrong implementation. The
   repository's own precedent is to verify this by deliberately breaking the
   implementation and confirming the case goes red.
