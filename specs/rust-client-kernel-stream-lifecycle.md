# Spec — Kernel stream lifecycle hooks: OPEN/DATA/CREDIT/END/ABORT, credit, stall, stream deadline (#269)

- **Issue:** kortiene/jeliya#269 — `[Rust][Client]: Implement the kernel stream lifecycle hooks (OPEN/DATA/CREDIT/END/ABORT, credit, stall and stream deadlines)`.
- **Program:** #156 (Dioxus clean-slate). **Milestone:** M2 (client runtime and platform adapters).
- **Carried over from:** `specs/rust-client-bounded-kernel.md` **§K16**, unchanged in scope. #168 shipped the request/reply kernel in full but **not** the stream hooks §K16 described; that spec and `crates/jeliya-client/src/stream.rs` now name **this** issue as the owner. Where §K16 and this document disagree, this document is the finer record for the stream layer; where either disagrees with `docs/protocol-v2.md` or `docs/dioxus-architecture.md`, those records are authoritative and the disagreement is a bug to be called out in the PR.
- **Records/derives its decision from:** `docs/protocol-v2.md` §*Byte-stream framing* (credit/deadlines/END/ABORT/disconnect, the two success sequences) and §*Served limits* (`transfer_connect_allowance_ms`, `transfer_floor_bits_per_second`, `transfer_stall_ms`); `docs/dioxus-architecture.md` §"Decision 4 — one seam, four adapters, one platform boundary" (the kernel paragraph).
- **Depends on (all landed):** #167 (`crates/jeliya-client` seam + `StreamCall` surface), #168 (`crates/jeliya-client/src/kernel/**` request/reply kernel), #164 (`crates/jeliya-codec` + its byte-stream record types from #233), and the protocol authority for framing (#233/#242/#243).
- **Blocks / is the entry point for:** stream operations in #171 `WsWeb`, #172 `WsNative`, #173 `DirectClient`, and the #175 four-adapter parity suite. Until this lands, `call_stream` enters the generic request path and cannot transfer bytes through the kernel.
- **Owner role:** core maintainer (the kernel must not depend on a specific transport; backend erasure and the framing bytes both stay behind the driver boundary).
- **Constraint:** this is a **planning/spec document only**. No production code is written by the planning phase; the orchestrator performs all git/gh/PR work.

> **The one-line scope fence.** #269 wires the **client's** stream *control plane* — the deadline, the stall timer, the credit ledger, and the `OPEN/DATA/CREDIT/END/ABORT` state machine — as a thin layer over the same sans-IO core that already drives request/reply, and grows the kernel's transport seam with a binary-record concept. It **re-implements no framing**: the byte layout of a record (the `JBS2` header, offset arithmetic, per-kind field rules, the size-precedence ladder) stays owned by `crates/jeliya-codec/src/byte_stream.rs` and the daemon executor (#233/#242/#243), consulted at the driver boundary exactly as `WireFrame` bytes are today.

---

## 1. Outcome

Give the kernel the stream lifecycle it advertises so that a kernel-backed streaming call can actually transfer bytes under the protocol's bounds:

1. **A real client lifecycle.** `call_stream::<FileShare>` (the client is the **producer**) and `call_stream::<FileRead>` (the client is the **receiver**) drive the two legal duplex sequences — request → OPEN → CREDIT → DATA\* / CREDIT\* → END → terminal Text reply, in the correct directions — through the kernel against the deterministic in-memory transport.
2. **Credit-bounded outbound bytes.** The producer never emits DATA past the daemon's cumulative `send_through`; the unaccepted window and the read-ahead/quarantine buffer are **byte-bounded**, so memory is independent of file size.
3. **A per-stream absolute deadline.** `transfer_connect_allowance + ceil(total·8 / floor_bits_per_second)` (protocol §deadlines), armed at OPEN and *replacing* the request/reply base deadline, produces an honest typed failure — never a hang.
4. **A stall timer.** A stream that stops making *accepted* progress for `transfer_stall` fails honestly instead of hanging.
5. **Honest teardown.** ABORT/ACK (ACK is **ABORT-only**, never on the success path), connection loss, cancellation, and total stop each settle the `StreamCall` terminal exactly once and leave **no unbounded task, map, or timer** behind (AC-7).
6. **No framing duplication.** Every byte-layout rule stays doc-pointed at #233/#242/#243 and executed through `jeliya-codec` at the driver boundary.

The deterministic in-memory driver (the `test-transport` feature's `KernelController`) is extended to script the stream side — deliver OPEN/CREDIT/DATA/END/ABORT/ACK, drive a deterministic byte source and a receiver-accepted sink, and observe outbound records — so every stream fault is a reproducible sequence of `step` calls, identical on wasm and native, and becomes the reference the four adapters are diffed against under #175.

## 2. What this issue is, and what it is not

The client stream path spans three ownership layers; #269 owns exactly one of them.

| Concern | Owner | Where |
|---|---|---|
| **Byte layout of a record** — `JBS2` 48-byte header, magic, reserved-zero, kind byte, `(id, stream_id)`, offset/value fields, DATA payload bound, checked non-wrapping offset arithmetic, malformed-record → `4007`/`malformed_frame` | **codec / #233** (not #269) | `crates/jeliya-codec/src/byte_stream.rs` (`StreamRecord`, `StreamRecordKind`, `decode_stream_*`, `encode_stream_record`, `max_stream_data_bytes`) |
| **Daemon-side executor** — OPEN authoring, size-precedence ladder, staging/import, the ACTIVE→FINALIZING sequencer, `transfer.cancel` registry | **#233/#242/#243** (not #269) | `crates/jeliyad/src/{file_share,file_read,transfer,serve}.rs` |
| **Client control plane** — the `OPEN/DATA/CREDIT/END/ABORT` *state* for the two client-driven ops, client credit accounting, the per-stream deadline + stall timers, honest teardown, the transport-seam binary concept | **#269 (this issue)** | `crates/jeliya-client/src/kernel/**` + `crates/jeliya-client/src/stream.rs` body |

**Consequence for scope.** #269 does **not** touch the public seam surface (`ClientHandle`, `CallError`, `State`, `ClientEvent`, `Dedup`, `StreamCall`, the mock). It rewires the *body* of `ClientHandle::call_stream` / `StreamCall` to drive the kernel, extends internal `kernel::*` machinery, and adds a small public `StreamLimits` config. It writes **none** of the three real transports and re-implements **no** framing.

**Not in scope:** OPEN authoring / size precedence / staging (daemon, #233); `transfer.cancel` as anything other than the ordinary first-class request it already is (a stream *hook* is a local abort, never a remote-cancel side effect — §S9); real `PlatformServices` file sources/sinks (#174, injected by the adapters); resumable / chunked transfer (#209); concrete sockets (#171/#172/#173).

## 3. Owning crate and layout

The stream layer lives **inside** `crates/jeliya-client`, next to the request/reply kernel it extends (it reuses the same `Core`, `Ledger`, `Admission`, timers, generation fence, `EventBus`, and `RawJson`). A sibling crate would force those into a public boundary and duplicate the erasure — the same reasoning §3 of the #168 spec gives.

```
crates/jeliya-client/
  src/
    lib.rs            # re-export StreamLimits; extend KernelConfig
    stream.rs         # (surface unchanged) rewire call_stream/StreamCall body to drive the kernel
    kernel/
      mod.rs          # KernelConfig gains StreamLimits; KernelBackend + KernelController grow the stream/media driver
      core.rs         # Input/Action grow stream + media variants; delegate to streaming::StreamTable
      streaming.rs    # NEW — the per-stream sub-core: OPEN/DATA/CREDIT/END/ABORT state, credit ledger,
                      #        deadline + stall arming, honest teardown; byte-free (offsets only)
      transport.rs    # NEW variants — outbound StreamRecordIntent, inbound StreamRecordMeta, the media seam
      replay.rs       # stream ops forced to ReplayPolicy::Never (the file.share/file.read defect, §S8)
      timing.rs       # (unchanged) Tick/TickDelta/TimerId already suffice
      diag.rs         # redaction extended to stream records (§S12)
  tests/
    kernel_stream.rs  # NEW — the OPEN→DATA/CREDIT→END/ABORT lifecycle happy-path suite (both directions)
    kernel_fault.rs   # EXTEND — stream deadline, stall, credit overshoot refusal, disconnect, cancel, stop, churn
    boundaries.rs     # EXTEND — the kernel source scan already covers streaming.rs; assert zero new runtime deps
```

**Boundary invariants preserved (asserted by `tests/boundaries.rs`):**

- **No new runtime dependency.** The sans-IO core stays byte-free (§S2), so `streaming.rs` needs no codec, no buffer crate, nothing. The *driver* (adapters, and the test controller) owns the `jeliya-codec` conversion, exactly as it owns `WireFrame`↔bytes today. The library dependency-tree scan still shows no Iroh/WebSocket/`tokio`/Dioxus.
- **No wall clock, RNG, or spawn in the library.** The stream deadline and stall timers are `Tick`-based `Action::ArmTimer`/`CancelTimer`, driven by the same injected clock. `boundaries.rs`'s existing `src/kernel/**` scan (no `std::time`/`Instant::now`/`SystemTime`/`getrandom`/`rand::`/`tokio`) covers the new module unchanged.
- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]` continue to hold; every new public item is documented to `jeliya-api` density.

## 4. Architecture — a companion sub-core over a byte-free driver seam

The stream layer is a **companion state machine** to the request/reply core, keyed by the existing `CallId`. A stream call begins life as an ordinary dispatch: the core admits it, sends the Text request, and marks it in the ledger. The one difference is that a stream op does **not** expect an immediate Text reply — it expects an OPEN binary record first, after which a per-`CallId` `StreamEntry` drives the record exchange until the terminal Text reply settles the call.

```
        ┌───────────────────────── crates/jeliya-client ──────────────────────────┐
UI ──▶ ClientHandle::call_stream::<FileShare|FileRead> ──▶ StreamCall (surface unchanged)
                                   │  (eager dispatch of the Text request)
                          ┌────────┴─────────┐
                          │   kernel::core    │  request/reply state (unchanged) +
                          │   (pure, sync)    │  delegates stream inputs to ↓
                          └────────┬─────────┘
                          ┌────────┴─────────┐
                          │ kernel::streaming │  SANS-IO, BYTE-FREE: OPEN admission, credit ledger
                          │  StreamTable      │  (send_through/accepted_through/high-water, OFFSETS ONLY),
                          │                   │  deadline + stall timers, END-after-ack, ABORT/ACK, teardown
                          └────────┬─────────┘
                                   │  Action (SendRecord intent, ProduceData/GrantCredit window,
                                   │          ArmTimer, Settle, …)   — all offsets, never bytes
                          ┌────────┴─────────┐
                          │  Transport seam   │  #269 extends: binary records + a media (source/sink) seam
                          │  + media seam     │  the DRIVER frames via jeliya-codec and moves the bytes
                          └────────┬─────────┘
             ┌─────────────────────┼─────────────────────────┐
   (this issue)                 (#171)         (#172)          (#173)
   deterministic in-memory    WsWeb          WsNative        DirectClient
   driver + KernelController  browser WS     native WS       in-proc core
   (i mod 251 source,         + PlatformServices file source/sink (#174)
    receiver-accepted sink)
```

**The load-bearing decision (S2):** the sans-IO core never holds payload bytes. It tracks *offsets and lengths*. For a producer it decides "you may send DATA up to offset `send_through`, in a window of at most `stream_window_bytes`"; the driver reads exactly that many bytes from its source, frames them into ≤64 KiB DATA records via `jeliya-codec`, sends them, and reports how far it got. For a receiver the core grants credit up to its bounded quarantine; the driver receives DATA, writes to its sink, and reports the accepted high-water. Bytes live only in the driver — which already owns the transport and (in production) `PlatformServices`. This keeps the core deterministic, dependency-free, byte-bounded by construction, and honours "MUST NOT read or enqueue the whole file while waiting for socket capacity."

## 5. Design decisions

### S1 — One core, a companion `StreamTable` (thin layer, no re-implementation)

`streaming.rs` adds a `StreamTable` — a bounded map `CallId -> StreamEntry` — that `core.rs` owns alongside its `Ledger`/`Admission`. A dispatch whose `op` is a stream op (`"file.share"` or `"file.read"`, recognized by path exactly as `ReplayPolicy::derive` recognizes ops) is admitted and sent through the *unchanged* request/reply path, but its ledger entry is flagged `stream: true` so that, instead of settling on the first Text reply, an OPEN record for its `(id, stream_id)` transitions it into the `StreamTable`. Everything the request/reply core already guarantees — bounded admission, correlation ids, generation fencing, exactly-once settlement, total stop — is reused verbatim; the stream layer adds only the record exchange and its two timers. This is the "thin layer over the same sans-IO core" §K16 requires.

`StreamEntry` (byte-free) holds: `role: {Producer, Receiver}` (derived from op), `stream_id` (from OPEN), `total` (from OPEN), `phase: {Requesting, Active, Finalizing, Aborting, Retired}`, the credit ledger `{ send_through, accepted_through, high_water, sent_offset, source_ended }`, `deadline_timer`, `stall_timer`, and `last_progress_at`. No file name, digest, or payload byte ever enters it (§S12).

### S2 — The sans-IO core stays byte-free; the driver owns all byte movement (AC-7, boundary)

Restated as a rule the reviewer can check: **no `Vec<u8>`/payload field appears in `kernel::streaming` or the core.** Outbound DATA is an offset+length *grant* the driver fulfils; inbound DATA is offset+length *metadata* the driver has already routed to its sink buffer. The framing (`encode_stream_record` / `decode_stream_record`) happens in the driver, mirroring how the driver converts `WireFrame`↔codec bytes for text today (`transport.rs` doc: "converting between the seam's erased JSON text and the codec byte form at the driver boundary"). Consequences: the core adds no dependency; `boundaries.rs`'s dependency-tree scan and its `src/kernel/**` token scan both pass unchanged; and the byte-bounded window is enforced by the core's grant arithmetic, not by trusting a buffer size.

### S3 — Transport seam extension: binary records + a media seam (issue's named deliverable)

`transport.rs` grows three things, all `pub(crate)` to the kernel (adapters consume them in #171/#172/#173):

- **Outbound `StreamRecordIntent`** — the client-sendable kinds only: `Credit { id, stream_id, accepted_through, send_through }`, `End { id, stream_id, offset }`, `Abort { id, stream_id, high_water, reason }`, `Ack { id, stream_id, high_water }`. The client never sends OPEN (daemon-only) and never *frames* DATA itself — see the media seam. A `Debug` impl renders kind + ids + offsets only, never a reason string that could carry a name (§S12).
- **Inbound `StreamRecordMeta`** — every kind the driver can decode and hand up, tagged with its arrival generation: `Open { id, stream_id, total }`, `Credit { … }`, `Data { id, stream_id, offset, len }` (bytes already delivered to the driver's sink buffer; the core sees only offset+len), `End { id, stream_id, offset }`, `Abort { … }`, `Ack { … }`. It joins the existing `Inbound` enum as `Inbound::Record { generation, record }`. A codec-undecodable or unbindable binary message stays `Inbound::Malformed` (dropped, strands nothing) exactly as today; a *bound-but-request-local* malformed record is delivered as `StreamRecordMeta` and the core aborts only that stream (§S10), never the connection — the connection-fatal vs stream-local distinction is the codec/#233 rule, surfaced through this tagging, not re-decided here.
- **The media seam** — the producer's source and the receiver's sink, owned by the driver. The core drives them through:
  - `Action::ProduceData { call_id, up_to }` — "you may send DATA covering up to `up_to` more bytes"; `up_to` is already credit- and window-bounded by the core. The driver reads ≤ `up_to` bytes from the source, frames them into ≤64 KiB DATA records, sends them, and reports `Input::Produced { call_id, sent_through }` (and, at source exhaustion, `Input::SourceEnd { call_id, total }`, or `Input::SourceFailed { call_id }`).
  - `Action::WriteSink { call_id, offset, len }` — "deliver this accepted DATA range to your sink"; the driver writes and reports `Input::SinkAccepted { call_id, through }` (or `Input::SinkFailed { call_id }`).

  The exact enumeration is an implementation detail to be kept minimal; what is normative is that (a) the core issues *bounds*, the driver moves *bytes*, and (b) the media seam is the driver's, so `PlatformServices` (#174) plugs in at the adapter, not the kernel.

The `Driver` trait gains no new required method for #269 beyond the record/media plumbing; `DirectClient` (#173), which never dials, supplies an always-ready media seam.

### S4 — The two lifecycles, as state over the sub-core (AC-1)

Both success sequences from `docs/protocol-v2.md` §*Binding and operation lifecycle*, expressed as `StreamEntry.phase` transitions (`<` daemon→client, `>` client→daemon):

**`file.share` (upload; client = Producer).** `> Text request` → `< OPEN` (→ `Active`, arm deadline+stall, §S6/S7) `< CREDIT` → `> DATA*` (bounded by credit, §S5) `< CREDIT*` → `> END` (only once `source_ended && accepted_through == sent_total`) → `< Text reply` (→ settle terminal `FileShareOut`). The daemon moves to FINALIZING on receipt of END; the client simply awaits the one terminal reply.

**`file.read` (download; client = Receiver).** `> Text request` → `< OPEN` (→ `Active`, arm timers) `> CREDIT` (bounded by quarantine ≤ `total`) → `< DATA*` `> CREDIT*` (advanced only after `SinkAccepted`) → `< END` (validate `offset == total`, all bytes sink-accepted) → `< Text reply` (→ settle terminal `FileReadOut`).

Rules the state machine enforces (client-side only; the daemon owns its half):

- **OPEN is admission, not a reply.** Exactly one OPEN per stream, accepted only while `Requesting`; a second OPEN, or an OPEN for an unknown/settled call, is a bound-record fault → local ABORT (§S10). The client adopts OPEN's `stream_id` and `total` as authoritative.
- **END is a terminal commitment.** For the producer, END is emitted only after every sent byte is CREDIT-acknowledged (`accepted_through == sent_total`) and the source has ended — never on "declared bytes reached" (receiving declared bytes is not EOF). For the receiver, an inbound END is accepted only after the full byte sequence is sink-accepted; the client keeps downloaded bytes quarantined until END and the success reply agree on the count.
- **ACK is ABORT-only.** The success path contains no ACK; ACK appears solely in an ABORT exchange (either side aborts → peer discards, drains, and ACKs). This is the memory ground truth and the protocol's §*END, abort, and cancellation*.
- **The terminal is the Text reply, settled through the existing per-call `oneshot`.** `StreamCall`'s terminal future *is* the request's dispatch future (§S9); the stream layer defers its settlement until the Text reply arrives (success) or a stream failure classifies it.

### S5 — Credit-bounded outbound bytes (AC-2)

The core owns the client's cumulative credit ledger and grants strictly within it. On inbound `CREDIT { accepted_through, send_through }` the core validates (state-level, protocol §*Credit*): monotonic non-decreasing, `accepted_through <= send_through`, no producer-direction credit, no acknowledgement beyond sent — a violation is a bound-record fault → local ABORT `protocol_error`. Then, for a Producer, it computes the grant

```
grant_to = min(send_through, sent_offset + stream_window_bytes, total_or_source_len)
up_to    = grant_to - sent_offset          // 0 ⇒ no ProduceData emitted (paused for credit)
```

and emits `Action::ProduceData { call_id, up_to }` only when `up_to > 0`. **DATA never exceeds `send_through`** because the grant ceiling is `send_through`; the read-ahead is capped at `stream_window_bytes` so memory is independent of file size; and the client never fabricates the daemon's mandatory one-byte probe — it simply responds to whatever `send_through` the daemon grants (the probe is the daemon's CREDIT behavior, #233). For a Receiver the symmetric rule bounds granted credit to `min(total, accepted_through + stream_window_bytes)` and advances `accepted_through` only after `SinkAccepted`. A deterministic fault test asserts every observed outbound DATA record satisfies `offset + len <= send_through` across an adversarial CREDIT schedule, and that a zero-credit pause emits no DATA.

### S6 — Per-stream absolute deadline (AC-3): connect allowance + floor term

At OPEN the base request/reply deadline (`KernelLimits::default_call_deadline`, which must cover the pre-OPEN handshake as the *connect allowance* does) is **cancelled and replaced** by the stream's absolute budget (protocol §deadlines):

```
floor_term_ticks = ceil(total * 8 * budget_ticks_per_second / transfer_floor_bits_per_second)   // checked
budget_ticks     = transfer_connect_allowance + floor_term_ticks
deadline_at      = open_at.saturating_add(budget_ticks)
```

`total` is OPEN's total (= `declared_bytes` for upload, the verified local size for download). The served limits (`transfer_connect_allowance_ms`, `transfer_floor_bits_per_second`, `transfer_stall_ms`) enter as `StreamLimits` (§6); `budget_ticks_per_second` maps the served millisecond/bit-rate limits into the kernel's abstract tick unit (default `1000` → 1 tick = 1 ms, matching `KernelLimits::default`'s documented convention). All arithmetic is checked; a **zero floor or zero `budget_ticks_per_second` is rejected at `KernelConfig` validation** (construction time — the protocol's "invalid served configuration … refuses readiness", enforced once rather than per stream), and a `total` large enough to overflow saturates `deadline_at` to the far future, where the stall timer remains the effective bound. On `Input::TimerFired(deadline)` for an `Active` stream the core settles the terminal `CallError::Timeout` (`Execution::Unknown` — the transfer may have partially landed), best-effort emits a client ABORT to release the daemon's reservation, and tombstones the `(id, stream_id)` so any late daemon record/terminal is fenced (§S10). Accepted trickle progress cannot extend the absolute deadline (the deadline is armed once at OPEN and never re-armed).

### S7 — Stall timer (AC-3): honest failure on stalled progress

A second per-stream timer arms at OPEN for `transfer_stall` and **re-arms on every accepted-progress event** — an increase in `accepted_through` (from inbound CREDIT for a producer, from `SinkAccepted` for a receiver). Ping/Pong, repeated identical CREDIT, bytes merely queued, and an unchanged transfer push are **not** progress (they never re-arm the timer). On `Input::TimerFired(stall)` with no progress since it was armed, the core settles the terminal with an honest typed failure and tears the stream down exactly as the deadline path does. The ACTIVE tie order is the protocol's: an already-sequenced explicit cancellation wins, then deadline expiry, then stall; **FINALIZING is uncancellable** — once the client has emitted END (producer) or accepted END (receiver) and is awaiting only the terminal reply, neither timer aborts it; the phase is governed only by the sequenced result (a late terminal still settles it, or the connection-loss path does).

> **Classification choice (open, §12).** The client-side deadline/stall are a *safety net* mirroring the daemon's authoritative bounds; when the daemon is healthy it sends the authoritative terminal (`transfer_deadline_exceeded` / `transfer_stalled`) as a `CallError::Wire`, and the client's timer never fires. The client timer exists to bound *client* resources when the daemon goes silent — precisely the role of the request/reply `Timeout`. This spec therefore settles both as `CallError::Timeout` (`Unknown`) with a best-effort courtesy ABORT, reusing the request/reply tombstone pattern verbatim rather than inventing a new error variant. The exact wire ABORT reason (`cancelled` vs a give-up reason the protocol lacks) is deferred to the daemon race table in §12.

### S8 — Streams never auto-replay — the `file.share`/`file.read` replay defect (correctness, red-before-green)

**Current defect.** `ReplayPolicy::derive` (`kernel/replay.rs:44`) lists `"file.share"` in the `op_id`-deduplicated set, so a `file.share` dispatched with a `Dedup::Key` under `stable_principal = true` derives `ReplayPolicy::ReplayableUnderOpId`. The request/reply core would then **hold that call across a reconnect and re-send its Text request** (`core.rs` `on_interrupted` → `replay_hold`). For a byte stream that is wrong on two counts: (a) **no byte stream survives its connection** — the protocol requires "retrying the bytes requires a new `op_id` from offset zero"; and (b) a mid-stream disconnect's `op_id` replay returns `stream_aborted{transport_lost}`, the recorded *failure*, not the original result — the §K5 "wrong answer, not a duplicate" rule. Auto-resending the Text request also strands the stream's byte state.

**Fix.** A stream op is `ReplayPolicy::Never` regardless of `mutating`/`op_id`/`stable_principal`. Implement by gating stream ops out of `ReplayableUnderOpId` in `derive` (an `is_stream_op(op)` check that runs before the dedup-set check, keeping `op_id_deduplicated`'s *shared-set* meaning intact for the daemon-side intent). A disconnected stream then settles honestly through the existing §K6 ledger walk: never-sent request ⇒ `Disconnected { DefinitelyNot }`; past OPEN ⇒ `Disconnected { Unknown }`. The lost-final-reply recovery (a `file.share` whose END was accepted before the drop) is an **explicit caller retry** under the same `op_id` — the daemon's ledger returns the committed result with no second import — *not* a kernel auto-replay (§12 tracks phase-dependent auto-recovery as a possible follow-up). **Red-before-green:** a test dispatching `file.share` with `Dedup::Key`, reaching `Active`, then interrupting, must assert the call is settled (not parked in `replay_hold`) and no second Text request is sent — this fails against the current `derive` and passes after the gate.

### S9 — Cancellation and the `StreamCall` surface (surface unchanged; behavior rewired)

Today `StreamCall::cancel(execution)` resolves the terminal through a *local* `oneshot` and drops the dispatch future — which, with the kernel, would drop the reply future and feed `Input::Cancel`, but would **not** send an ABORT to release the daemon's transfer reservation. #269 rewires the body so cancellation drives the kernel:

- `StreamCall::cancel` / `StreamCancel::cancel` feed the backend an `Input::Cancel(call_id)`. For a stream in `Active`, the core emits a client `Abort { reason: cancelled }`, awaits ACK best-effort (or tombstones immediately, §S10), and settles the terminal `CallError::Cancelled { execution }` with the **kernel-derived** execution — `DefinitelyNot` before OPEN, `Unknown` once any DATA/END has gone out — superseding the caller-supplied value (the current `stream.rs` doc already promises "#269 will supply the true value from framing state"). The public method signatures are unchanged; the `execution` argument becomes advisory (the kernel's framing-state value wins). A queued (pre-send) stream cancel sends nothing, exactly as the request/reply queued-cancel does.
- **`transfer.cancel` stays a separate first-class operation.** True remote cancellation of an upload names its `transfer_op_id` through the ordinary `call::<TransferCancel>` path; it is never a side effect of dropping a `StreamCall`. The kernel routes it like any other request and never conflates a local drop with a remote cancel — the §K9 non-goal, extended to streams.

### S10 — Generation fencing, disconnect, and bound-record faults

- **A stream never survives its connection.** On `Input::Interrupted`/`Closed` (or a send/close race), every `Active`/`Finalizing` stream is torn down locally: release its window and reservation accounting, cancel its two timers, and settle the terminal — never-sent request ⇒ `Disconnected { DefinitelyNot }`, past OPEN ⇒ `Disconnected { Unknown }`. No stream is held for replay (§S8). This reuses the §K6/§K7 walk; the `StreamTable` is drained in the same pass as the ledger.
- **Stale-generation records are fenced.** An inbound `StreamRecordMeta` whose generation is older than the stream's issuing generation (or the current live generation) is discarded before it can touch a `StreamEntry` — the same generation fence §K7 applies to replies and pushes, extended to records.
- **Bound-record faults abort one stream, never the connection.** A request-local malformed record on an active binding (bad reserved byte, unknown kind for the direction, credit regression, oversized DATA within `max_frame_bytes`, offset discontinuity) → the client aborts *that* stream (`protocol_error`) and awaits its `stream_aborted` terminal; a codec-connection-fatal condition (bad magic, no trustworthy binding, malformed daemon OPEN/END/ABORT) arrives as `Inbound::Malformed` or a `4007`-class close the driver reports as connection loss — the codec/#233 rule, surfaced through the seam, not re-decided in the core.
- **Tombstones for streams.** A cancelled/timed-out/stalled stream keeps its `(id, stream_id)` reserved as a tombstone (bounded by `max_concurrent_streams`, evicted oldest-first like the request/reply tombstones) so a late daemon DATA/END/ABORT/terminal is absorbed and strands nothing, then reclaimed by that late record or a reclaim timer.

### S11 — Bounded by construction (AC-7)

Every new collection is statically or configurably bounded, and every terminal releases everything:

| Structure | Bound |
|---|---|
| `StreamTable` (active + finalizing + tombstoned streams) | `max_concurrent_streams`; admitting a stream past it settles the call `QueueFull { resource: "max_concurrent_streams", limit }` (`DefinitelyNot`) before OPEN, mirroring §K2's visible refusal |
| per-stream timers | ≤ 2 (deadline + stall) while `Active`; 0 once terminal |
| per-stream outbound read-ahead / inbound quarantine | `stream_window_bytes` (byte-bounded window, §S5) — held by the driver, granted by the core |
| stream tombstones | ≤ `max_concurrent_streams` (FIFO, oldest evicted) |

There is no map keyed by an unbounded external input (no per-record, per-offset accumulation — the core holds one `StreamEntry` per call, and offsets are scalars). A stream terminal (reply, END-then-reply, ABORT, timeout, stall, disconnect, stop) removes the entry, cancels both timers, and releases the window in one place. `Input::Stop` and terminal failure drain the `StreamTable` alongside the ledger. The controller exposes `streams()`, `stream_timers()`, and `stream_window_bytes_reserved()` so a churn fault test (saturate `max_concurrent_streams`, flap the connection, cancel mid-stream, over thousands of steps) can assert no collection exceeds its bound and that stop empties every one.

### S12 — Secrets and payloads never enter diagnostics (§K15 extended)

The `Debug` impls for `StreamRecordIntent`/`StreamRecordMeta` render only kind, `id`, `stream_id`, and the numeric offsets/values — never payload bytes, and never a file name, `declared_content_type`, or digest (none of which enter the byte-free core anyway, §S2). ABORT reasons render as their closed vocabulary tag, never a free string. `diag.rs`'s redaction test is extended to scan the stream records' `Debug` output for the absence of any payload/identifier field, matching the existing `WireFrame` redaction test (`transport.rs::wire_frame_debug_redacts_op_id_and_payload`).

## 6. Configuration and public surface

One new **public** type, re-exported from `jeliya_client`, and one new field on the existing `KernelConfig`:

```rust
/// The served transfer bounds a stream is driven under (protocol §Served limits
/// / §Byte-stream framing). A host reads these from the daemon's served limits
/// object and passes them in; none defaults silently to "unbounded".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct StreamLimits {
    /// Fixed connect allowance in a stream's absolute budget
    /// (`transfer_connect_allowance_ms`), as a Tick delta.
    pub transfer_connect_allowance: TickDelta,
    /// The floor throughput the size-aware absolute budget is computed from
    /// (`transfer_floor_bits_per_second`). Zero is an invalid served
    /// configuration and is rejected at construction.
    pub transfer_floor_bits_per_second: u64,
    /// Ticks per second — maps the served ms / bit-rate limits into the kernel's
    /// abstract tick unit. Default 1000 (1 tick = 1 ms), matching KernelLimits.
    pub budget_ticks_per_second: u64,
    /// Zero-accepted-progress window before an honest stall failure
    /// (`transfer_stall_ms`), as a Tick delta.
    pub transfer_stall: TickDelta,
    /// The byte-bounded per-stream read-ahead (producer) / quarantine (receiver)
    /// window — the client's cumulative-credit ceiling. Bounds memory
    /// independent of file size.
    pub stream_window_bytes: u64,
    /// The maximum number of concurrent streams the client will drive; a stream
    /// past it is refused before OPEN (bounds the StreamTable, §S11).
    pub max_concurrent_streams: u32,
}

impl Default for StreamLimits { /* conservative, documented; e.g. window 1 MiB, floor 64 kbit/s */ }

pub struct KernelConfig {
    pub limits: KernelLimits,
    pub jitter_seed: u64,
    pub stable_principal: bool,
    /// NEW — the stream bounds. Validated at construction (non-zero floor and
    /// ticks-per-second); an invalid served configuration refuses readiness.
    pub streams: StreamLimits,
}
```

- **Unchanged public seam:** `ClientHandle`, `call`, `call_stream`, `StreamCall`, `StreamCancel`, `CallError`, `Execution`, `State`, `ClientEvent`, `Dedup`, `EventSubscription`, the mock. No new `CallError` variant is required — `Timeout`, `Cancelled`, `Disconnected`, `Wire`, and the `QueueFull { resource, limit }` shape already cover every stream outcome (the new `resource` value `"max_concurrent_streams"` is a `&'static str`, additive).
- **Not exported:** `StreamRecordIntent`, `StreamRecordMeta`, the media-seam Actions/Inputs, `StreamTable`/`StreamEntry` — all `pub(crate)`, consumed by the adapters, exactly as the request/reply `Transport`/`WireFrame` are.

## 7. Implementation steps

1. **`kernel/replay.rs` — the replay gate (§S8), first and standalone.** Add `is_stream_op(op)` and gate stream ops to `ReplayPolicy::Never` in `derive`. Add the unit cases (`file.share` + `Dedup::Key` + `stable_principal` ⇒ `Never`; `file.read` ⇒ `Never`). This is a self-contained correctness fix that can land and be reviewed before the lifecycle machinery.
2. **`kernel/transport.rs` — the seam extension (§S3).** Add `StreamRecordIntent` (client-sendable kinds), `StreamRecordMeta` (all kinds), `Inbound::Record { generation, record }`, and the media Actions/Inputs. Add the redaction-safe `Debug` impls + their tests (§S12). No behavior yet — types + the boundary.
3. **`kernel/streaming.rs` — the sub-core (§S1, S4–S7, S10, S11).** `StreamTable`, `StreamEntry`, `StreamRole`, `StreamPhase`; the credit ledger and grant arithmetic (§S5); the deadline/stall arming and the checked budget math (§S6/S7); OPEN admission, END-after-ack, ABORT/ACK, tombstones, teardown; bounds introspection. Byte-free: assert (in review and by the `boundaries.rs` scan) no payload field exists.
4. **`kernel/core.rs` — wire the sub-core in.** Grow `Input`/`Action` with the stream + media variants; flag stream ledger entries `stream: true`; on a stream op's first Text reply *not* settling but an OPEN record installing the `StreamEntry`; delegate every stream input to `streaming.rs`; drain the `StreamTable` in `on_interrupted`/`fail_all`/`on_stop` alongside the ledger; apply the generation fence to `Inbound::Record`.
5. **`kernel/mod.rs` — config + driver.** Add `StreamLimits` to `KernelConfig` with construction-time validation; grow the `Shared` driver with the media buffers and the bounded outbound-record log; extend `KernelController` with `open(total)`, `credit(accepted_through, send_through)`, `deliver_data(offset, len)` (drives the deterministic `i mod 251` source/sink), `end(offset)`, `abort(reason)`, `ack()`, and observers `take_outbound_records()`, `streams()`, `stream_timers()`. The controller is the reference driver: it frames via `jeliya-codec` and enforces the same media contract the real adapters will.
6. **`src/stream.rs` — rewire the surface body (§S9).** Replace `dispatch_typed` with the stream dispatch path; route `StreamCall::cancel`/`StreamCancel::cancel` to `Input::Cancel`; keep the public types and signatures exactly. Update the module/`call_stream` docs to state the hooks now ship (remove the "did not ship with #168 / depth deferred" language, pointing framing at #233/#242/#243).
7. **`src/lib.rs`** — `pub use kernel::StreamLimits;` and thread `streams` through the construction path.
8. **Tests** — `tests/kernel_stream.rs` (happy-path lifecycle, both directions) and `tests/kernel_fault.rs` extensions (§8), all on the deterministic driver + virtual clock.
9. **`tests/boundaries.rs`** — confirm the `src/kernel/**` scan already covers `streaming.rs`; assert zero new runtime dependencies in the library tree.
10. **Docs** — normative surface is crate rustdoc (match `jeliya-api`/seam density). No new `docs/` page is required; the framing decision already lives in `docs/protocol-v2.md`. Amend `specs/rust-client-bounded-kernel.md` §K16's status line to point at this document as the finished design record (a spec-hygiene edit, not code).

## 8. Test strategy — every acceptance criterion mapped

**Acceptance criteria (all on the deterministic in-memory driver):**

| Issue AC | Mechanism | Test |
|---|---|---|
| `call_stream::<FileShare>` / `::<FileRead>` drive a full OPEN→DATA/CREDIT→END/ABORT lifecycle through the kernel | §S1/§S4 sub-core over the unchanged core | upload: request → controller `open(N)`+`credit` → observe credit-bounded DATA → `credit` acks → END emitted after full ack → `deliver_reply` → terminal `FileShareOut`. download: request → `open(N)` → observe client CREDIT → `deliver_data*` → sink-accepted → `end(N)` → reply → `FileReadOut` |
| Credit enforced: outbound DATA never exceeds granted credit; final CREDIT/END/Text-reply ordering matches protocol-v2 | §S5 grant arithmetic; §S4 END-after-ack | adversarial CREDIT schedule (zero-credit pause, staircase grants) ⇒ assert every DATA `offset+len <= send_through`, zero DATA while paused, END only after `accepted_through == sent_total`, terminal only after END |
| Per-stream absolute deadline + stall timer produce honest typed failures | §S6/§S7 timers | deadline: `open(N)`, advance past `connect_allowance + floor_term` with progress stalled ⇒ terminal `Timeout{Unknown}` + courtesy ABORT + tombstone. stall: `open`, one CREDIT, then advance `transfer_stall` with no further accepted progress ⇒ honest failure; a progress event re-arms and defers it |
| AC-7: no stream failure mode leaves an unbounded task, map, or timer | §S11 bounds | saturate `max_concurrent_streams` ⇒ `QueueFull{max_concurrent_streams}`; churn (flap + cancel mid-stream + stall) over thousands of steps ⇒ `streams()`/`stream_timers()`/window never exceed bounds; after stop all zero |
| Framing rules remain doc-pointed at #233/#242/#243 — no re-implementation | §S2/§S3 byte-free core; codec at the driver boundary | `boundaries.rs` dependency-tree scan (no new dep) + `src/kernel/**` token scan; a review checklist item that `streaming.rs` holds no `Vec<u8>`/payload field and no JBS2 constant |

**Additional verification (property/fault):**

- **Streams never auto-replay (§S8, red-before-green)** — `file.share` + `Dedup::Key`, reach `Active`, interrupt ⇒ settled honestly, `replay_hold` empty, no second Text request. Fails against current `derive`.
- **Disconnect at each phase (§S10)** — never-sent ⇒ `Disconnected{DefinitelyNot}`; mid-DATA ⇒ `Disconnected{Unknown}`; FINALIZING (END emitted, awaiting reply) ⇒ the late terminal still settles, or disconnect settles `Unknown`; the `StreamTable` empties.
- **Cancellation at each phase (§S9)** — queued stream cancel sends nothing; `Active` cancel emits ABORT and settles `Cancelled{Unknown}` with the kernel-derived execution; a late daemon terminal is absorbed by the tombstone.
- **Bound-record fault (§S10)** — a credit regression or oversized-within-frame DATA aborts only that stream (`protocol_error`), leaving a second concurrent stream and ordinary requests untouched.
- **Stale-generation record fenced (§S10)** — a DATA/END from generation N-1 after a reconnect is dropped.
- **Zero-byte stream (§S4)** — `open(0)` → CREDIT → END at offset 0 → terminal, no DATA record ever sent/expected.
- **Total stop mid-stream (§K11 extended)** — stop with an `Active` stream settles it once (`Cancelled`), cancels both timers, drains the `StreamTable`, closes the bus.
- **Redaction (§S12)** — the stream records' `Debug` renders no payload/name/digest/reason string.

**Determinism guard:** every stream test uses only the in-memory driver, the virtual clock, and a fixed `jitter_seed`; the deterministic byte source is `i mod 251` (matching the conformance harness), so behaviour is identical on native and `wasm32-unknown-unknown`.

## 9. CI

- The existing `jeliya-client` workspace test already compiles `tests/kernel_stream.rs` and the `kernel_fault.rs` extensions (production code, default-on). The deterministic stream driver ships behind the existing `test-transport` feature (default-off), so the extra step is the *existing* `cargo test -p jeliya-client --features test-transport` invocation — no new CI step, only more cases under it.
- **MSRV 1.91** must still compile the stream layer: no edition-2024-only syntax, no std API newer than 1.91, no new dependency. The existing MSRV job gates this.
- `boundaries.rs` (run by the workspace test) covers `streaming.rs` under its existing `src/kernel/**` scan; no new CI step.
- No new toolchain, target, or runner capability: the kernel adds no dependency and no wasm test run (compilation is already gated).

## 10. Risks and mitigations

- **Scope creep into the framing/executor (#233/#242/#243).** Re-implementing the JBS2 header, size-precedence ladder, or the daemon sequencer here would duplicate #233. *Mitigation:* the byte-free core (§S2), the codec-at-the-driver-boundary rule, and a review checklist item that `streaming.rs` holds no payload field and no framing constant.
- **The stream replay defect shipping silently (§S8).** If the gate is forgotten, a `file.share` with a `Dedup::Key` will auto-replay a byte stream. *Mitigation:* land §S8 first, standalone, with the red-before-green test.
- **A hidden wall clock or unbounded buffer.** The deadline/stall could tempt a real timer, and credit could tempt a whole-file buffer. *Mitigation:* `Tick`-based timers only (existing scan), and offsets-not-bytes in the core with a byte-bounded window (§S2/§S5).
- **Cancellation that leaks the daemon's reservation (§S9).** Dropping a `StreamCall` without an ABORT would strand the daemon's transfer slot. *Mitigation:* cancel drives `Input::Cancel`, which emits a client ABORT before settling.
- **Deadline unit confusion.** Mixing served milliseconds/bit-rates with abstract ticks could produce a wrong budget. *Mitigation:* an explicit `budget_ticks_per_second` mapping (default 1000), checked arithmetic, and construction-time rejection of a zero floor.
- **FINALIZING cancelled by a late timer.** A stall/deadline firing after END would abort a stream the daemon may already have committed. *Mitigation:* §S7's FINALIZING immunity — once END is emitted/accepted, neither timer aborts; only the sequenced terminal settles.
- **Surface drift.** If a new `ClientBackend` method were needed, #167's "sufficient without a breaking change" claim would fail. *Mitigation:* streams route through the existing `dispatch`/`Input` machinery; the media seam is `Driver`-side (internal). Any gap is a reviewable finding surfaced in the PR, not a silent seam change.

## 11. Non-goals (restated)

- **Framing execution** — the JBS2 byte layout, offset arithmetic, per-kind field rules, size precedence, malformed-record close-code selection (codec / #233/#242/#243).
- **The daemon executor** — OPEN authoring, staging/import, the ACTIVE→FINALIZING sequencer, the `transfer.cancel` registry (#233 `jeliyad`).
- **`transfer.cancel` as a stream hook** — it is the ordinary first-class request it already is; a `StreamCall` drop is a local abort, never a remote cancel (§S9).
- **Real `PlatformServices` sources/sinks** — the media seam is the driver's; #174/#171/#172/#173 plug in real file bytes.
- **Concrete sockets** — #171/#172/#173.
- **Resumable / chunked transfer** — restart-from-zero only; resumability is #209.
- **The public seam surface** — unchanged; #269 adds internal machinery + the small `StreamLimits` config only.

## 12. Open questions

1. **Client stall/deadline classification (§S7).** Is `CallError::Timeout { Unknown }` the right client-side settlement for a stream the *daemon* is authoritatively bounding, or should the client wait for the daemon's authoritative `transfer_stalled`/`transfer_deadline_exceeded` `Wire` reply and treat its own timer purely as a connection-liveness backstop? Recommend `Timeout` (reuses the request/reply pattern, bounds client resources honestly); revisit under #175's parity suite once a real adapter observes daemon timing.
2. **The give-up ABORT reason.** The protocol's ABORT vocabulary has no "client timed out / gave up" reason (`cancelled`/`source_failed`/`sink_failed`/`protocol_error`/`operation_error`). Which does a client-side deadline/stall ABORT carry? Recommend `cancelled` (the client is unilaterally abandoning), deferring a definitive answer to the daemon race table in `docs/protocol-v2.md` §*END, abort, and cancellation*.
3. **Phase-dependent auto-recovery of the lost final reply (§S8).** A `file.share` whose END was accepted before the drop *could* auto-replay its `op_id` to recover the committed terminal (the daemon opens no second stream). #269 keeps this an explicit caller retry; should a later slice add bounded auto-recovery for the FINALIZING-then-disconnect case only? Recommend explicit-retry for #269, tracked as a follow-up.
4. **Media seam ownership vs. `PlatformServices` (#174).** The source/sink seam is the driver's here. Confirm that #171/#172/#173 source real bytes from `PlatformServices` (#174) through this seam rather than the kernel growing a `PlatformServices` dependency — the assumption this spec builds on.
5. **`stream_window_bytes` / served-limits provenance.** The served transfer limits are the daemon's; confirm the adapter reads them from the served limits object at handshake and constructs `StreamLimits` from them (rather than a compile-time default), so the client's credit window and budget track the daemon's actual configuration.
6. **`test-transport` reuse by #175.** Confirm the extended `KernelController` stream/media API is the surface #175 drives the four real adapters against, so the parity diff covers streams — the same reuse question §12.6 of the #168 spec raised for request/reply.

## 13. Assumptions

- `crates/jeliya-client` is at its #168 shape: the request/reply kernel (`core.rs`, `admission.rs`, `ids.rs`, `inflight.rs`, `replay.rs`, `backoff.rs`, `timing.rs`, `transport.rs`, `diag.rs`), the `KernelBackend`/`KernelController` shell behind `test-transport`, and the `StreamCall`/`StreamCancel` surface in `src/stream.rs` — all present and consumed here without changing their public surface.
- `crates/jeliya-codec` (#164/#233) provides the byte-stream record types and staged decoders (`StreamRecord`, `StreamRecordKind`, `decode_stream_*`, `encode_stream_record`, `max_stream_data_bytes`); the driver — not the kernel core — depends on them for framing.
- `jeliya_api::{FileShare, FileShareOut, FileRead, FileReadOut}` are stable at their current shapes (`ops.rs:747`/`858`), `FileShare` is `mutating` and `FileRead` is not, and both are the *only* two client-driven stream ops.
- `docs/protocol-v2.md` §*Byte-stream framing* (credit, deadlines, END/ABORT, disconnect, the two success sequences) and the served limits `transfer_connect_allowance_ms` / `transfer_floor_bits_per_second` / `transfer_stall_ms` are authoritative; this layer implements the client half against them.
- The kernel's tick unit convention is 1 tick = 1 ms (the `KernelLimits::default` documentation), so `budget_ticks_per_second` defaults to 1000; a host that chooses another mapping supplies a matching value.
- The crate MSRV is **1.91**, the CI toolchains are 1.96.0 (primary) + 1.91.0 (MSRV) with `wasm32-unknown-unknown` available, and the stream layer adds no dependency and no wasm-hostile code.
- The orchestrator performs all git/gh/PR actions; this document is the only artifact the planning phase produces, and no production code is written for #269 by the planning phase.
