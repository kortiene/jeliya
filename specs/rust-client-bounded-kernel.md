# Spec — Bounded, lifecycle-aware Rust client kernel (#168)

- **Issue:** kortiene/jeliya#168 — `[Rust][Client]: Implement bounded request, lifecycle, cancellation, and retry-safety semantics`
- **Program:** #156 (Dioxus clean-slate). **Milestone:** M2 (client runtime and platform adapters).
- **Records/derives its decision from:** `docs/dioxus-architecture.md` §"Decision 4 — one seam, four adapters, one platform boundary" (the kernel paragraph) and `docs/protocol-v2.md` (the generation gate, idempotency/retry, connection generation, deadlines, pushes/gap/resync).
- **Depends on (both landed):** #164 (`crates/jeliya-codec`, the protocol-v2 codec) and #167 (`crates/jeliya-client`, the lifecycle-aware seam and deterministic mock — present in the workspace today).
- **Blocks / is the entry point for:** #171 `WsWeb`, #172 `WsNative`, #173 `DirectClient`, #175 (the one fault-injected four-adapter parity suite), and #169's single resync path consumes the kernel's fencing.
- **Owner role:** core maintainer (per the architecture layering table: "client kernel and seam … must not depend on a specific transport; backend erasure stays internal").
- **Status of this document:** implemented, **except §K16**. The request/reply kernel ships as `crates/jeliya-client/src/kernel/` (modules: `core`, `admission`, `ids`, `inflight`, `replay`, `backoff`, `timing`, `transport`, `diag`), with fault coverage in `tests/kernel_fault.rs` and the deterministic in-memory driver behind `feature = "test-transport"`. The **stream lifecycle hooks §K16 describes did not ship with #168** — `call_stream` still enters the generic request path — and are owned by the follow-up issue #269 (blocking stream ops in #171/#172/#173 and the #175 parity suite). This document is the authoritative design record; where it and the code disagree the code is right and this document has a bug.

> Where this spec and `docs/protocol-v2.md` or `docs/dioxus-architecture.md` disagree, those records are
> authoritative and this spec has a bug — say which in the PR, exactly as the architecture record
> requires of every slice that tests against it.

---

## 1. Outcome

Build the **transport-independent client kernel** that sits *behind* the `ClientBackend` trait #167 defined, giving the seam its real machinery:

1. **Hard request bounds** — an explicit, byte-aware queued-admission limit and an explicit in-flight limit; `QueueFull` is *visible*, never absorbed.
2. **Deterministic settlement** — every accepted call settles **exactly once** locally; malformed, duplicate, and late replies cannot strand a call or double-settle it.
3. **Correlation and operation IDs** — a per-connection correlation id for reply matching and the envelope `op_id` for cross-reconnect deduplication, with ids that cannot collide with an **outstanding** id after wrap (completed-id reuse at the `2^53` ceiling is outside the design envelope — §K3).
4. **Deadlines** — a per-call absolute deadline yielding `Timeout` (may-have-executed `Unknown`), driven by an injected clock/timer, never a wall clock inside the library.
5. **Cancellation at every phase** — queued, sent-awaiting-reply, and mid-stream; a dropped or explicitly-cancelled caller future stops *local* delivery and never fabricates remote cancellation.
6. **Reconnect generations (generation fencing)** — a monotonic connection generation stamps every in-flight call and every inbound frame; stale-generation replies, pushes, and teardown state are rejected.
7. **Capped, jittered backoff** — bounded reconnect attempts with full-jitter exponential backoff from a deterministic in-core PRNG; exhaustion is honest, not an infinite spin.
8. **Honest post-send uncertainty** — connection loss distinguishes **never-sent** (`DefinitelyNot`) from **may-have-executed** (`Unknown`); only operations with an explicit, tested v2 dedup guarantee may replay, and **everything else never auto-replays**.
9. **Total stop** — `stop` cancels any in-progress dial/backoff, drains queue and in-flight settling each call once, closes every event stream, and leaves **no unbounded task or map** behind.

This kernel is the reference the four adapters are measured against; the deterministic mock (#167) remains the seam's reference fixture, and this issue adds a second reference — the kernel driven by a **deterministic in-memory transport** — with **no concrete sockets** (those are #171/#172/#173).

## 2. What this issue is, and what it is not

The architecture splits the client into two adjacent slices plus the adapters:

- **#167 — the seam.** Owns the *public contract and the types the kernel's decisions flow through*: `ClientHandle`, the object-safe `ClientBackend` trait, `ErasedCall`, `CallError`/`Execution`/`LocalError`, `State`, `ClientEvent`, the multi-consumer `EventBus`, and the deterministic **mock** backend. Its error taxonomy already carries the kernel's outputs — `QueueFull { resource, limit }`, `Timeout`, `Cancelled { execution }`, `Disconnected { execution }` — and `Execution` (`DefinitelyNot`/`Unknown`/`Definitely`). `ErasedCall` already carries `op`, `mutating`, `op_id`, and `input`.
- **#168 (this issue) — the kernel below it.** Fills in the *internal machinery* behind the same `ClientBackend` trait: real bounded queues, correlation-id allocation, the in-flight/settlement ledger, deadlines, cancellation, generation fencing, backoff, the dedup/replay ledger, and total stop. It defines a **transport seam** the adapters implement, and ships a deterministic in-memory driver for tests.
- **The adapters (#171/#172/#173) — concrete transports.** `WsWeb` (browser WebSocket + `/api/session`), `WsNative` (native async WebSocket via the supervisor + resolver), `DirectClient` (in-process `jeliya-core` behind a bounded serial actor). Each implements the transport seam this issue defines. **This issue writes none of them.**

**Consequence for scope.** #168 does **not** modify the public seam surface — proving #167's claim that `ClientBackend` is "sufficient … without a later breaking change." It adds a new internal module tree and a second, kernel-backed `ClientBackend` implementation. Where the kernel legitimately needs a field the seam already reserved (`ErasedCall.input`, `ErasedCall.mutating`, `ErasedCall.op_id` are `#[allow(dead_code)]` in #167), consuming it removes that dead-code allowance — an expected, non-breaking change internal to the crate.

## 3. Owning crate and layout

The kernel lives **inside** `crates/jeliya-client` (the seam crate), not a new crate: the kernel *is* the transport-independent runtime the architecture pairs with the seam ("one cloneable UI-facing handle over a transport-independent kernel"), and it reuses the seam's private infrastructure (`EventBus`, `RawJson`, `ErasedCall`, `ClientBackend`) directly. A separate crate would force those into a shared-public boundary and duplicate the erasure.

```
crates/jeliya-client/
  src/
    lib.rs            # add `mod kernel;` + re-exports of the new public config/error surface
    backend.rs        # (unchanged trait) — kernel consumes ErasedCall.{input,mutating,op_id}
    error.rs          # (unchanged taxonomy) — kernel produces QueueFull/Timeout/Cancelled/Disconnected
    event.rs          # (unchanged) — kernel reuses EventBus/State/ClientEvent
    kernel/
      mod.rs          # KernelBackend: ClientBackend + KernelConfig/KernelLimits + public re-exports
      core.rs         # the SANS-IO state machine: inputs -> (settlements, actions); no I/O, no clock
      admission.rs    # bounded queue + in-flight admission, byte accounting, QueueFull
      ids.rs          # CorrelationId allocator (per-connection, wrap-safe) + generation counter
      inflight.rs     # the outstanding-call ledger: settle-exactly-once, late/dup/malformed suppression
      replay.rs       # dedup/replay eligibility + bounded replay ledger (op_id-guaranteed only)
      backoff.rs      # capped full-jitter exponential backoff + deterministic xorshift PRNG
      timing.rs       # Tick (logical monotonic time) + Deadline; NO wall clock
      transport.rs    # the Transport/driver seam adapters implement (#171/#172/#173)
      diag.rs         # redaction: secrets/tokens/op_id/payload bytes never enter diagnostics
    mock/mod.rs       # (unchanged) — the seam's reference fixture stays
  tests/
    kernel.rs         # property/fault suite driven by the deterministic in-memory transport
    kernel_fault.rs   # explicit fault cases: saturation, timeout, phase cancellation, races, decode fail,
                      #   reconnect exhaustion, stop (one test per Verification bullet)
```

**Boundary invariants (already asserted by `tests/boundaries.rs`, extended):**

- The library still carries **no** Iroh, WebSocket crate, native transport, `tao`/`wry`, Dioxus, or `tokio` (dependency-tree scan). The kernel adds **no** new runtime dependency; concurrency stays on `futures` primitives (`oneshot`, `Stream`, `BoxFuture`).
- **No wall clock in the library.** The kernel core takes logical time (`Tick`) as an *input* and emits "arm timer at `Tick`" as an *action*; it never reads `std::time`. The deterministic test driver owns a virtual clock. Real adapters own the real clock/timer (`std`/`wasm`), outside this library. Extend `boundaries.rs` with a source scan asserting `std::time`, `Instant::now`, `SystemTime`, and `getrandom`/`rand` appear in **no** kernel source file.
- **No spawn in the library.** `KernelBackend` never spawns; the driver owns the event loop (test driver = manual stepping; real adapters spawn via the platform/supervisor, not this crate). Keeps `wasm32-unknown-unknown` clean and determinism total.
- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]` continue to hold; every new public item is documented to `jeliya-api` density.

## 4. Architecture — a sans-IO core with a driver seam

The kernel is a **sans-IO state machine** (the pattern of `quinn-proto`/`rustls`): all bounded-request, lifecycle, cancellation, and retry logic lives in a pure, synchronous core with **no async, no I/O, no clock, and no RNG syscall**. The core is a function of its inputs, so every fault the Verification section names is expressed as an ordinary, deterministic unit input — identical on wasm and native.

```
        ┌───────────────────────── crates/jeliya-client ──────────────────────────┐
UI ──▶  ClientHandle ──▶ Arc<dyn ClientBackend>
                                   │
                          ┌────────┴─────────┐
                          │  KernelBackend    │  implements ClientBackend
                          │  Arc<Mutex<Core>> │  + oneshot reply correlation + EventBus
                          └────────┬─────────┘
                                   │  step(Input, now: Tick) -> Vec<Action>
                          ┌────────┴─────────┐
                          │   kernel::core    │  SANS-IO: admission, ids, inflight,
                          │   (pure, sync)    │  replay, backoff, fencing, stop
                          └────────┬─────────┘
                                   │  Action (Send, ArmTimer, Dial, CancelDial, Settle, Emit, Close)
                          ┌────────┴─────────┐
                          │  Transport seam   │  #168 defines; adapters implement
                          └────────┬─────────┘
             ┌─────────────────────┼─────────────────────────┐
   (this issue)                 (#171)         (#172)          (#173)
   deterministic in-memory    WsWeb          WsNative        DirectClient
   test transport             browser WS     native WS       in-proc core
```

- **`step`** is the single entry point of the core: it consumes one `Input` and the current logical time `now: Tick`, mutates bounded internal state, and returns a `Vec<Action>` for the driver to perform. It **never blocks and never awaits**.
- **`Input`** — `Dispatch(ErasedCall, reply: Settle)`, `Start`, `Stop`, `Inbound(Frame)`, `TimerFired(TimerId)`, `Connected { generation }`, `Interrupted { reason }`, `Closed { reason }`, `Cancel(CorrelationId)` (a caller dropped/cancelled its future).
- **`Action`** — `Send(WireFrame)`, `ArmTimer { id, at: Tick }`, `CancelTimer(TimerId)`, `Dial`, `CancelDial`, `Settle(CorrelationId, Result<RawJson, CallError>)`, `Emit(ClientEvent)`, `CloseBus`.
- **`Settle`** is the sender half of the per-call `oneshot`; the driver applies `Action::Settle` by taking the ledger's sender and sending exactly once (the ledger guarantees the sender is present at most once — see §K4).

`KernelBackend::dispatch` locks the core, calls `step(Input::Dispatch(call, settle))`, hands the returned actions to the driver, and returns the `oneshot::Receiver` (mapped to `Result<RawJson, CallError>`) — exactly the `BoxFuture` shape the seam expects. `subscribe`/`state` read core state; `start`/`stop` feed `Input::Start`/`Input::Stop`.

## 5. Design decisions

### K1 — Sans-IO core is the whole point (Verification: determinism)

Because the core is pure and time is an input, "timeout at each phase", "send/close race", and "reconnect exhaustion" are **sequences of `step` calls**, not timing-dependent integration tests. This is what lets the property/fault suite be exhaustive and reproducible, and it is why the core carries **no** clock, RNG syscall, task, or socket. The async surface is a thin shell (`KernelBackend` + the driver); all correctness lives in `core.rs` and its helpers.

### K2 — Hard request bounds: byte-aware queued admission + explicit in-flight cap (AC-1, AC-7)

Two explicit, configured limits, both tested:

- **`queue_depth`** — the maximum number of *admitted-but-not-yet-sent* calls (`Connecting`/back-pressured while `Ready`). Exceeding it refuses the new call with `CallError::QueueFull { resource: "queue_depth", limit }` **before any byte is sent** ⇒ `Execution::DefinitelyNot`.
- **`outbound_bytes`** — a byte cap on the queued outbound payloads (protocol §Bounded resources: "outbound queues MUST be byte-bounded; a message-count-only queue is insufficient"). A request whose serialized `in` would push queued bytes past the cap is refused `QueueFull { resource: "outbound_bytes", limit }` ⇒ `DefinitelyNot`.
- **`in_flight`** — the maximum number of *sent-and-awaiting-reply* calls. This is a **throttle, not a rejection**: when in-flight is at the cap, further admitted calls stay queued (subject to `queue_depth`) and are released as replies land. The invariant `in_flight_count <= in_flight` is asserted by construction and by a saturation test; the in-flight map never grows past the cap (AC-7).

`QueueFull` is therefore only ever raised for `queue_depth` or `outbound_bytes` (admission-time), never for in-flight throttling. Both limits live in `KernelLimits` (§6) and both are exercised (queue rejection test; in-flight throttle-invariant test).

### K3 — Correlation ids and operation ids, wrap-safe (Security: IDs cannot collide after wrap)

Two distinct id spaces, matching `jeliya-api`:

- **`CorrelationId`** — the envelope `id: RequestId` (`u64`, `0..=2^53-1`, `MAX_REQUEST_ID`). It correlates a **reply to a request and is unique only while outstanding on one connection**. The kernel allocates it; the codec (#164) serializes it. It is **reset per connection generation** (a fresh connection starts a fresh id space), so it never carries meaning across a reconnect.
- **`op_id`** — the envelope `OpId` (opaque string), *caller-supplied* via `Dedup::Key`, the **cross-reconnect** dedup key. The kernel never generates it; it forwards the caller's `Dedup` choice and consults it only for replay eligibility (§K5). The kernel imposes **no** `op_id` on operations the caller left as `Dedup::None`.

**Wrap safety.** The allocator issues the next id as a monotonic counter and **skips any candidate currently present in the in-flight/queued ledger**, refusing to reuse an outstanding id. Because the outstanding set is bounded by `queue_depth + in_flight` (both ≪ `2^53`), a free id always exists; on the astronomically unreachable approach to `MAX_REQUEST_ID` the counter wraps to zero and resumes skipping. The skip rule covers *outstanding* ids only: post-wrap, an id from a long-completed call of the same connection could in principle be reissued, and a delayed duplicate reply for that ancient call would resolve to the new entry — generation fencing cannot help within one connection. Reaching that state requires `2^53` requests on a single live connection (~285 years at 1M req/s) and is **outside the design envelope, recorded as such**: a deployment that could approach the ceiling must retire the connection first (a reconnect resets the id space and its generation). A property test drives the counter to the boundary (seeded near `MAX_REQUEST_ID`) with a saturated ledger and asserts no allocation ever equals an outstanding id.

### K4 — Exactly-once local settlement; late/duplicate/malformed replies cannot strand a call (AC-2, Security)

The **in-flight ledger** (`inflight.rs`) maps `CorrelationId -> Outstanding { settle: Settle, generation, deadline_timer, sent: bool, replay: ReplayPolicy }`. Settlement is a **take**: `settle_once(id, result)` removes the entry and sends on the `oneshot`; a second attempt on the same id finds nothing and is a no-op. Therefore:

- **Every accepted call settles exactly once.** The single owner of the sender is the ledger entry; once taken, the call is settled and the entry is gone.
- **A duplicate reply** (same id twice) settles the first and is dropped on the second (no entry) — never a double-settle, never a panic.
- **A malformed/undecodable reply frame** is classified at the driver boundary: a frame the codec (#164) cannot parse to an envelope is **dropped with a diagnostic** (it correlates to no id, so it cannot strand a call); a reply that *parses* but whose `out` cannot decode to `O::Output` is the seam's existing `LocalError::DecodeReply` (`Definitely` — the daemon ran it), produced by the handle's decode step, not the kernel. The kernel's job is only to deliver the raw `out` bytes to the right `oneshot` exactly once.
- **A late reply** — one whose id is no longer outstanding (already settled by timeout/cancel/disconnect, or from a prior generation) — finds no entry (or is generation-fenced) and is dropped. It can never re-open or double-settle a call. This is the "malformed/duplicate/late replies cannot strand calls" guarantee, made structural by the take-once ledger.

### K5 — Deduplication and replay policy: opt-in, bounded, guaranteed-only (AC-3)

The protocol's ledger is keyed on `(session principal, op_id)` and **only the 13 `op_id`-deduplicated mutating operations** guarantee "a replayed `op_id` returns the original result and performs no second effect." The kernel encodes that guarantee as a per-call `ReplayPolicy` decided at admission:

| Call shape (from `ErasedCall`) | `ReplayPolicy` | Behaviour on reconnect |
|---|---|---|
| `mutating == true` **and** `op_id == Some(_)` (caller chose `Dedup::Key`) | **`ReplayableUnderOpId`** | MAY be replayed under the same `op_id` on the next connection, bounded by the reconnect budget; the daemon's ledger returns the original result or the committed error. |
| `mutating == true` **and** `op_id == None` | **`Never`** | Never auto-replayed. On disconnect it settles with the never-sent/may-have-executed classification (§K6). |
| `mutating == false` (any `op_id`) | **`Never`** (default) | Never auto-replayed by default. Non-mutating re-issue is *safe*, but the kernel does not silently re-issue; the caller observes the disconnect and decides. (Re-issue-on-reconnect for reads is an explicitly-scoped **open question**, §14, not a default.) |

Rules made testable:

- **The kernel never auto-replays a call whose policy is `Never`** — every non-goal about "automatic replay without an explicit tested v2 deduplication guarantee" and "all others never auto-replay" is this one rule.
- **Replay reuses the caller's `op_id` verbatim** and re-serializes the identical `in` bytes; it never mutates the request (an `op_id` replayed with a *different* body is the daemon's `op_id_conflict`, surfaced as `CallError::Wire`, not a client retry).
- **`Dedup::Key` on a non-deduplicating operation** stays `ReplayPolicy::Never` even though the envelope still carries the `op_id` (the daemon "accepts it and ignores it"): the kernel does not manufacture a replay guarantee the protocol does not give. Only `mutating && op_id.is_some()` earns `ReplayableUnderOpId`, matching the protocol's "`op_id` deduplicated" row.
- Replay is **bounded** by the reconnect budget (§K10); on exhaustion a replayable call settles `Disconnected { Unknown }` (it was sent at least once).

> **Note on the `mutating && op_id` heuristic.** The heuristic is deliberately **broader** than the daemon's dedup-ledger set: a `Dedup::Key` on a mutating operation outside that set (`daemon.stop`, `transfer.cancel`, the naturally idempotent mutations, connection-scoped `stream.*`) is also classified replayable. That is safe because for every such operation the protocol's idempotency table makes a repeat harmless *within a daemon lifetime* — `daemon.stop` is terminal single-effect (a replay returns `shutdown_in_progress`, an honest typed result), the naturally-idempotent ops return existing state, and `stream.*` re-issue is connection-scoped and always safe. The risk direction is therefore a **duplicate answer**, never a silent double effect; the daemon-restart case voids the in-memory dedup ledger for *every* operation equally and is out of this milestone's envelope. §14 Q3 asks whether the kernel should instead consult an explicit per-operation replay table from `jeliya-api` rather than the `mutating && op_id` heuristic. Replay additionally requires the driver's **stable-principal certification** (`KernelConfig::stable_principal`, default **false**): the ledger is keyed `(principal, op_id)`, so an adapter that omits `client_id` receives a fresh ephemeral principal per connection, where a replay would re-execute a lost-reply mutation. Without the certification every call is `Never` and a disconnect settles honestly as `Disconnected { Unknown }`.

### K6 — Connection loss: never-sent vs may-have-executed (AC-4)

On `Input::Closed { reason }` (or `Interrupted` that the backoff budget cannot recover), the core walks the outstanding ledger and settles each call by **send state and replay policy**:

| Outstanding call | Settlement | `Execution` |
|---|---|---|
| Never-sent (queued, `sent == false`) | `CallError::Disconnected` | `DefinitelyNot` — no byte left the client, it provably did not execute |
| Sent, `ReplayPolicy::Never` | `CallError::Disconnected` | `Unknown` — bytes went out; it may have landed |
| Sent, `ReplayableUnderOpId`, budget remaining | **held for replay** (§K5/§K10), not settled yet | — |
| Sent, `ReplayableUnderOpId`, budget exhausted | `CallError::Disconnected` | `Unknown` |

This is the AC verbatim — "connection loss distinguishes never-sent from may-have-executed work" — expressed as a total function over the ledger. It flows through the seam's existing `Execution` classification (`CallError::execution()`), so a UI can branch on mutation safety without new types. The reference mock already models the leaf behaviour (`drop_connection`: sent ⇒ `Unknown`, unsent ⇒ `DefinitelyNot`); the kernel adds the replay-hold branch.

### K7 — Protocol-validation barrier and generation fencing (AC-6 "generations are fenced")

**Generation counter.** A monotonic `generation: u64` increments on **every** transition into a live connection (each successful gate pass). Every outstanding call is stamped with the generation it was *sent* under; every inbound frame is tagged by the driver with the generation it arrived on.

- **Fence stale replies/pushes.** A reply or push whose generation is older than the call's issuing generation (or older than the current live generation, for pushes) is **discarded** — it cannot settle a call or reach the event bus. The `peer` push carries the connection `generation` (protocol U1); a stale-generation `peer` teardown is dropped so it "cannot overwrite newer presence state." A property test replays a generation-N-1 reply after a generation-N reconnect and asserts the call is untouched.
- **Protocol-validation barrier.** `State::Ready` is emitted **only after** the generation gate passes (`v`/`sg` accepted; `hello`/handshake validated) — never before. While `Connecting`/`Interrupted`, calls are *admitted* to the bounded queue but **no operation frame is sent** until `Connected { generation }` arrives; the queue then flushes (subject to `in_flight`). If the gate returns a **terminal** refusal (`protocol_unsupported`, `storage_generation_mismatch`, `unauthenticated`), the core transitions to `State::Failed`, settles every **never-sent** call `DefinitelyNot` and every call **held for replay from a prior live generation** (`ever_sent`) `Unknown` — that call was on the wire under a generation whose gate *had* passed and may have executed there; this generation's refusal says nothing about it — and **does not** back off or retry — a terminal gate refusal "carries no auto-retry" (matching the seam's `State::Failed` doc).

### K8 — Deadlines without a wall clock (AC via Verification: timeout)

Each admitted call gets an **absolute deadline** `admitted_at + default_call_deadline` (`KernelLimits::default_call_deadline`, a `Tick` delta). The core:

- emits `Action::ArmTimer { id, at }` at admission and `Action::CancelTimer` at settlement;
- on `Input::TimerFired(id)` for a call still outstanding, settles it `CallError::Timeout` ⇒ `Execution::Unknown` (a request that timed out may still land) and cancels its in-flight slot locally — **without** claiming remote cancellation.

Time is the injected `Tick` passed to `step`; the deterministic driver advances a virtual clock, so "timeout while queued", "timeout while in-flight", and "reply arrives one tick after the deadline (late ⇒ dropped)" are exact tests. The streaming absolute-budget model (connect allowance + floor-throughput term, protocol §deadlines) is the stream layer's refinement (§K16); the request/reply deadline is the base case here.

### K9 — Cancellation at every phase; suppress late delivery without fabricating remote cancel (AC via Verification: cancellation; non-goal: dropped futures do not cancel remote execution)

A caller cancels by **dropping the returned future** or, for streams, via the seam's existing `StreamCall::cancel(execution)` / `StreamCancel`. The kernel observes cancellation through the dropped `oneshot::Receiver`:

- **Queued (never-sent):** the call is removed from the queue; it never sends. If the caller observes an error it is `Cancelled { DefinitelyNot }` (nothing executed). Byte accounting for its queued payload is released (AC-7).
- **Sent (awaiting reply):** the in-flight entry is **tombstoned** — the correlation id stays reserved (so a real late reply is matched and *discarded*, never mis-routed) but the caller's `oneshot` is gone. The kernel does **not** send any cancel frame and does **not** claim the daemon stopped (the non-goal: "dropped caller futures cancel remote execution" is false). The operation may still run to completion on the daemon; the client simply stops delivering to a caller that is no longer listening.
- **`transfer.cancel` is different and explicit.** True remote cancellation of a transfer is a *first-class operation* (`transfer.cancel` naming `transfer_op_id`), not a side effect of dropping a future. The kernel routes it like any other call; it never conflates a local drop with a remote cancel.

"Suppress late delivery to cancelled callers" is the tombstone: the id is retained until its reply/deadline resolves so the late reply is absorbed, then reclaimed. A test cancels at each phase and asserts (a) the caller sees the correct classified `Cancelled`, (b) no frame is sent for a queued cancel, (c) a subsequent real reply for a sent-then-cancelled call is dropped and strands nothing.

### K10 — Capped, jittered backoff; bounded reconnect (AC via Verification: reconnect exhaustion)

Reconnect uses **full-jitter exponential backoff**: attempt `n` waits a random duration in `[0, min(cap, base * 2^n))`, with `base`, `cap`, and `max_attempts` from `KernelLimits`. The randomness is a **deterministic in-core xorshift PRNG** seeded from `KernelConfig::jitter_seed` — no `rand`/`getrandom` (wasm-hostile) and fully reproducible in tests. The core emits `Action::Dial` after arming the backoff timer; on `Connected` it resets the attempt counter and generation-bumps; on repeated `Closed` it increments the attempt counter.

- **Exhaustion is honest.** After `max_attempts` without a live connection, the core transitions to `State::Failed` (or `Interrupted → Failed` per §K7) and settles every outstanding call: never-sent ⇒ `Disconnected { DefinitelyNot }`, sent/replayable ⇒ `Disconnected { Unknown }`. It does **not** spin forever. `DirectClient` (#173) has no dial at all and never enters this path — its resume triggers authoritative resync "without a fabricated reconnect"; the kernel expresses that by an adapter that reports `Connected`/`Interrupted` without ever emitting `Dial`.
- **Stop wins over backoff** — see §K11.

### K11 — Total stop (AC-5)

`Input::Stop` runs the §D6 ordering the seam fixed, now with the kernel's real resources:

1. **Cancel any in-progress dial/backoff:** emit `Action::CancelDial` and `Action::CancelTimer` for the backoff timer; the reconnect loop stops.
2. Transition to `State::Stopping`; **refuse new calls** (a `dispatch` after stop returns `Cancelled { DefinitelyNot }`, matching the seam's eager-refusal boundary).
3. **Drain queue and in-flight, settling each exactly once:** queued ⇒ `Cancelled { DefinitelyNot }`; sent ⇒ its real reply if one is already buffered, else `Cancelled { Unknown }`. Cancel every per-call deadline timer.
4. Transition to `State::Stopped`, emit the final `StateChanged`, and `CloseBus` (every `EventSubscription` yields `None` after the `Stopped` event).
5. Resolve the `stop()` future.

**No unbounded task or map survives (AC-7):** stop empties the queue, the in-flight ledger, the replay-hold set, and the timer set; the sans-IO core holds only bounded maps by construction (`queue_depth`, `in_flight`, plus at most `in_flight` tombstones and timers). A test asserts every internal collection is empty after `Stopped` and that a second `stop()` is idempotent.

### K12 — Bounded by construction; no unbounded growth anywhere (AC-7)

Every internal collection has a static or configured bound:

| Structure | Bound |
|---|---|
| outbound queue (count) | `queue_depth` |
| outbound queue (bytes) | `outbound_bytes` |
| sent-and-awaiting calls (the in-flight throttle) | `in_flight` |
| ledger (queued + sent + tombstones) | ≤ `queue_depth` + 2·`in_flight` — every queued call holds a ledger entry from admission, so the ledger is a composite, not `in_flight` alone |
| tombstones (cancelled-but-id-reserved) | ≤ `in_flight` (a FIFO budget: creating one past it evicts the oldest; reclaimed on reply/deadline) |
| replay-hold set | ≤ `in_flight` (only replayable sent calls) |
| armed timers | ≤ `queue_depth` + 2·`in_flight` + 1 — one deadline/reclaim timer per ledger entry (queued calls arm theirs at dispatch), plus the single backoff timer |
| per-subscription event buffer | `DEFAULT_FANOUT_CAPACITY` (existing, with `Lagged` overflow) |
| reconnect attempts | `max_attempts` |

There is no map keyed by an unbounded external input (no per-room, per-push, or per-generation accumulation). A fault test drives saturation + repeated flap + cancel churn and asserts no collection exceeds its bound across thousands of `step`s.

> **Peer payload-generation reconciliation is deliberately not the kernel's.**
> The `peer` push carries its own `generation` so a stale teardown is
> discardable (protocol §Presence — truthful data depends on upstream U1,
> and the conformance cases for it are `blocked_on_upstream`). Discarding by
> payload generation requires last-seen state per `(room, device)` — exactly
> the unbounded-external-key map this table forbids. The kernel forwards the
> push verbatim, fenced only by the transport generation (§K7); the
> presence-folding consumer, whose per-member state is already bounded by
> room membership, compares `peer.generation` when folding.

### K13 — The Transport/driver seam (what #171/#172/#173 implement)

The kernel drives an abstract transport. The seam is **object-safe and I/O-shaped**, mirroring how `ClientBackend` erases the four adapters:

```rust
// kernel/transport.rs — pub(crate) to the kernel; adapters live in #171/#172/#173.
/// One connection attempt's byte pipe, opened by the driver after the driver
/// resolves/authenticates. The kernel never dials directly — it emits
/// `Action::Dial` and the driver calls the adapter.
pub(crate) trait Transport: Send + 'static {
    /// Push one already-encoded frame toward the peer. Non-blocking; the driver
    /// owns any real back-pressure/flush. Returns `Err` if the pipe is broken,
    /// which the driver turns into `Input::Closed`.
    fn send(&mut self, frame: WireFrame) -> Result<(), TransportClosed>;
    /// The next inbound frame, or the connection's end. Adapters map their
    /// native read into this; the driver tags each with the current generation.
    fn poll_inbound(&mut self, cx: &mut Context<'_>) -> Poll<Option<WireFrame>>;
}

/// What an adapter must provide to build a kernel-backed `ClientBackend`:
/// a dialer (opens a `Transport`, performs the generation gate, yields a
/// `generation`), plus the injected clock/timer the driver uses. `DirectClient`
/// supplies a dialer that is always-ready and never reconnects.
pub(crate) trait Driver: Send + 'static { /* dial(), now() -> Tick, sleep(until: Tick), spawn-free contract */ }
```

- **`WireFrame`** is the codec (#164) byte form — the kernel handles *frames*, not JSON text, on the transport side; it converts between `ErasedCall`/`RawJson` (the seam's erased JSON text) and the codec frame at the driver boundary. (For `DirectClient`, the "frame" is an in-process typed round-trip, still behind this seam so the kernel is unchanged.)
- The kernel **defines** these traits and provides a **deterministic in-memory implementation** for tests (§K14). It implements **none** of the three real transports; those consume this seam in #171/#172/#173. This is the "no concrete sockets" non-goal, honoured.

### K14 — The deterministic test transport/driver (Verification substrate)

`tests/kernel.rs` builds the kernel over an in-memory transport whose controller (like the mock's `MockController`) drives every non-determinism explicitly:

- `deliver_reply(id, out)`, `deliver_error(id, ApiError)`, `deliver_push(ClientEvent)`, `deliver_malformed()`, `deliver_late(id)` — inbound framing under test control.
- `advance(ticks)` — the virtual clock, the sole source of time; fires due timers.
- `connect(generation)`, `interrupt(reason)`, `close(reason)` — lifecycle transitions.
- `fail_send()` — a send/close race: the transport reports the pipe broken exactly when the kernel tries to send. Every frame in the failing batch is dropped, and everything that batch had flushed classifies as sent (held if replayable, `Disconnected { Unknown }` otherwise) — a real transport cannot prove that writes queued behind a failed one never left the host, so the driver mirrors the weakest honest claim; a false `DefinitelyNot` would invite an unguarded retry, while `Unknown` only withholds a provable-negative.

No wall clock, no timers, no RNG syscall, no scheduling dependence — the same guarantees the #167 mock gives the seam, now for the kernel. This driver is the reference the four real adapters are diffed against under #175.

### K15 — Secrets never enter diagnostics (Security)

`diag.rs` centralizes every log/`Debug`/error string the kernel produces:

- Bearer tokens, browser session tickets/credentials, `client_id`, and `op_id` are **never** rendered; diagnostics name the *operation* (`op` path) and *counts/limits*, never payload bytes or identifiers that grant or correlate.
- `CallError::QueueFull` carries a `resource: &'static str` and a numeric `limit` — no payload, matching the seam's already-secret-free error taxonomy. `Disconnected`/`Cancelled`/`Timeout` carry only `Execution`.
- The `WireFrame` and `RawJson` payloads have **no** `Debug` that prints their bytes in kernel diagnostics; a `redacted` wrapper is used wherever a payload would otherwise be formatted. A test scans the kernel's diagnostic outputs (and the `Debug` impls it adds) to assert no token/op_id/payload field is rendered.

### K16 — Streaming operations: deadline/credit surface, depth deferred to #233 (scope boundary)

`file.share`/`file.read` are duplex byte streams (protocol §Byte-stream framing; owned by #233/#242/#243 daemon-side and #167's `StreamCall` surface client-side). #168 shipped the **request/reply kernel** in full; the **stream lifecycle hooks** this section describes — per-stream absolute deadline (connect allowance + floor-throughput term), stall timer, credit-bounded outbound bytes, and `OPEN/DATA/CREDIT/END/ABORT` state as a thin layer over the same core driving the existing `StreamCall` — **did not ship with #168 and are owned by #269** (they also need the transport seam to grow a binary-frame concept, which is a slice of its own). Until #269 lands, `call_stream` enters the generic request path and cannot transfer bytes through the kernel. Full byte-stream execution and its fixtures land with #233's executor; #269 wires the surface to real deadlines/credit and **does not** re-implement the framing rules, doc-pointed at #233/#242/#243.

## 6. Configuration and public surface

New **public** items (documented, `#![deny(missing_docs)]`), re-exported from `jeliya_client`:

```rust
/// The kernel's hard bounds. Every field is explicit; none defaults silently
/// to "unbounded". Chosen by the adapter/host, with documented defaults.
pub struct KernelLimits {
    /// Max admitted-but-unsent calls before `QueueFull { resource: "queue_depth" }`.
    pub queue_depth: u32,
    /// Max queued outbound payload bytes before `QueueFull { resource: "outbound_bytes" }`.
    pub outbound_bytes: u64,
    /// Max sent-and-awaiting-reply calls (a throttle; never a QueueFull).
    pub in_flight: u32,
    /// Absolute per-call deadline, as a Tick delta. Timeout ⇒ Execution::Unknown.
    pub default_call_deadline: TickDelta,
    /// Reconnect backoff base, cap, and attempt ceiling (full-jitter).
    pub backoff_base: TickDelta,
    pub backoff_cap: TickDelta,
    pub max_reconnect_attempts: u32,
}

/// Kernel construction inputs that are not limits.
pub struct KernelConfig {
    pub limits: KernelLimits,
    /// Seeds the deterministic in-core jitter PRNG. Tests fix it; a host seeds
    /// it from platform entropy at construction (passed in, never a syscall
    /// inside the library).
    pub jitter_seed: u64,
}

impl Default for KernelLimits { /* documented, conservative defaults */ }
```

- **Not exported:** `Transport`, `Driver`, `WireFrame`, the sans-IO `Core`, and every `kernel::*` internal — the kernel's machinery stays internal exactly as the seam's erasure does. Adapters (#171/#172/#173) live in this crate (or depend on a `pub(crate)`-to-them seam) and consume the internal traits; the *only* new public surface is `KernelLimits`/`KernelConfig` and a constructor `KernelBackend`-behind-`ClientHandle` builder (e.g. `ClientHandle::with_kernel(config, driver)` gated so tests and adapters can build it).
- The seam's public surface (`ClientHandle`, `CallError`, `State`, `ClientEvent`, `Dedup`, `EventSubscription`, `StreamCall`, the mock) is **unchanged**.

## 7. Implementation steps

1. **`kernel/timing.rs`** — `Tick`/`TickDelta` (logical monotonic time as a `u64` newtype), `Deadline`, `TimerId`. No `std::time`. Unit-test ordering/arithmetic (checked add; a non-representable deadline is a config error, mirroring the protocol's "not representable by a finite timer … refuses readiness").
2. **`kernel/ids.rs`** — `CorrelationId` allocator over `RequestId` (skip-outstanding, per-generation reset, wrap boundary) and the `generation: u64` counter. Property-test wrap-at-boundary and no-collision-with-outstanding.
3. **`kernel/admission.rs`** — the bounded queue with count + byte accounting; `try_admit` → `Ok(slot)` or `CallError::QueueFull { resource, limit }`; release on settle/cancel. Unit-test both limits and byte release.
4. **`kernel/inflight.rs`** — the outstanding ledger with `settle_once` (take semantics), tombstones, generation stamping. Unit-test exactly-once, duplicate-drop, late-drop, tombstone-absorb.
5. **`kernel/replay.rs`** — `ReplayPolicy` derivation from `ErasedCall.{mutating, op_id}` and the bounded replay-hold set. Unit-test the policy table (K5) and that `Never` never replays.
6. **`kernel/backoff.rs`** — full-jitter schedule + deterministic xorshift PRNG; `max_attempts` exhaustion. Unit-test the sequence for a fixed seed and the exhaustion transition.
7. **`kernel/transport.rs`** — `Transport`/`Driver`/`WireFrame` seam traits + the deterministic in-memory implementation and its controller. (No real sockets.)
8. **`kernel/core.rs`** — the sans-IO `step(Input, now) -> Vec<Action>`: admission → send-when-Ready → reply/timeout/cancel/disconnect settlement → generation fencing → backoff → stop. This is the correctness heart; it owns §K2–§K11.
9. **`kernel/diag.rs`** — redaction wrappers and the secret-free `Debug`/error rendering (K15).
10. **`kernel/mod.rs`** — `KernelBackend: ClientBackend` (the async shell binding the core to a `Driver` + `EventBus` + per-call `oneshot`), `KernelLimits`/`KernelConfig`, and the `ClientHandle::with_kernel(...)` builder. Wire `dispatch`/`subscribe`/`state`/`start`/`stop` to core inputs.
11. **`src/lib.rs`** — `mod kernel;` and re-export `KernelLimits`, `KernelConfig`. Remove the now-consumed `#[allow(dead_code)]` on `ErasedCall.{input,mutating,op_id}` (they are read by the kernel).
12. **`tests/kernel.rs` + `tests/kernel_fault.rs`** — the property/fault suite (§8), driven by the deterministic transport, one test per Verification bullet plus the AC map.
13. **`tests/boundaries.rs`** — extend the source scan: no `std::time`/`Instant::now`/`SystemTime`/`getrandom`/`rand`/`tokio` token in `src/kernel/**`; the library dependency-tree scan already covers new deps (assert it still passes with zero new runtime deps).
14. **CI** — the existing `jeliya-client` steps already run the workspace tests, the `--features mock` suite, and the native+wasm example builds; add a kernel test invocation only if the kernel tests need a feature gate (see §9). Confirm MSRV **1.91** compiles the kernel (the crate's stated MSRV).
15. **Docs** — normative surface is crate rustdoc (match `jeliya-api`/seam density). No new `docs/` page is required; the decision is already in `docs/dioxus-architecture.md` §Decision 4 (the kernel paragraph). If a reference page is later wanted it must satisfy `docs/PROFILE.md` (exactly 10 frontmatter fields, index reachability) as a separate follow-up.

## 8. Test strategy — every acceptance criterion and Verification bullet mapped

**Acceptance criteria:**

| Issue AC | Kernel mechanism | Test |
|---|---|---|
| Queue and in-flight limits are explicit and tested | `KernelLimits.{queue_depth, outbound_bytes, in_flight}` (§K2) | saturate queue ⇒ `QueueFull{queue_depth}`; oversize payload ⇒ `QueueFull{outbound_bytes}`; flood sends ⇒ assert `in_flight_count <= in_flight` never violated |
| Every accepted call settles exactly once locally | take-once ledger (§K4) | deliver a reply twice ⇒ one settle, second dropped; deliver reply after timeout ⇒ dropped; assert each `oneshot` fires once |
| Only op_id-guaranteed operations may replay; all others never auto-replay | `ReplayPolicy` table (§K5) | `mutating+op_id` replays under same `op_id` on reconnect; `mutating`-no-`op_id` and non-mutating settle, never re-sent; assert no duplicate send frame for `Never` |
| Connection loss distinguishes never-sent from may-have-executed | ledger walk by send state (§K6) | close with mixed queued/sent calls ⇒ queued `Disconnected{DefinitelyNot}`, sent `Disconnected{Unknown}` |
| Stop cancels dial/backoff and settles queued/in-flight calls | stop ordering (§K11) | stop mid-backoff ⇒ dial cancelled, no further `Dial`; every queued/in-flight settles once; bus closes; `Stopped` |
| Generation fencing rejects stale replies/state | generation stamp + fence (§K7) | reconnect (gen N), replay a gen N-1 reply and a stale `peer` teardown ⇒ both dropped, calls/state untouched |
| No failure mode leaves an unbounded task or map | bounded-by-construction (§K12) | saturation + flap + cancel churn over thousands of steps ⇒ every collection ≤ its bound; after stop, all empty |

**Verification bullets (property/fault):**

- **queue saturation** — fill `queue_depth`/`outbound_bytes`, assert `QueueFull`, then drain and re-admit.
- **timeout** — timeout while queued, while in-flight, and a reply one tick late (dropped).
- **cancellation at each phase** — cancel queued (no send), cancel sent (tombstone, no remote-cancel claim, late reply absorbed), cancel a `StreamCall`.
- **send/close races** — `fail_send()` exactly at flush; `Closed` arriving between admit and send; `Connected` racing a pending stop.
- **decoder failure** — malformed inbound frame dropped (strands nothing); a parsable reply with undecodable `out` ⇒ `LocalError::DecodeReply` at the handle.
- **reconnect exhaustion** — `max_reconnect_attempts` closes ⇒ `Failed`, all outstanding settled honestly; deterministic backoff sequence for a fixed seed.
- **stop** — from `Idle`, `Connecting`, `Ready` (with in-flight), and mid-backoff; idempotent second stop; all maps empty.

**Determinism guard:** every kernel test uses only the in-memory transport + virtual clock + fixed `jitter_seed`; a lint/test asserts no kernel test constructs a real clock, timer, or thread. The behaviour is identical on native and `wasm32-unknown-unknown` (the example/build gate already proves the crate links on wasm; the kernel adds no wasm-hostile dependency).

## 9. CI changes (`.github/workflows/ci.yml`, Rust job)

The seam's CI steps already cover most of this crate (workspace test at 1.96.0, the `--features mock` suite, native+wasm example builds, and the MSRV 1.91.0 job). For #168:

1. `cargo test --locked --workspace` (existing) already compiles and runs `tests/kernel.rs`/`tests/kernel_fault.rs` **if** they need no feature gate. **Recommendation:** keep the kernel and its tests **default-on** (no feature) so the plain workspace test covers them — the kernel is production code, not test scaffolding, unlike the `mock` feature. (If the deterministic *test transport* must ship in the library for adapters to reuse, gate only that fixture behind a `test-transport` feature and run one extra `cargo test -p jeliya-client --features test-transport` step.)
2. **MSRV:** confirm the kernel compiles under **1.91.0** (the crate's `rust-version`). No `let`-else-in-const, no edition-2024-only syntax, no std API newer than 1.91. The existing MSRV job gates this.
3. `boundaries.rs` (run by the workspace test) gains the kernel source scan (§7.13); no new CI step needed.
4. No new toolchain, target, or runner capability is required — the kernel adds no dependency and no wasm test run (AC-6 asks for compilation, already gated).

## 10. Risks and mitigations

- **Scope creep into the adapters (#171/#172/#173).** Building a real WebSocket/native/direct transport here would violate the "no concrete sockets" non-goal. *Mitigation:* #168 defines the `Transport`/`Driver` seam and ships **only** the deterministic in-memory implementation; the real transports are separate issues that consume it.
- **Scope creep into the stream executor (#233).** Re-implementing `OPEN/DATA/CREDIT/END/ABORT` framing here would duplicate #233. *Mitigation:* §K16 keeps streaming to deadline/credit hooks over the existing `StreamCall`, doc-pointed at #233/#242/#243.
- **A hidden wall clock or RNG syscall.** A stray `Instant::now()`/`getrandom` would break determinism and wasm. *Mitigation:* sans-IO core with injected `Tick` + in-core seeded PRNG, plus the `boundaries.rs` source scan.
- **Over-claiming replay safety.** Auto-replaying a mutation without a dedup guarantee would double-execute. *Mitigation:* the `ReplayPolicy` table (§K5) makes `Never` the default and `ReplayableUnderOpId` the only exception, gated on `mutating && op_id`; a test asserts no `Never` call is ever re-sent.
- **Fabricating remote cancellation.** Treating a dropped future as a remote cancel would lie. *Mitigation:* §K9 tombstones locally and sends no cancel frame; only `transfer.cancel` cancels remotely, as an explicit operation.
- **Correlation-id collision after wrap.** *Mitigation:* skip-outstanding allocator + per-generation reset + generation fencing (§K3/§K7); a boundary property test.
- **Unbounded growth under flap/cancel churn.** *Mitigation:* every collection has a static/configured bound (§K12); a stress fault test asserts the bounds hold and that stop empties everything.
- **Seam surface drift.** If the kernel needed a new `ClientBackend` method, #167's "sufficient without a breaking change" claim would fail. *Mitigation:* the kernel is designed to fit the existing trait exactly; if a gap is found, that is a reviewable finding against #167, surfaced in the PR, not a silent seam change.

## 11. Non-goals (from the issue, restated)

- **Concrete sockets** — any real WebSocket, native transport, or in-process `jeliya-core` binding (#171/#172/#173).
- **Automatic replay without an explicit, tested v2 dedup guarantee** — only `ReplayableUnderOpId` replays; everything else settles.
- **Claiming dropped caller futures cancel remote execution** — local suppression only; remote cancel is `transfer.cancel`.
- **Resynchronizing room state** — the kernel fences generations and emits `ResyncRequired`/`Gap`; reconciling the room is a separate coordinator (#169 owns the single resync path; the kernel only surfaces it).
- **The public seam surface** — unchanged; #168 adds internal machinery and a small `KernelLimits`/`KernelConfig` surface only.
- **Full byte-stream framing execution** (#233/#242/#243 daemon; §K16 client hooks only).
- **`PlatformServices`** (#174) — files, persistence, lifecycle, URLs; injected separately, not the kernel's concern.

## 12. Open questions

1. **Kernel-backed backend location vs. the adapters.** Should `KernelBackend` and the `Transport`/`Driver` seam live in `jeliya-client` (this spec's choice, so #171/#172/#173 depend on the seam crate and add only their transport), or in a sibling `jeliya-client-kernel` crate? Recommend **in `jeliya-client`** to reuse `EventBus`/`RawJson`/`ErasedCall` without a public boundary; confirm the adapters can reach a `pub(crate)`-to-them seam (a `#[doc(hidden)] pub` "adapter" module or a dedicated feature).
2. **Non-mutating re-issue on reconnect.** A read that was in-flight when the connection dropped is *safe* to re-issue (no side effect). The strict default here is `Never` (settle `Disconnected{Unknown}` and let the caller decide). Should the kernel instead transparently re-issue non-mutating ops on reconnect for resilience? Recommend keeping **`Never`** for #168 (honest, matches the mock reference) and revisiting under #175's parity suite.
3. **Replay eligibility source of truth.** Derive `ReplayableUnderOpId` from the `mutating && op_id.is_some()` heuristic (this spec), or from an explicit per-operation replay table exported by `jeliya-api` (the protocol's "`op_id` deduplicated" row of 13)? The heuristic is safe (only ever under-replays) but a table is exact. Recommend adding a `const REPLAY: ReplayClass` to `jeliya_api::Operation` **iff** review wants exactness; otherwise the heuristic, documented.
4. **Deadline configurability per call.** The seam's `call<O>` takes no deadline; the kernel applies `default_call_deadline`. Do we need a per-call deadline override (e.g. a `call_with_deadline`), or is one configured default plus `stop`/cancel sufficient for M2? Recommend the single default for #168; a per-call override is an additive seam change if a real UI flow needs it.
5. **Where the real clock/RNG entropy enters.** The kernel takes `Tick` and a `jitter_seed`; the *driver* owns the real clock and the host supplies the seed. Confirm that seeding jitter from a host-provided `u64` (not an in-library syscall) is acceptable for the security posture (it is not a credential; it only decorrelates reconnect storms), and that `PlatformServices` (#174) or the adapter is the right place to source it.
6. **`test-transport` feature vs. always-on.** Should the deterministic in-memory transport ship in the library (feature-gated) so #171/#172/#173 and #175 can reuse it as a reference, or stay in `tests/`? Recommend a `test-transport` feature (default-off) so the parity suite (#175) can drive the real adapters against the same controller.

## 13. Assumptions

- `crates/jeliya-client` (#167) is landed at its current shape: `ClientBackend`, `ErasedCall` (with `op`/`mutating`/`op_id`/`input`), `RawJson`, `EventBus`, `CallError`/`Execution`/`LocalError`, `State`, `ClientEvent`, `Dedup`, `StreamCall`, and the mock — all present and used here without modification to their public surface.
- `crates/jeliya-codec` (#164) provides the `WireFrame` byte form and request routing the driver boundary converts to/from; if #164's surface differs, the driver-boundary conversion is the only place that adapts, not the core.
- `jeliya-api`'s `Operation::{PATH, MUTATING, Output}`, `RequestId` (`MAX_REQUEST_ID = 2^53-1`), `OpId`, and `ApiError` (incl. `ResyncRequired`, `OpIdConflict`, `ProtocolUnsupported`, `StorageGenerationMismatch`, `Unauthenticated`) are stable at their current shapes.
- The protocol's idempotency/retry model — ledger keyed on `(session principal, op_id)`, the 13-operation `op_id`-deduplicated set, `stream.*` scoped-to-connection, the connection generation that "MUST NOT reset on a new connection, a new session, or a reconnect" (the *pairing-code budget*; the *connection* generation, by contrast, increments per connection and is what fences stale state) — is authoritative and this kernel implements against it.
- The crate's MSRV is **1.91** and the CI toolchains are 1.96.0 (primary) + 1.91.0 (MSRV) with `wasm32-unknown-unknown` available; the kernel adds no dependency and no wasm-hostile code.
- The orchestrator performs all git/gh/PR actions; this document is the only artifact the planning phase produces, and no production code is written for #168 by the planning phase.
