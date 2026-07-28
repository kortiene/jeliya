# Protocol v2 conformance corpus

Hand-authored, language-neutral fixtures for
[protocol v2](../../docs/protocol-v2.md). Replayed by an independent harness
against every adapter: the codec, the typed daemon, the Rust adapters, and the
agent cutover.

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

## Status — this corpus does not yet freeze

`manifest.json` is the machine-checkable answer, and today it says:

| | |
|---|---|
| Cases | 310 |
| Operations satisfying the required kinds | **21 of 33** |
| Quarantined | 11 |
| Blocked on upstream | 10 |

Three things are deliberately visible rather than hidden.

**Twelve operations do not yet have a distinctive error case.** The rule is one
success case plus one error case using *that operation's own most specific
code*; a shared code such as `subject_absent` does not satisfy it, because
`subject_absent` asserted against `room.list` and against `fleet.list` is the
same assertion twice. The specification now assigns a distinctive code to each
of those operations — the cases that use them still need writing.

**Eleven cases are quarantined.** Each asserts a payload contract the
specification does not state. They were caught by an independence audit, and
they are a finding about the *specification*, not about the fixtures: an
operation described only as "read the agent fleet projection" cannot be
conformance-tested, and cannot be implemented either. `fleet.list`,
`status.history`, `status.post`, and the pipe publish-target policy each need a
payload contract written before their cases can be trusted. A quarantined case
does not run and does not count as coverage.

**Ten cases are blocked on upstream work** (U1, U2, U3 in the spec). They
**fail**; they do not skip. A skipped case reads as coverage and quietly
becomes permanent. A failing case reads as work.

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
