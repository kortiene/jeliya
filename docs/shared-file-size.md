---
type: "Decision"
title: "Shared-file size policy for protocol v2"
description: "Decision record retaining 104,857,600 bytes as the normative protocol-v2 maximum shared-file size, the served preflight and distinctive over-limit error it requires, its provisional resource budgets, and the falsifiers that would force a policy change."
tags: ["clean-slate", "files", "protocol", "release", "security"]
timestamp: "2026-07-28T11:15:06Z"
status: "canonical"
implementation_status: "planned"
verification_status: "unverified"
release_status: "unreleased"
audience: ["contributors", "maintainers", "product"]
---

# Shared-file size policy for protocol v2

**Status: DECIDED 2026-07-28. Nothing in this record is built.** Protocol v2
retains **104,857,600 bytes (100 MiB)** as the maximum size of one shared
file. Chunked and resumable transfer is out of the first release and is
designed separately.

Read every statement below as a requirement on unwritten code. This record
satisfies #92 and feeds the protocol authority #161. It is a policy decision
about a number, not a description of a working system: the [Dioxus clean-slate
architecture](dioxus-architecture.md) that will enforce it is itself planned
and unbuilt, and the released `v0.6.x` line reaches the same number by a
different route through React, Flutter, and the C ABI.

The value is unchanged from what ships today. **Everything else in this record
changes.** Retaining the number is not the same as retaining the mechanism,
and most of this record exists because the mechanism around the number is not
currently good enough to make it true.

## Why this needed deciding at all

The size cap looks settled and is not. Four facts forced an explicit decision
rather than an inherited constant.

**The number was never ratified.** `MAX_SHARED_FILE_BYTES = 104_857_600` is
declared upstream at `crates/iroh-rooms-core/src/event/constants.rs:37` (pin
`a5d98b70d717f35d3ce60953a88e12e646f2e871`) with a doc comment citing "spec
IR-0202 §5.7 / OQ-1". Upstream's own spec calls it "a deliberate MVP policy
bound" whose "Exact value is OQ-1", and OQ-1 reads "Product sign-off needed".
Upstream's release-readiness checklist still lists the cap as "to confirm",
and its feature-complete audit records the matching decision D-4 as "never
resolved". The erratum that was meant to close it was never written at the
pinned revision. The number is an implementer's proposal that no product
decision ever accepted.

**It is in open conflict with the only product-stated figure.** PRD v0.3
§17.1 item 9 states "Files up to 25 MB can be shared and fetched". Upstream
tracks the gap explicitly as a divergence between a 100 MiB enforced cap and a
25 MB product metric. This record does not resolve that conflict in the PRD's
favour; it resolves it in the implementation's favour, deliberately, and
**that is the substantive product content of this decision.**

**The constant is not an external ceiling.** `iroh-rooms` is Jeliya's own
upstream under the same owner (`Cargo.toml:15` pins
`https://github.com/kortiene/iroh-room`), already repinned more than once. The
constant sits beside `SCHEMA_VERSION` and `WIRE_VERSION` in the exact module a
protocol revision would touch. Retaining 100 MiB is therefore a choice, not a
constraint. A larger value was available and was not taken.

**The number exists six times and cannot currently be changed safely.** It is
written independently at `crates/jeliya-core/src/supervisor.rs:109`,
`dart/jeliya_protocol/lib/src/daemon_http.dart:24`, and
`scripts/jeliya-agent.mjs:92`, and the literal string `100 MiB` is baked into
the English and French catalogs for both clients (`ui/src/l10n/en.ts:110`,
`ui/src/l10n/fr.ts:132`, `app/lib/src/l10n/arb/app_en.arb:283`,
`app/lib/src/l10n/arb/app_fr.arb:64`), with a translator note instructing that
the number be kept verbatim. Changing the limit today silently makes two
translations lie. This is the duplicated-contract problem the clean-slate
architecture exists to end, in miniature.

## Decision 1 — the normative maximum

Protocol v2 accepts a shared file of **at most 104,857,600 bytes**, compared
with `>`, so exactly 104,857,600 bytes is accepted and 104,857,601 is not.

The normative value is the integer byte count, typed `u64`. It is carried on
the wire as an **integer**, never as a formatted string. Binary units (MiB,
1024-based) are for human display only and MUST be rendered by the client from
the served integer.

**No client, catalog, or script may contain the number.** A localized string
describing the limit MUST interpolate the served value. This requirement is
the reason the six existing copies are listed above: they are the defect this
decision closes, and a v2 client that reintroduces one has not implemented
this record.

## Decision 2 — the limit is served, not assumed

v2 MUST publish the maximum in its handshake as
`limits.max_shared_file_bytes`, an integer. A client learns the limit before
attempting a share and MUST NOT assume a compiled-in default.

This is new. There is no capability or limits field in the v1 handshake today;
the Dart client preflights against its own hard-coded copy
(`dart/jeliya_protocol/lib/src/daemon_http.dart:62`), which is correct only
for as long as the two constants happen to agree.

Serving the limit is what makes the number changeable later without a
coordinated multi-client release — which matters precisely because this record
retains a value that later evidence may overturn.

## Decision 3 — a distinctive over-limit error

v2 MUST define a distinct error code for the over-limit case, carrying the
declared size and the enforced limit as separate integer fields.

Today every over-limit rejection is `invalid_params`
(`crates/jeliya-core/src/error.rs:48`), the same code as "path is a
directory", "cannot read path", and a malformed request. The 14-code taxonomy
at `crates/jeliya-core/src/error.rs:11-41` has no size variant, so a client
that wants to show a size-specific message must pattern-match the English
substring `share limit` — which `dart/jeliya_protocol/test/daemon_http_test.dart:254`
does today. A machine-recognizable code with structured fields replaces that.

**The receive side needs it more than the send side.** An over-limit blob on
the fetch path surfaces as `FetchOutcome::HashMismatch`, because the upstream
outcome enum has no size variant — which Jeliya renders as "integrity check
FAILED … refusing to save" (`crates/jeliya-core/src/supervisor.rs:2232-2241`).
A size refusal reported as a **integrity failure** is a false accusation
against an honest peer. v2 MUST preflight the signed `size_bytes` before
fetching, and MUST NOT report a size refusal as a hash mismatch.

## Decision 4 — enforcement points, earliest first

Rejection MUST happen before expensive or partially persistent work. v2 MUST
enforce at every layer below, and MUST NOT rely on any single one:

1. **Client preflight**, against the served limit, before any copy or upload.
2. **Daemon upload edge**, on declared length, before reading the body.
3. **Daemon upload edge**, on streamed bytes, aborting mid-stream — a declared
   length is an assertion by an untrusted caller, not a fact.
4. **Core authoring**, on file metadata, before hashing or blob import.
5. **Fetch preflight**, on the signed `size_bytes`, before contacting a
   provider.
6. **Fetch transfer**, mid-stream, refusing a peer that serves more than the
   declared size.

Point 5 does not exist today. Point 6 exists upstream but Jeliya does not use
it: `crates/jeliya-core/src/supervisor.rs:2222` calls the unsized `fetch_file`,
which substitutes the full ceiling, rather than the public `fetch_file_sized`,
which honours a smaller caller value.

## Decision 5 — the transfer budget must be resized

**This is the condition on which retaining 100 MiB depends.**

`FETCH_TIMEOUT` is 30 seconds per provider
(`crates/jeliya-core/src/supervisor.rs:124`), applied at
`crates/jeliya-core/src/supervisor.rs:2222`. Moving 104,857,600 bytes inside
30 seconds requires **~27.96 Mbit/s sustained**.

The only throughput this project has ever measured spans **0.1–8.6 Mbit/s**
across fifteen samples. 100 MiB exceeds the fastest sample by 3.2× and the
slowest by 274×. The strongest single datum is more direct than the
arithmetic: ten natural-path runs at **8 MiB** — one twelfth of the limit —
**all failed at the bulk-transfer stage** against a 30-second budget.

Those numbers must be read with their caveats, and this record does not
overstate them. They measure the bare iroh substrate, not Jeliya's transfer
path. They are noisy single samples on one operator's hardware. They time a
fresh stream from `t=0` with QUIC slow start inside the window, so they are
floors rather than sustained rates, and upstream labels its lowest figures
"256 KiB samples dominated by slow-start over a constrained mobile uplink" and
"forced worst-case", with a larger-sample re-measure still owed.

Even read as generously as the evidence permits, a 100 MiB file does not
transfer in 30 seconds on any link this project has observed. **A limit the
transport cannot reach is not a limit, it is a promise the product does not
keep.** v2 therefore MUST NOT retain both 100 MiB and a fixed 30-second
per-provider budget.

The budget is Jeliya's to set: upstream imposes no timeout of its own and
takes the value as a caller parameter, and the 30-second figure mirrors
upstream's CLI default, whose own comment says it was chosen to be large
enough for a transfer up to the 100 MiB cap. v2 MUST replace the fixed budget
with one that accounts for the declared size, and MUST surface transfer
progress and cancellation so that a long transfer is observable and
interruptible rather than silently pending. Sizing that budget is #161's to
specify and #198's to validate.

## Decision 6 — chunked and resumable transfer is out of the first release

The first-release limit stays bounded and whole-blob. Chunked, resumable, and
range-requested transfer is **not** first-release work and is designed
separately under #209.

This is not a deferral of the size decision. It is the reason the size
decision can be conservative: there is no resume today, so a failed transfer
restarts from zero, and a bounded whole-blob limit is the only honest shape
available for the first release. Nothing in this record makes the
first-release limit unbounded.

## Provisional budgets and their falsifiers

Every number below is a **provisional assumption derived by reading code, not
a measurement**. None has been observed at or near the limit. Each carries the
observation that would invalidate it.

| Resource | Provisional assumption at 100 MiB | Basis | Falsifier |
|---|---|---|---|
| Fetch memory | ~2× the file, ~200 MiB transient per in-flight fetch | The blob is collected into one buffer, then copied again by `to_vec()` at `crates/jeliya-core/src/supervisor.rs:2229` while the original is still live, before the atomic save | A measured peak materially above 2×, or any second concurrent fetch on a constrained device |
| Upload cost | ~3× the file — one full in-memory body, one staged disk copy, one blob-store copy with a second full read for the BLAKE3 recompute | `crates/jeliyad/src/serve.rs:553` buffers the body, `:566` stages it, then core imports it in copy mode | A measured peak materially above 3×, or concurrent uploads exhausting the daemon |
| Store disk | ~1× per shared file, retained indefinitely, plus ~1× transient staging | Blob store import is a copy; the staging file is removed on the normal path | Any store growth beyond one copy per distinct blob |
| Android memory | Unbounded by any recorded figure | v2 runs core in-process with no daemon to absorb buffers; `largeHeap` bounds the Java heap and core allocates natively, so it does not apply | Any OOM or low-memory kill during a near-limit transfer on a real device |
| Network time | Not reachable in 30 s at any observed rate; requires ~27.96 Mbit/s | Fifteen samples spanning 0.1–8.6 Mbit/s; 8 MiB runs already failed at 30 s | A size-aware budget that still fails at the limit on a healthy direct link |
| Cancellation | None exists on the request path | No RPC method cancels, no error variant reports it, and a client disconnect cannot abort an in-flight call | Any requirement that a user abandon a transfer before it completes |
| Cleanup | Staged uploads are reaped on both success and share failure, but not across process death | Removal precedes the outcome branch; shutdown ends in a process exit that runs no destructors, and stage names are unique per attempt, so residue is permanent | Any stranded full-size file observed after a restart |

**If any falsifier is observed, it MUST open an explicit policy-change issue.**
It MUST NOT be resolved by quietly editing the wire limit. That rule is the
main protection this record offers, because the number it retains is the one
with the least evidence behind it.

## Downstream verification

This decision selects a number and specifies behavior. It proves nothing. Each
issue below owns the evidence for its own surface.

| Issue | Owns |
|---|---|
| #161 | The normative v2 spec: the served `limits.max_shared_file_bytes` field, the distinctive over-limit error and its fields, the six enforcement points, the size-aware transfer budget, and hand-authored conformance fixtures for below-limit, exactly-at-limit, over-limit by one byte, and malformed size |
| #181 | Web enforcement: browser preflight against the served limit, streaming upload without buffering the file in wasm, the over-limit message rendered from the served integer, and cancellation |
| #192 | Android enforcement: SAF content-URI sizing before any copy, near-limit transfers on real hardware including a low-memory device, no-backup placement of staged and fetched files, and cancellation |
| #198 | Resource validation: measured peak memory for upload and fetch at the limit against the ~3× and ~2× assumptions, on desktop and Android; measured transfer time against the size-aware budget |
| #195 | Release evidence: below-limit, exactly-at-limit, over-limit, cancellation, low-disk, restart-during-transfer, direct and relayed, and hostile metadata, as executable required-behavior cases |

## Residual risks this record does not close

- **No quota of any kind exists.** There is no per-room, per-peer, or total
  store cap anywhere in `crates/`. The per-file limit is the *only* bound on
  how much disk a room member can cause a peer to consume; a member may share
  many at-limit files. Retaining the larger of the candidate limits makes this
  worse than the alternatives would have.
- **Storage exhaustion by a room member is absent from the threat model.**
  [Security and threat model](security-threat-model.md) has no row for it.
- **Staged uploads survive process death.** A share interrupted by shutdown
  strands a full-size file permanently, with no `Drop` guard and no startup
  sweep. The Dart client already gets the equivalent right with `try`/`finally`.
- **The PRD conflict is resolved but not closed.** The product document still
  says 25 MB. Amending it is outside this record's scope.

## What this record does not decide

- The size-aware transfer budget's formula or constants — #161 specifies it.
- The wire spelling of the handshake limits object or the error code — #161.
- Whether the store gains quotas or retention — unowned, and worth an issue.
- Any change to the upstream constant, which retaining the value makes
  unnecessary.
- Anything about v1. The released line keeps its current behavior; this record
  binds only new v2 events and transfers, and no legacy file is migrated.

## Implementation

Nothing here is implemented. The decision is normative on #161, which converts
it into the v2 specification and its conformance corpus; enforcement follows in
#181 and #192; validation in #198; release evidence in #195.
