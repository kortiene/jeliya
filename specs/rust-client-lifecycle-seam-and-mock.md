# Spec — Lifecycle-aware Rust client seam and deterministic mock (#167)

- **Issue:** kortiene/jeliya#167 — `[Rust][Client]: Define the lifecycle-aware client seam and deterministic mock`
- **Program:** #156 (Dioxus clean-slate). **Milestone:** M2 (client runtime and platform adapters).
- **Records the decision in:** `docs/dioxus-architecture.md` §"Decision 4 — one seam, four adapters, one platform boundary".
- **Depends on (both landed):** #157 (architecture record) and #163 (`crates/jeliya-api`, the Iroh-free typed API — present in the workspace today).
- **Blocks / is the entry point for:** #168 (transport-independent kernel) → #169/#171/#172/#173 → the four adapters (`WsWeb`, `WsNative`, `DirectClient`, deterministic mock) and #174 (`PlatformServices`).
- **Owner role:** core maintainer (per the architecture layering table).
- **Status of this document:** planning/spec only. **No production code is to be written for this issue by the planning phase.**

> Where this spec and `docs/protocol-v2.md` or `docs/dioxus-architecture.md` disagree, those records
> are authoritative and this spec has a bug — say which in the PR, exactly as the architecture record
> requires of every slice that tests against it.

---

## 1. Outcome

Deliver the **single UI-facing Rust client contract** that shared Dioxus components use, with:

1. **compile-time request/output pairing** (a call cannot be made without knowing its reply type),
2. an **observable, honest lifecycle** (start, stop, states, state transitions),
3. **multi-consumer events** where no consumer can silently steal another's pushes,
4. an **error model that separates wire failures from queue / timeout / cancel / gap / local failures** and **preserves whether a failed mutation may have executed**, and
5. a **deterministic mock backend** that scripts responses, errors, push-before-response, gaps, cancellation, and shutdown.

Verification is a minimal shared Dioxus component compiled against the mock for **both** `wasm32-unknown-unknown` and a native target, with **no per-component `cfg` logic**.

This becomes the sole UI client contract for the new stack; legacy clients are not runtime fallbacks (clean-slate cutover).

## 2. What this issue is, and what #168 is

The architecture splits the client into two adjacent slices:

- **#167 (this issue) — the seam.** "One cloneable concrete UI handle, preferred over an object-unsafe generic trait, keeping backend erasure internal. It models `Push`, `StateChanged`, `Gap`, and `ResyncRequired`. Calls are compile-time paired with their outputs; multiple consumers cannot silently steal each other's pushes; stop settles all accepted work and closes event streams."
- **#168 — the kernel below it.** Transport-independent: bounded queues where `QueueFull` is visible rather than absorbed; connection loss distinguishing never-sent work from work that may have executed; only operations with an explicit, tested v2 dedup guarantee may replay; generations are fenced.

**Consequence for scope.** #167 owns the *public contract and the types the kernel's decisions flow through* (the `State` model, `ClientEvent`, `CallError` with a may-have-executed classification, the object-safe internal backend trait, and the reference mock). #168 fills in the *internal machinery* (real bounded queues, generation fencing, replay ledger) behind the same `ClientBackend` trait this issue defines. The mock is the reference behavior both are measured against.

Do not implement bounded-queue backpressure accounting, generation fencing, or replay dedup in this issue beyond (a) defining the `CallError::QueueFull` variant and its `Execution` classification, and (b) letting the mock *script* a `QueueFull`/`Cancelled`/`Disconnected` outcome so the seam types are exercised. The kernel that produces them for real is #168.

## 3. Owning crate and layout

Add one new workspace crate, `crates/jeliya-client`, and add it to the single `members` line in the root `Cargo.toml` (the lane convention: every new-crate issue edits that one line).

```
crates/jeliya-client/
  Cargo.toml
  src/
    lib.rs          # crate docs, re-exports, boundary invariants (mirror jeliya-api/src/lib.rs)
    handle.rs       # ClientHandle (the cloneable seam), typed call<O>, subscribe, state, start, stop
    backend.rs      # object-safe ClientBackend trait + ErasedCall/ErasedReply (internal)
    event.rs        # ClientEvent, EventSubscription (multi-consumer stream), State
    error.rs        # CallError, Execution, LocalError
    stream.rs       # streaming-call surface for file.share/file.read (shape only; depth deferred)
    mock/
      mod.rs        # MockBackend + MockController + MockScript builder (feature = "mock")
  examples/
    shared_component.rs   # a Dioxus RSX component used by both target compiles (feature = "mock")
  tests/
    boundaries.rs   # dependency-tree + no-serde_json::Value-in-public-API assertions (CI)
    seam.rs         # AC-mapped behavior tests driven by the mock
```

**Boundary invariants (asserted, not merely intended):**

- Crate-level `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]`, mirroring `jeliya-api`.
- The **library target** must not depend on Iroh, any WebSocket crate, native transports, `tao`/`wry`, **or Dioxus**. Dioxus appears only as an `examples`/`dev` dependency for the verification component. This keeps the seam transport-independent (architecture: the seam "must not depend on a specific transport; backend erasure stays internal") and keeps `ClientHandle` and `PlatformServices` injected separately (#174), never entangled.
- No `serde_json::Value`, `serde_json::value::Value` token in any **public** signature (mirror `jeliya-api/tests/boundaries.rs`). The internal erased payload is a JSON *text* newtype (`RawJson(Box<str>)`), so no `Value` token appears at all and the existing source-scan style test passes unmodified.
- No `tokio`, `std::time`, `tokio::time`, or `wasm-bindgen`-specific timing in the library. Concurrency primitives must be executor-agnostic and `wasm32-unknown-unknown`-safe (see §9).

## 4. Design decisions

### D1 — A cloneable concrete handle over an object-safe backend

`ClientHandle` is a concrete `#[derive(Clone)]` struct wrapping `Arc<dyn ClientBackend>`. Cloning is cheap (an `Arc` bump); every clone shares one backend, so `handle.clone()` handed to N components is one client, not N. This is the exact shape the architecture prefers over "an object-unsafe generic trait", and it is what Dioxus context needs: a `Clone` value stored once and read by many components.

The **typed convenience calls live on the concrete handle as generic methods**; the **backend trait is object-safe** (no generic methods, returns boxed futures). This is how "backend erasure stays internal" is satisfied without losing pairing (§D2, §D8).

### D2 — Calls are compile-time paired with outputs (AC-1)

The seam re-uses `jeliya_api::Operation`, which already binds `Operation::Output` and `Operation::PATH`. The one paired entry point is:

```rust
impl ClientHandle {
    /// Invoke one approved v2 operation. The reply type is `O::Output`,
    /// bound at compile time by `jeliya_api::Operation` — there is no
    /// unpaired downcast and no generic "call by string" that returns an
    /// untyped value.
    pub fn call<O: Operation>(
        &self,
        input: O,
        dedup: Dedup,
    ) -> impl Future<Output = Result<O::Output, CallError>> + '_;
}

/// How a request participates in the envelope `op_id` dedup ledger.
/// `op_id` is an *envelope* field, never an argument to `in` — matching the
/// protocol's "request deduplication lives in the envelope".
pub enum Dedup {
    /// No `op_id`. The default for non-mutating reads and for callers that
    /// accept no cross-reconnect replay.
    None,
    /// A caller-chosen, stable key. Required to make a mutation replay-safe
    /// across a reconnect; also the value `transfer.cancel` must be able to
    /// name later.
    Key(OpId),
}
```

`call<O>` is *the* contract. Hand-written per-operation convenience wrappers (e.g. `room_create`, `message_send`, `room_timeline`) MAY be added for ergonomics, but each is a thin forwarder to `call::<O>` and none may erase the output type. The non-goal — "a generic method that loses request/output pairing" — is avoided precisely because `call<O>` returns `O::Output`, not an untyped value.

`Dedup::Key` on a non-deduplicated operation is accepted and ignored (the protocol: `op_id` "is accepted on every operation and ignored by those that do not deduplicate … never `unrecognised_field`"). `transfer.cancel` names another request's `op_id` in its `in.transfer_op_id` request field (already modeled in `jeliya-api`), not through `Dedup`.

### D3 — Multi-consumer events, no silent stealing (AC-3)

`ClientHandle::subscribe()` returns an **independent** `EventSubscription` each time it is called. Every active subscription observes **every** event (a fan-out / broadcast), so two components subscribing cannot starve each other. This is the opposite of a single shared `mpsc` receiver, where whichever task polls first *consumes* the item — the "silent stealing" the AC forbids.

- **Replies never travel on the event stream.** A reply is delivered to exactly one place: the future returned by `call<O>` (a `oneshot`). Pushes travel on the fan-out. This mirrors the wire rule "a push carries `t` and never `id`; a reply carries `id` and never `t`", and it makes reply-vs-push confusion unrepresentable at the seam.
- A subscription created *after* an event does not receive that past event; pushes are live. Components that need history call `room.timeline` / `stream.resync`, not the event stream.
- **Local fan-out overflow is surfaced, never dropped silently.** If a slow consumer lags a bounded fan-out buffer, that consumer receives `ClientEvent::Lagged { dropped, .. }` — a *local* signal distinct from a protocol `Gap`. A component that sees `Lagged` knows it missed live pushes and must reconcile (typically by issuing `stream.resync`). Dropping pushes and saying nothing is exactly the honesty failure the clean-slate generation exists to remove.

### D4 — An honest, observable client state model (AC-2)

```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    /// Never started, or fully stopped-then-idle. No backend activity.
    Idle,
    /// A connect/activation attempt is in progress; not yet usable.
    Connecting,
    /// Usable: the backend has completed whatever validation its transport
    /// requires and will accept calls.
    Ready,
    /// Was Ready, lost usability (connection dropped / transport interrupted)
    /// and is recovering. Calls may be refused with a delivery-classified error.
    Interrupted,
    /// stop() is draining accepted work; new calls are refused.
    Stopping,
    /// stop() completed: all accepted work settled, all event streams closed.
    Stopped,
    /// A terminal, non-recoverable failure (e.g. a generation/protocol gate
    /// refusal the client cannot retry past). Carries no auto-retry.
    Failed,
}
```

- `ClientHandle::state()` returns a snapshot; **transitions** are delivered as `ClientEvent::StateChanged { from, to }` so a component can react without polling.
- Adapters map their **honest** transport lifecycle into these states **without leaking `cfg` into components** (AC-6). The mapping is the adapter's job (#168+), but the seam fixes the vocabulary now:

  | Adapter | Honest lifecycle mapped into `State` |
  |---|---|
  | deterministic mock | fully scriptable; the reference sequence |
  | `WsWeb` | `Connecting` while authenticating; `Ready` emitted **only after protocol validation** (never before `hello`) |
  | `WsNative` | resolver runs per attempt; `Ready` only after a verified-loopback dial and `hello` |
  | `DirectClient` | `Idle → Ready` with **no fabricated `Connecting`/`Interrupted` round-trip**; on resume it emits `ResyncRequired`, never a fake reconnect (architecture: "Pretending `DirectClient` reconnects is an explicit non-goal") |

  The point of one enum is that a component renders `State` identically regardless of which adapter is behind the handle — the honest differences are *which* transitions occur, not *which type* the component branches on.

### D5 — `ClientEvent`: Push, StateChanged, Gap, ResyncRequired (models the four named signals)

The architecture names four signals the seam "models": `Push`, `StateChanged`, `Gap`, and `ResyncRequired`. On the wire, a `gap` is a `Push` and `resync_required` is an `ApiError` reply to `stream.resync`; the seam **lifts both to first-class events** so components get a clean four-way model instead of re-deriving them:

```rust
pub enum ClientEvent {
    /// A lifecycle transition (D4).
    StateChanged { from: State, to: State },
    /// A live room push that is neither a gap nor a resync: the wire
    /// `event`, `peer`, and `transfer` pushes, unchanged from jeliya-api.
    Push(RoomPush),
    /// A position discontinuity for one room (the wire `gap` push, lifted).
    Gap { room_id: RoomId, from_pos: u64, to: GapTo, reason: GapReason },
    /// The authoritative recovery instruction: discard back to `from_pos`
    /// and re-read. Synthesized by the kernel when it detects a discontinuity
    /// it cannot bridge, or received as the `resync_required` reply.
    ResyncRequired { room_id: RoomId, from_pos: u64 },
    /// LOCAL fan-out overflow (D3) — distinct from a protocol Gap. This
    /// consumer missed live pushes and must reconcile.
    Lagged { room_id: Option<RoomId>, dropped: u64 },
}

/// The non-gap pushes, reusing jeliya-api's `Push` arms verbatim.
pub enum RoomPush {
    Event { room_id: RoomId, event: Event },
    Peer  { room_id: RoomId, subject_id: SubjectId, device_id: DeviceId, link: Link, generation: u64 },
    Transfer { transfer_op_id: OpId, transferred_bytes: u64, total: ByteTotal },
}
```

`RoomPush` reuses the `jeliya_api::Push` payloads (do not redefine the shapes; construct `RoomPush` from `Push` by mapping `Push::Gap` into `ClientEvent::Gap`). Keep `Gap` and `ResyncRequired` as first-class arms because the architecture and the issue both enumerate them, and because a component's rendering decision for "you missed data" is different from "here is new data."

### D6 — Start / stop settle semantics (AC-2, AC-4)

```rust
impl ClientHandle {
    /// Begin connecting/activating. Idempotent: calling start on a client
    /// already Connecting/Ready is a no-op. Transitions Idle → Connecting.
    pub fn start(&self);

    /// Graceful shutdown. Resolves only after every accepted call has
    /// settled and every event stream is closed.
    pub fn stop(&self) -> impl Future<Output = ()> + '_;
}
```

`stop()` MUST, in order:

1. Transition to `Stopping` and **refuse new calls**: any `call<O>` begun after `stop()` resolves with `CallError::Cancelled { execution: Execution::DefinitelyNot }` (it never left the seam).
2. **Settle all accepted work.** Every already-accepted in-flight call future resolves — never hangs — to either its real terminal reply (if one is already available) or a `CallError::Cancelled { execution }` whose `execution` **preserves whether the request may have executed** (`DefinitelyNot` if still queued and unsent, `Unknown` if it had been sent to the transport). "Settles" means *resolves to a definite outcome*, not *completes successfully*.
3. **Close all event streams:** every live `EventSubscription` yields `None` (end of stream) after a final `StateChanged { to: Stopped }`.
4. Transition to `Stopped` and resolve the `stop()` future.

This is the AC verbatim — "Stop settles all accepted work and closes event streams" — expressed as a testable ordering. It is also why `stop` is `async`: a synchronous stop cannot truthfully say accepted work settled.

### D7 — The error model separates failure classes and preserves may-have-executed (Security & correctness)

This is the correctness heart of the issue: *"Separate wire errors from queue/timeout/cancel/gap/local failures. Preserve whether a failed mutation may have executed."*

```rust
/// Whether a failed mutation may have taken effect on the daemon.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Execution {
    /// The request provably never executed (never sent, or the daemon
    /// answered a typed refusal that commits no effect).
    DefinitelyNot,
    /// The request may or may not have executed; the client cannot tell.
    /// A retry is only safe under an explicit, tested dedup guarantee (#168).
    Unknown,
    /// The request provably executed (the daemon replied; only the reply was
    /// unreadable locally).
    Definitely,
}

pub enum CallError {
    /// The daemon reached operation semantics and answered a typed protocol
    /// error. Carries the full `jeliya_api::ApiError`. `execution()` is
    /// `DefinitelyNot`: a typed refusal commits no effect (a pre-END stream
    /// failure produces no `file.share` event, `op_id_conflict` performed no
    /// second effect, etc.).
    Wire(ApiError),
    /// The bounded outbound queue refused the request before it was sent
    /// (#168's backpressure). `execution()` = `DefinitelyNot`.
    QueueFull { resource: &'static str, limit: u64 },
    /// No reply arrived within the call deadline. `execution()` = `Unknown`.
    Timeout,
    /// The caller cancelled, or stop() drained the request. `execution()`
    /// is the carried value: `DefinitelyNot` if never sent, `Unknown` if sent.
    Cancelled { execution: Execution },
    /// The connection was lost before a reply. `execution()` is the carried
    /// value: `DefinitelyNot` if the request never left the queue, `Unknown`
    /// if bytes went out.
    Disconnected { execution: Execution },
    /// A client-local failure that never involved a daemon verdict.
    Local(LocalError),
}

pub enum LocalError {
    /// The request could not be encoded. `execution()` = `DefinitelyNot`.
    EncodeRequest,
    /// A reply was received but could not be decoded into `O::Output`.
    /// `execution()` = `Definitely` — the daemon *did* run the operation.
    DecodeReply,
    /// A backend-internal invariant failed (bug surface). `execution()` =
    /// `Unknown`, because a bug near the send/receive boundary cannot claim
    /// otherwise.
    Backend,
}

impl CallError {
    /// Total function: every variant classifies its may-have-executed state.
    pub fn execution(&self) -> Execution { /* per the rules above */ }
    /// Convenience: the wire error, if this was a daemon verdict.
    pub fn as_wire(&self) -> Option<&ApiError>;
}
```

Rules, stated so a test can assert them:

- **Wire vs everything else.** `Wire(ApiError)` is *only* a daemon verdict. Queue pressure, timeout, cancellation, disconnect, and local encode/decode are **never** dressed up as an `ApiError`. A component can therefore branch on "the daemon said no (localizable, actionable)" vs "the client could not complete the round trip."
- **`gap`/`resync` are not `CallError`s.** They are events (§D5). The one exception is the *wire reply* `resync_required` to `stream.resync`, which arrives as `Wire(ApiError::ResyncRequired{..})` because it is that operation's answer; the kernel also re-emits it as `ClientEvent::ResyncRequired` for subscribers.
- **`execution()` is total and preserved through `stop()` and disconnect.** This is the "preserve whether a failed mutation may have executed" AC. It is the seam-level type that #168's "connection loss distinguishes never-sent work from work that may have executed" flows through.

### D8 — Internal erasure keeps the backend object-safe (backend erasure stays internal)

```rust
// backend.rs — NOT exported.
pub(crate) trait ClientBackend: Send + Sync {
    fn dispatch(&self, call: ErasedCall) -> BoxFuture<'static, Result<RawJson, CallError>>;
    fn subscribe(&self) -> EventSubscription;
    fn state(&self) -> State;
    fn start(&self);
    fn stop(&self) -> BoxFuture<'static, ()>;
}

pub(crate) struct ErasedCall {
    pub op: &'static str,       // O::PATH
    pub mutating: bool,         // O::MUTATING
    pub op_id: Option<OpId>,    // envelope-level, from Dedup
    pub input: RawJson,         // serialized `in`
}

/// A JSON text blob. A *text* newtype, not `serde_json::Value`, so the
/// no-`Value`-in-source rule holds and the erased boundary carries nothing
/// but bytes.
pub(crate) struct RawJson(Box<str>);
```

`ClientHandle::call<O>` does the type work at the edges and hands the backend only erased bytes:

1. Serialize `input: O` → `RawJson` (`EncodeRequest` on failure ⇒ `DefinitelyNot`).
2. `backend.dispatch(ErasedCall { op: O::PATH, mutating: O::MUTATING, op_id, input })`.
3. On `Ok(raw)`, deserialize `raw` → `O::Output` (`DecodeReply` on failure ⇒ `Definitely`).
4. On `Err(CallError)`, forward unchanged.

This is why the public API keeps pairing (`call<O> -> O::Output`) while the backend stays object-safe (no generics, boxed futures) — the erasure is entirely inside the crate. `serde_json` is a normal (non-`dev`) dependency of this crate; it is confined to this boundary. The `WsWeb`/`WsNative` adapters (#168+) may later replace `RawJson` round-tripping with #164's codec byte form; `DirectClient` pays one serialize/deserialize hop rather than executing core directly (executing core directly here is an explicit non-goal). None of that changes the public seam.

### D9 — Executor-agnostic, `wasm32` + native, no scattered `cfg` (AC-6)

- The library returns futures (`impl Future` on the handle, `BoxFuture` inside the backend) and **never spawns**. Any background pumping a real transport needs is the adapter's concern (#168); where a spawn is genuinely required it lives behind a single injected `Spawn`/`PlatformServices` seam or one `#[cfg]`-gated `rt` module — **never** in a component and never sprinkled across the seam.
- Channels: an **executor-agnostic, wasm-safe** broadcast for the event fan-out (e.g. `async-broadcast`) and `futures::channel::oneshot` for reply correlation. Rationale: these compile and run on `wasm32-unknown-unknown` and on native without a runtime feature flag, so the seam needs no `tokio` and no `cfg(target_arch)` fork. (`tokio::sync::broadcast` also compiles on wasm, but pulling `tokio` into the seam invites accidental `rt`/`time` use; prefer the runtime-free crate.)
- No wall clock in the seam. Deadlines that produce `CallError::Timeout` are the kernel/adapter's concern; the mock triggers `Timeout` only when scripted.
- The **verification component contains zero `cfg`** (AC-6). It takes a `ClientHandle`, subscribes, and renders `State` + the latest `ClientEvent`. `cfg` differences, if any, are confined to the adapter construction the *host* does, never the shared component.

### D10 — The deterministic mock (AC-5)

The mock is the reference backend shipped **with** the seam (architecture: "in-process fixture, shipped with the seam … it is the reference behavior"), behind a default-off `mock` feature that the examples and tests enable.

Determinism means: **no wall clock, no timers, no RNG, no reliance on task-scheduling order.** Given the same script and the same sequence of caller actions, the observable sequence of replies and events is identical on wasm and native. This is achieved with a **controller-driven** model: scripted calls stay pending until the test advances the mock, so ordering (including push-before-response) is explicit rather than a race.

```rust
// mock/mod.rs (feature = "mock")
pub struct MockScript { /* builder */ }
impl MockScript {
    pub fn new() -> Self;
    /// Program the outcome of the next call to `op` (matched by wire name and
    /// occurrence order). `Reply` may carry a typed Ok output or an ApiError.
    pub fn on(self, op: &'static str, program: Program) -> Self;
    /// Build the backend plus a controller the test drives.
    pub fn build(self) -> (ClientHandle, MockController);
}

pub enum Program {
    /// Resolve the call with a typed success output (serialized like the wire).
    ReplyOk(RawOut),
    /// Resolve the call with a wire error → CallError::Wire.
    ReplyErr(ApiError),
    /// Emit these events to all subscribers, THEN resolve with the reply.
    /// Proves push-before-response ordering deterministically.
    EmitThenReply { before: Vec<ClientEvent>, reply: Box<Program> },
    /// Never resolve on its own; only cancellation or stop() settles it.
    /// The `sent` flag fixes the Execution the resulting Cancelled/Disconnected
    /// error must carry.
    Hang { sent: bool },
    /// Resolve with a client-side classification (QueueFull, Timeout, ...).
    Local(CallError),
}

pub struct MockController {
    /// Deliver the next pending scripted step (a reply and/or its `before`
    /// events). Deterministic: one step per call.
    pub fn deliver_next(&self) -> bool;
    /// Inject an out-of-band event (Gap, ResyncRequired, Peer, Lagged, ...).
    pub fn emit(&self, event: ClientEvent);
    /// Drive lifecycle transitions the script asserts (e.g. Interrupted).
    pub fn set_state(&self, state: State);
    /// Force a connection loss: settle all sent-and-pending calls as
    /// Disconnected{Unknown} and unsent ones as Disconnected{DefinitelyNot}.
    pub fn drop_connection(&self);
}
```

The mock MUST make every AC-5 scenario expressible and deterministic:

- **responses** — `Program::ReplyOk`.
- **errors** — `Program::ReplyErr(ApiError)` → `CallError::Wire`.
- **push-before-response** — `Program::EmitThenReply`: a subscriber deterministically observes the push(es) *before* the caller's future resolves.
- **gaps** — `controller.emit(ClientEvent::Gap { .. })`; and a scripted `stream.resync` may reply `ApiError::ResyncRequired`.
- **cancellation** — `Program::Hang { sent }`; the test cancels (drops the call future or calls a cancel path) and asserts `CallError::Cancelled { execution }` with the `sent`-derived `Execution`.
- **shutdown** — `handle.stop()` on the mock settles every pending scripted call (§D6), emits `StateChanged` to `Stopping` then `Stopped`, and ends every subscription; a test asserts no future hangs and every stream yields `None`.

### D11 — Streaming operations: surface defined, depth deferred

`file.share` and `file.read` are duplex byte-stream operations (protocol §Byte-stream framing), not simple request→reply. This issue **defines the seam surface** so it is not painted into a corner, but the executor is out of scope here (daemon side is #233/#242/#243; client kernel is #168):

- A `call_stream<O>` entry point returns a `StreamCall<O>` that exposes (a) a cancel path mapping to `CallError::Cancelled { execution }` with `Execution` preserved, and (b) a terminal `Result<O::Output, CallError>`. Its credit/OPEN/END/ABORT semantics are the kernel's (#168) to implement against the protocol.
- **This issue's mock is not required to drive full byte-stream framing.** It must only prove the scenarios AC-5 names (responses, errors, push-before-response, gaps, cancellation, shutdown), plus that a `call_stream` cancellation yields a delivery-classified `Cancelled`. Full stream fixtures live with #233's executor and #168's kernel.

Keep this section minimal in code — a defined type and doc comments pointing at the owning issues — so #167 does not absorb streaming scope.

## 5. Implementation steps

1. **Scaffold the crate.** Create `crates/jeliya-client` with `Cargo.toml` (deps: `jeliya-api` path dep, `serde`, `serde_json`, `futures`, `async-broadcast`, `thiserror`; dev/example: `dioxus` core, `futures-executor` or a tiny block-on for native tests, `wasm-bindgen-test` optional). Add the crate to root `Cargo.toml` `members`. Add `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]` to `lib.rs` with a boundary-invariant module doc mirroring `jeliya-api/src/lib.rs`.
2. **`error.rs`** — `Execution`, `CallError`, `LocalError`, and a total `CallError::execution()` + `as_wire()`. Unit-test the classification of every variant (§D7).
3. **`event.rs`** — `State`, `ClientEvent`, `RoomPush`, a `From<jeliya_api::Push>`-style mapper that routes `Push::Gap` → `ClientEvent::Gap` and the rest → `RoomPush`, and `EventSubscription` (a wrapper over the fan-out receiver implementing `futures::Stream<Item = ClientEvent>`).
4. **`backend.rs`** — the `pub(crate)` `ClientBackend` trait, `ErasedCall`, `RawJson`, `RawOut`. Object-safety is a compile-time property; add a `fn _assert_object_safe(_: &dyn ClientBackend) {}` guard.
5. **`handle.rs`** — `ClientHandle { inner: Arc<dyn ClientBackend> }`, `#[derive(Clone)]`; `call<O>` (serialize → dispatch → deserialize with the §D8 error mapping), `Dedup`, `subscribe`, `state`, `start`, `stop`. Add a small number of hand-written convenience wrappers over `call::<O>` for the highest-traffic operations (`room_create`, `room_list`, `message_send`, `room_timeline`, `stream_subscribe`, `stream_resync`) — each a one-line forwarder.
6. **`stream.rs`** — the `call_stream<O>` / `StreamCall<O>` surface (D11), doc-commented as depth-deferred to #168/#233.
7. **`mock/mod.rs`** — `MockScript`, `Program`, `MockController`, and `MockBackend: ClientBackend`. Implement the controller-driven, clock-free resolution (§D10). Feature-gate under `mock`.
8. **`examples/shared_component.rs`** — a `#[component] fn RoomStatus(handle: ClientHandle) -> Element` (Dioxus core RSX) that subscribes, tracks `State` and the last `ClientEvent`, and issues one `call` on mount; **zero `cfg`**. Guard with `#![cfg(feature = "mock")]` at the example level only.
9. **`tests/boundaries.rs`** — mirror `jeliya-api`: (a) `cargo tree -p jeliya-client --no-default-features` (library only) excludes `iroh`, WebSocket crates, `tao`, `wry`, `dioxus`; (b) source scan asserts no `serde_json::Value` token in public source.
10. **`tests/seam.rs`** — the AC-mapped behavior suite (§7), driven by the mock, runnable on native; the wasm compile is proven by the CI target build (a full wasm *test* run is optional, see §8).
11. **CI** — extend `.github/workflows/ci.yml` Rust job (§8): add the wasm target and two example builds.
12. **Docs** — normative surface is crate rustdoc (mirror `jeliya-api`'s doc density). The decision itself is already recorded in `docs/dioxus-architecture.md` §Decision 4; no new `docs/` page is required for this slice. If a reference page is later wanted, it must satisfy `docs/PROFILE.md` (exactly 10 frontmatter fields, index reachability) — treat that as a separate, optional follow-up, not part of #167.

## 6. Public API surface (summary)

Exported from `jeliya_client`:

- `ClientHandle` (`Clone`), with `call<O>`, `call_stream<O>`, `subscribe`, `state`, `start`, `stop`, and convenience wrappers.
- `Dedup`.
- `State`, `ClientEvent`, `RoomPush`, `EventSubscription` (`impl Stream`).
- `CallError`, `Execution`, `LocalError`.
- `StreamCall<O>` (surface only).
- Under `feature = "mock"`: `MockScript`, `Program`, `MockController`.
- Re-export nothing from `jeliya-api` that would duplicate it; depend on it and reference its types (`ApiError`, `Push`, `Operation`, ids, shared value types) directly.

**Not exported:** `ClientBackend`, `ErasedCall`, `RawJson` (the erasure stays internal).

## 7. Test strategy — every acceptance criterion mapped

| Issue AC | How the seam satisfies it | Test |
|---|---|---|
| Calls are compile-time paired with outputs | `call<O> -> O::Output` via `jeliya_api::Operation`; no untyped return | a `tests/seam.rs` case whose type-checking *is* the proof: `let out: RoomCreateOut = handle.call(RoomCreate{..}, Dedup::None).await?;` and a doc/UI-test asserting a mismatched output type fails to compile (`trybuild` optional) |
| Start, stop, state, gap, subscription behavior are explicit | `start`/`stop`/`state`/`StateChanged`; `ClientEvent::Gap`; independent `subscribe()` | mock scripts a state sequence and a gap; test asserts observed `StateChanged` order and a `Gap` event |
| Multiple consumers cannot silently steal each other's pushes | fan-out subscriptions; replies off the event stream | two subscriptions, emit one push, **both** receive it; a reply is delivered only to its caller future, never to a subscriber |
| Stop settles all accepted work and closes event streams | §D6 ordering | accept N calls (some `Hang{sent:true}`, some `sent:false`), call `stop()`; assert every future resolves (real reply or `Cancelled` with correct `Execution`), every subscription yields `None`, final state `Stopped`, and `stop()` resolves |
| Mock scripts responses, errors, push-before-response, gaps, cancellation, shutdown | §D10 `Program` + `MockController` | one test per scenario; push-before-response asserts subscriber sees push strictly before the caller future resolves |
| WASM-local and native compilation pass without scattered component `cfg` | §D9 + zero-`cfg` component | CI builds the example for `wasm32-unknown-unknown` and native; a grep-style test asserts no `cfg(` appears in `examples/shared_component.rs` |

Additional correctness tests (Security & correctness section of the issue):

- `CallError::execution()` is total and correct for **every** variant, including `Cancelled`/`Disconnected` carrying both `DefinitelyNot` and `Unknown`, `LocalError::DecodeReply` ⇒ `Definitely`, `Wire(_)` ⇒ `DefinitelyNot`.
- `drop_connection()` settles sent-and-pending as `Disconnected{Unknown}` and unsent as `Disconnected{DefinitelyNot}` — the never-sent vs may-have-executed distinction the issue names.
- Wire errors are never conflated with local/queue/timeout classes (a `ReplyErr` yields `Wire`, never `Local`; a scripted `QueueFull` yields `QueueFull`, never `Wire`).

## 8. CI changes (`.github/workflows/ci.yml`, Rust job)

The Rust job uses toolchain `1.96.0` with `fmt`/`clippy --locked --workspace --all-targets -- -D warnings`/`test --locked --workspace`. Add:

1. `rustup target add wasm32-unknown-unknown` (toolchain step already installs stable 1.96.0; add the target via the `dtolnay/rust-toolchain` `targets:` input or an explicit `rustup target add`).
2. **Native example build:** `cargo build --locked -p jeliya-client --example shared_component --features mock`.
3. **WASM example build:** `cargo build --locked -p jeliya-client --example shared_component --features mock --target wasm32-unknown-unknown`.
4. `cargo test --locked --workspace` already covers `jeliya-client`'s native tests and `tests/boundaries.rs`.

Notes:
- Keep Dioxus in the crate as an **example/dev** dependency using the **core `dioxus`** crate (RSX + `#[component]`), **not** `dioxus-desktop` — `dioxus-desktop` links OpenSSL non-optionally and pulls `tao`/`wry`, which would both break the wasm build and violate the library's boundary. The example needs only RSX and a component signature; no renderer runs in CI.
- A full wasm *test* run (`wasm-bindgen-test` + headless browser) is **optional** and not required by AC-6, which asks for *compilation*. The behavior suite runs natively; the wasm target build proves the seam links on `wasm32`.
- Default finalize gates for this repo (per project pack): `cargo +1.96.0 test --locked --workspace`, plus `fmt`, `clippy`, and `check-docs`. The new crate must be clean under all; `#![deny(missing_docs)]` means every public item needs a doc comment (match `jeliya-api`'s density).

## 9. Dependencies and boundary rationale

| Dependency | Why | Boundary note |
|---|---|---|
| `jeliya-api` (path) | the typed operations, outputs, pushes, errors, value types | the whole point; already wasm-safe |
| `serde`, `serde_json` | erase requests/replies at the internal `RawJson` boundary only | `serde_json` confined to `handle.rs`/`backend.rs`; never in a public signature |
| `futures` | `Stream`, `oneshot`, `BoxFuture` | executor-agnostic |
| `async-broadcast` | multi-consumer event fan-out, wasm-safe, runtime-free | overflow surfaces as `Lagged`, never a silent drop |
| `thiserror` | error ergonomics (matches `jeliya-api`) | — |
| `dioxus` (core) | **example/dev only** — the verification component | never a library dependency; asserted by `boundaries.rs` |

The library MUST NOT depend on `tokio`, `iroh`, any WebSocket crate, `tao`, `wry`, or Dioxus. These are asserted by `tests/boundaries.rs` against the library target's dependency tree.

## 10. Risks and mitigations

- **Scope creep into #168.** The queue/backpressure/replay/generation-fencing machinery is #168, not #167. *Mitigation:* #167 only *defines* `QueueFull`/`Cancelled`/`Disconnected` + `Execution` and lets the mock *script* them; no real accounting here.
- **Object-safety regressions.** A future generic method on `ClientBackend` would break `dyn`. *Mitigation:* the `_assert_object_safe` guard and the erased `dispatch` signature; generics live only on the concrete `ClientHandle`.
- **Accidental `serde_json::Value` / `tokio` / Dioxus creep.** *Mitigation:* `boundaries.rs` dependency-tree + source-scan tests, run in the standard `cargo test --workspace`.
- **wasm build breakage from a transitive native dep.** *Mitigation:* the CI wasm example build gates every change; keep Dioxus to the core crate and out of the library.
- **Non-deterministic mock.** A mock that resolves on real task-scheduling order would make push-before-response flaky. *Mitigation:* controller-driven, clock-free resolution (`deliver_next`), so ordering is explicit and identical across targets.
- **Mis-modeling `resync_required`.** It is both a wire reply (`ApiError::ResyncRequired`) and a synthesized event (`ClientEvent::ResyncRequired`). *Mitigation:* §D5/§D7 state both paths explicitly and a test asserts a scripted `stream.resync` reply is `Wire`, while a `controller.emit` is an event.
- **`Wire ⇒ DefinitelyNot` over-claiming for streams.** A partially-streamed `file.share` that aborts pre-END commits no event, so `DefinitelyNot` holds; but this must be stated so a later streaming implementer does not weaken it. *Mitigation:* documented in §D7 and re-checked when #168 lands the stream kernel.

## 11. Non-goals (from the issue, restated)

- Real networking; any WebSocket, Iroh, or native transport (those are `WsWeb`/`WsNative`/#168).
- Direct `jeliya-core` execution (`DirectClient` is #173; even it goes through the erased boundary, not core, at this seam).
- Platform file/lifecycle services (`PlatformServices`, #174 — injected separately).
- A generic method that loses request/output pairing.
- The kernel's bounded-queue accounting, generation fencing, and replay ledger (#168) — only their seam-visible *types* are defined here.
- Full byte-stream framing execution (#233/#242/#243 daemon; #168 client kernel) — only the seam surface (§D11).

## 12. Open questions

1. **Fan-out library.** `async-broadcast` vs a hand-rolled fan-out over `futures`. Recommend `async-broadcast` for its explicit overflow signal (which powers `Lagged`); confirm it is acceptable as a new workspace dependency and that its wasm build is clean under the pinned toolchain.
2. **Cancellation trigger for `call<O>`.** Is dropping the returned future the sole cancel path, or does the seam also expose an explicit `CallToken`/`cancel()` (needed to cancel without dropping, e.g. from another task)? Recommend defining a lightweight `CallToken` so cancellation can carry through to `transfer.cancel` semantics later; confirm scope.
3. **Convenience wrappers.** Which operations (if any beyond the six proposed) get hand-written wrappers now, versus leaving everything to `call<O>`? A macro over `jeliya-api`'s operation list could generate all 33, but adds a code-gen surface; recommend the small hand-written set for #167.
4. **`trybuild` for the negative pairing test.** Worth adding a `trybuild` dev-dependency to prove a mismatched `O::Output` fails to compile, or is the positive typed test plus the `Operation` bound sufficient? Recommend the positive test for #167 and defer `trybuild` unless review wants the negative proof.
5. **Whether `State::Failed` needs a carried reason.** A terminal gate refusal (`protocol_unsupported`, `storage_generation_mismatch`) is actionable (show the reset path). Recommend `Failed` carry a small `FailReason` so components can render the reset path without inspecting a separate error; confirm.

## 13. Assumptions

- `crates/jeliya-api` (#163) is landed and stable at its current shape (verified: `Operation`, `Envelope`, `Push`, `ApiError`, ids, shared value types all present); this seam builds directly on it.
- #168 will implement the kernel *behind* the `ClientBackend` trait this issue defines, so the trait signature must be sufficient for bounded queues, generation fencing, and replay without a breaking change — the erased `dispatch` + `subscribe` + `state`/`start`/`stop` shape is chosen with that in mind.
- The repo's Rust toolchain is `1.96.0` with `wasm32-unknown-unknown` available on the CI runner after `rustup target add`.
- The orchestrator performs all git/gh/PR actions; this document is the only artifact the planning phase produces, and no production code is written for #167 by the planning phase.
- Adding one new workspace crate and the CI wasm/example steps is within the acceptable change surface for an M2 entry slice (the architecture explicitly anticipates this crate under "client kernel and seam").
