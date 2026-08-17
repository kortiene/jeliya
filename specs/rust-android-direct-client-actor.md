# Spec — Bounded single-owner DirectClient actor (#173)

- **Issue:** kortiene/jeliya#173 — `[Rust][Android]: Implement the bounded single-owner DirectClient actor`
- **Program:** #156 (Dioxus clean-slate). **Milestone:** M2 (client runtime and platform adapters).
- **Records/derives its decision from:** `docs/dioxus-architecture.md` §"Decision 4 — one seam, four adapters, one platform boundary" and `docs/protocol-v2.md` (generation gate, idempotency/retry, deadlines, pushes/gap/resync).
- **Depends on (all landed on this base):** typed Engine #166 (`crates/jeliya-core` `Engine::execute_with` + `TypedCall`/`TypedReply`), the bounded client kernel #168 (`crates/jeliya-client/src/kernel/`), the authoritative reconciler #169 (`crates/jeliya-client/src/reconcile/`, already carries `Reconciler::resume()`), and physical feasibility #160 (aarch64 toolchain).
- **Sibling adapters (parallel, not blockers):** #171 `WsWeb`, #172 `WsNative`. All three land the same generic driver runtime; see §5.1 for merge coordination.
- **Downstream:** #196 (WebView trust-boundary security qualification) consumes the completed DirectClient. #202 deletes `crates/jeliya-ffi` (Dart/C-ABI/FFI) once this candidate passes.
- **Owner role:** core maintainer (client kernel + platform adapter).
- **Status of this document:** design, not yet implemented. Where this spec and `docs/protocol-v2.md` / `docs/dioxus-architecture.md` disagree, those records are authoritative and this spec has a bug — say so in the PR.

> The orchestrator owns all git/gh. This document does not instruct any git or GitHub action.

---

## 1. Outcome

Give Android a `ClientHandle` whose backend calls **typed `jeliya-core` in-process** through one bounded, serialized actor. The direct data path contains no daemon-envelope JSON (`Engine::handle_frame`), no Dart, no C ABI, no socket, and no token/portfile. It reuses the shared kernel (#168), reconciler (#169), state model, and error taxonomy so that a Dioxus component renders and reasons about the direct backend exactly as it does the WebSocket backends.

This is the production replacement for `crates/jeliya-ffi/src/host.rs` (`FfiHost`), which today serializes dispatch through Dart `SendPort`s over a C ABI to `Engine::handle_frame(String)`. The workspace manifest already records the plan: `jeliya-ffi` is quarantined out of the build and "deleted outright under #202 once the Android DirectClient candidate passes."

---

## 2. Ground-truth evidence (read before implementing)

| Fact | Location |
| --- | --- |
| The FFI host **serializes dispatch** to avoid concurrent SQLite/WAL "database is locked" on a fresh `rooms.db`; the push loop must run even with zero subscribers to maintain the join-bootstrap `accept_joins` window; teardown = `push_loop.stop()` → drain dispatch → `close_all_rooms()` (10 s bound) → drop engine. | `crates/jeliya-ffi/src/host.rs` |
| Typed, JSON-free engine surface: `Engine::execute_with(TypedCall, Option<OpId>, principal_key) -> Executed { reply: Result<TypedReply, ApiError>, stop_after_reply }`; `Engine::new(PathBuf, loopback, EngineConfig) -> CoreResult<Arc<Engine>>`; `subscribe_pushes() -> broadcast::Receiver<Push>`; `start_push_loop() -> PushLoopHandle`; `close_all_rooms() -> bool`; `data_dir()`, `limits()`. | `crates/jeliya-core/src/engine.rs` |
| `Engine::handle_frame(String)` is the **retired v1 string seam**; using it is the forbidden JSON path. | workspace `Cargo.toml` comment; `jeliya-ffi` |
| Daemon request pipeline to reuse minus the socket: `route(op, in_value) -> Call` (`jeliya-codec/src/routing.rs`), `resolve_call(op, call.input_any()) -> Option<TypedCall>` (`jeliya-core/src/typed.rs:3185`), `execute_with(...)`, then `Ok(out) => serde_json::to_value(out)` / `Err(e) => e` into a reply. | `crates/jeliyad/src/serve.rs:1856-1892` |
| Shared client contract: `ClientHandle::call<O>` **unconditionally** `serde_json::to_string(&input)` on the way in and `serde_json::from_str::<O::Output>` on the way out; the backend seam `ClientBackend`/`ErasedCall` carries `RawJson` for every adapter. | `crates/jeliya-client/src/handle.rs:196-256`, `src/backend.rs` |
| Kernel is sans-IO and synchronous: `KernelBackend: ClientBackend` drives a `Core` state machine that emits `Action::{Dial{token}, Send(WireFrame), ArmTimer, CancelTimer, CancelDial, Settle, Emit, CloseBus}` and consumes `Input::{Start, Stop, Connected{token}, Interrupted{generation}, DialFailed{token}, GateRefused{token}, Inbound, TimerFired, Cancel, Dispatch}`. In **this tree** `Action::Dial` is a no-op (`self.dialing = true`): no real driver/runtime loop exists yet — only the in-memory `KernelController` behind `test-transport`. | `crates/jeliya-client/src/kernel/{mod,core,transport}.rs` |
| `KernelConfig.stable_principal` doc: **`DirectClient` (in-process: daemon restart = client restart) can certify**; a socket adapter cannot until `hello` carries a daemon incarnation. Defaults to `false`. | `crates/jeliya-client/src/kernel/mod.rs:119-131` |
| The reconciler already owns Android resume: `Reconciler::resume() -> Result<(), ReconcileError>` issues `Command::Resume → Input::Resume → ResyncReason::Resume`, re-baselining every active room through the same authoritative read path **without a reconnect**. `ClientEvent::Lagged` (local fan-out overflow) drives `ResyncReason::LocalOverflow`. | `crates/jeliya-client/src/reconcile/{driver.rs:296, reason.rs, mod.rs}` |
| Shared, transport-independent state vocabulary already exists and is reused verbatim: `State`, `ClientEvent`, `EventBus` fan-out (bounded, coalescing), `CallError`/`Execution`. | `crates/jeliya-client/src/event.rs`, `src/error.rs` |
| Boundary CI: `tests/boundaries.rs` scans the library's `--no-default-features` tree and bans `tokio`, `tokio-tungstenite`, iroh, etc.; separately scans `src/kernel/**` and `src/reconcile/**` for `tokio`/`std::time`/RNG. `#![forbid(unsafe_code)]` is workspace-wide; MSRV 1.91; edition 2021. | `crates/jeliya-client/tests/boundaries.rs`, workspace `Cargo.toml` |

---

## 3. Goals / Non-goals (from the issue, made concrete)

**Goals.** Own Engine, push loop, request mailbox, lifecycle, and teardown behind one serialized actor; expose the shared `ClientHandle`. Bounded mpsc + per-call response channels. Hierarchical cancellation. One live owner per canonical data directory. Explicit resume resync. Complete protocol-v2 adapter conformance at the view level.

**Non-goals.**
- Unrestricted concurrent core method calls (serialization is load-bearing, §K/WAL).
- Pretending DirectClient reconnects — it never emits `Interrupted`, never dials twice, never arms a backoff timer.
- Migrating any Flutter-created identity, room, preference, or signed log — DirectClient opens **only new-generation state**.
- Background-execution claims while the app is suspended.

---

## 4. The "no JSON" resolution (mandatory reading — this is the crux)

The issue's AC "Direct path contains no JSON/Dart/C ABI/socket/token/portfile" must be read against the code that exists. `ClientHandle::call<O>` — the **shared contract every adapter exposes** — always `serde_json::to_string(&input)` on the request and `serde_json::from_str::<O::Output>` on the reply (`handle.rs:201,184`). That erasure is a fixed property of the shared seam, identical for WsWeb/WsNative/DirectClient/mock; it is not a transport artifact and cannot be removed for one adapter without changing `ClientHandle`, `ClientBackend`, and `ErasedCall` for all four — which contradicts "expose the shared client contract."

Therefore "no JSON" is scoped to the **wire and FFI boundary**, and this spec enforces it precisely:

- **Forbidden and absent:** `Engine::handle_frame(String)` (the retired v1 envelope seam); any `{"id","op","op_id","in"}`/`{"id","ok","out"}` envelope framing; `jeliya-codec` **byte** framing (`decode(bytes,..)`, `Reply::to_bytes`, `push_to_bytes`); any WebSocket/TCP; Dart `SendPort`/`nativePort`; C ABI exports; session `token`; daemon `portfile`; storage-generation query gate.
- **Present and typed:** the engine is called only through `Engine::execute_with(TypedCall, …)` and observed only through `subscribe_pushes()` → `Push`. No bytes cross any transport.
- **The one residual, named honestly:** to turn the shared seam's erased `ErasedCall { op, input: RawJson }` into a `TypedCall`, the adapter reuses the daemon's own in-process router — `route(op, serde_json::from_str(in)) → Call → resolve_call → TypedCall` — and encodes `TypedReply` back with `serde_json::to_value`. This is an **in-process struct↔struct transform**, not wire JSON: it produces no bytes, touches no socket, and is exactly the transform the shared `ClientHandle` already forces at its edge. It is the same routing the daemon runs; reusing it is what guarantees "adapter contract tests match WS view-level outcomes" and "complete protocol-v2 adapter conformance."

If the team later wants *literally* zero `serde` in the direct path, that is a separate, larger change to the shared contract (a typed `ClientBackend` fast-path carrying `TypedCall`), tracked as OQ-1 below and explicitly **out of scope** here.

---

## 5. Architecture

Four cooperating layers. Only layers 5.2–5.4 are new code; 5.1 is shared with #171/#172; the reconciler and kernel core are consumed unchanged.

```
Dioxus UI ── ClientHandle (shared; typed↔JSON erasure at its edge)
   │
   ├── Reconciler (#169, unchanged): activate_room / resume() / subscribe → authoritative resync
   │
   ▼
ClientBackend  ═══ KernelBackend (#168 kernel core, reused) ═══════════  [5.1]
   emits Action::{Dial,Send,ArmTimer,…}     consumes Input::{Connected,Inbound,TimerFired,…}
   │
   ▼
Generic driver runtime (land-or-reuse; shared with #171/#172)            [5.1]
   binds one concrete Driver to the kernel core, owns the async loop
   │
   ▼
DirectDriver  ─ never-reconnecting, always-ready in-process "transport"  [5.3]
   Dial(token) → open engine (once) → Connected{token} gen 1
   Send(WireFrame) → route→resolve_call→execute_with → Inbound::Reply
   push subscription → Inbound::Push ;  CancelDial/Stop → teardown
   │
   ▼
DirectEngineActor  ─ FfiHost port: Arc<Engine> + serialized dispatch task [5.2]
   + push-forward task + PushLoopHandle + bounded request mpsc + teardown
   │
   ▼
OwnershipRegistry ─ one live owner per canonical data dir, serialized     [5.4]
   first-open (fail-closed on a second owner)
```

**Why reuse the kernel (Decision D1).** The kernel already provides — and `tests/kernel_fault.rs` already proves — the bounded queue (byte + count), in-flight throttle, per-call deadline, cancel-on-drop, exactly-once settlement, and the total-stop drain ordering the issue asks for. Reusing it makes AC "adapter contract tests match WS view-level outcomes" *structural*: identical `Core`, `State`, `CallError`, and `EventBus`. The DirectDriver is a degenerate `Driver` (dial once, never `Interrupted`, no backoff), which is clean.

**Fallback (D1-alt).** If the shared runtime proves too entangled with reconnect semantics for a never-reconnecting adapter, a bespoke `ClientBackend` that owns the Engine actor directly and reuses the shared `EventBus`/`State`/`CallError`/`ClientEvent` (skipping the kernel core) also satisfies every AC — at the cost of re-deriving the bounded-mailbox/cancellation/timeout/stop-drain invariants the kernel already tests. **Choose the kernel path unless the shared runtime lands with a reconnect-only shape.** Record whichever is chosen in the PR.

### 5.1 The generic driver runtime (land-or-reuse)

`transport.rs` (§K13) states the runtime that "binds a real `Driver`'s transport, clock, and dialer to the core … lands with the first adapter slice." In this tree it does not exist. Whichever of #171/#172/#173 merges first lands it; the others rebase onto it. The runtime must:

- Own a single async loop that owns the concrete `Driver`, feeding `Core` inputs and performing `Core` actions, reusing the kernel's existing re-entrancy-safe delivery queue (`Deferred`/`drain_delivery`) so wakers run outside the `Shared` lock.
- Read time from an **injected** clock (no `std::time` inside `src/kernel`), map `Action::ArmTimer/CancelTimer` to real timers, `Action::Dial{token}`/`CancelDial` to the driver's dialer, `Action::Send` to the driver's transport, and translate driver events back into `Input::{Connected{token}, Inbound, Interrupted{generation}, DialFailed{token}, GateRefused{token}}`.
- Expose a public constructor returning a `ClientHandle` bound to that runtime (analogous to today's test-only `ClientHandle::with_kernel`).

This spec **does not** re-specify that runtime's exact shape (that belongs to the first-lander and to merge coordination); it specifies only the DirectDriver's obligations against it (§5.3). If #173 is the first-lander, build the minimal runtime described here; keep it in `src/kernel` **only** for the sans-IO parts and put every `tokio`/timer/clock detail in the adapter module (§6) so the `kernel_source_has_no_wall_clock_rng_or_runtime` scan stays green.

### 5.2 `DirectEngineActor` — the FfiHost port (owns the Engine)

A single struct owning everything the live engine needs, torn down as one unit. Directly ports `FfiHost` minus the Dart/atomic-port machinery:

- `engine: Arc<Engine>` — constructed by `Engine::new(canonical_dir, loopback = true, EngineConfig { port: 0, version: CORE_VERSION, shutdown_tx })`. `loopback` is `true` (in-process, no external listener); `port: 0` = "no listener" (a bound daemon never reports 0).
- `push_loop: PushLoopHandle` — started **immediately** on open, before any subscriber exists (join-bootstrap maintenance, AC "always-run push-loop join maintenance").
- `requests_tx: mpsc::Sender<EngineRequest>` — **bounded** (§9); the dispatch task is the only receiver.
- Task handles: `dispatch`, `push_forward`, `shutdown_watch` — all `JoinHandle<()>`.
- `owner_token: OwnerToken` — the ownership grant from §5.4, released on teardown.

**Dispatch task (the serialized actor, load-bearing).** One task, `while let Some(req) = requests_rx.recv().await`, running each `Engine::execute_with(...)` strictly one at a time and returning the result on the request's own `oneshot`. This serialization is the WAL-race guard the FFI host documents; it also satisfies AC "Calls execute serially." An `EngineRequest` carries the pre-resolved `TypedCall`, the `op_id`, and a `oneshot::Sender<Result<RawJson, CallError>>` (or a typed equivalent the driver bridges — see §5.3). `daemon.stop` is sequenced by the engine (`stop_after_reply`) via the `shutdown_watch` task, which then runs the same teardown as an explicit stop.

**Push-forward task.** Subscribes once (`engine.subscribe_pushes()`), forwards every `Push` into the kernel driver as `Inbound::Push` for the engine's life. `RecvError::Lagged(n)` is **not** dropped silently: it maps to the seam's local-overflow signal so the reconciler resyncs (see §5.3, "lag"). `RecvError::Closed` ends the task.

**Teardown (deterministic, bounded).** In order: `push_loop.stop()` (no new room pumps); drop `requests_tx` and `dispatch.await` (drains every accepted request — an accepted call must not lose its reply); `engine.close_all_rooms().await` (internally bounded to 10 s); abort `push_forward`; drop `engine`; release `owner_token`. Return a `TeardownOutcome { clean: bool }` where `clean = false` means rooms stayed open past the close budget (their `rooms.db`/blob locks may remain held until process exit — a re-open over the same dir would then fail). This mirrors `FfiHost::teardown`'s `0`/`1` done code.

### 5.3 `DirectDriver` — the never-reconnecting transport binding

Implements the driver contract (§5.1) against the `DirectEngineActor`:

- **`dial(token)`** → acquire ownership (§5.4) and open the `DirectEngineActor` **once**. On success feed `Input::Connected { token }` (generation becomes 1). Opening is `Engine::new` + `start_push_loop` + task spawns; it is the "serialized first-open" — the ownership registry guarantees only one `Engine::new` per canonical dir runs at a time, and the actor's single dispatch task serializes the first room-store opens that race the WAL transition.
  - **First-open failure** (`Engine::new` error, or ownership already held by a live owner) → `Input::GateRefused { token }` (terminal `Failed`, **no retry** — there is nothing to back off to). This maps "protected data-dir ownership" and "serialized first-open" failures to an honest terminal state.
- **Never dials again.** The driver holds the live actor. There is no transport to lose, so it **never** produces `Input::Interrupted` and **never** produces `Input::DialFailed` after the first open. State therefore only ever moves `Idle → Connecting → Ready → Stopping → Stopped` (or `→ Failed` from a failed first-open). This is precisely "Pretending DirectClient reconnects" being a non-goal.
- **`Send(WireFrame { id, op, op_id, input: RawJson })`** → build an `EngineRequest`:
  1. `value = serde_json::from_str(input.as_str())` → on parse failure, settle `Inbound::Reply { WireReply::Err(ApiError::MalformedFrame) }`.
  2. `call = jeliya_codec::routing::route(op, value)` → `Err(ApiError)` (unknown op / invalid argument) settles `WireReply::Err(that error)` without touching the engine.
  3. `typed = jeliya_core::typed::resolve_call(op, call.input_any())` → `None` ⇒ `WireReply::Err(ApiError::MalformedFrame)`.
  4. enqueue on the bounded `requests_tx`. **If the channel is full**, settle a delivery-classified error so backpressure is honest (see §9); do not block the driver loop.
  5. The dispatch task runs `execute_with(typed, op_id, principal_key)`; on completion the driver feeds `Input::Inbound(Inbound::Reply { generation: 1, id, result })` where `result = Ok(RawJson(serde_json::to_value(reply).to_string())) | Err(api_error)`.
  - **`op_id` / principal.** Pass a **stable per-session `principal_key`** (e.g. `"direct"`), not the ephemeral form, so envelope `op_id` dedup/idempotency behaves within the session exactly as on the wire. The kernel forwards the caller's `op_id` verbatim.
- **Push** → `Inbound::Push { generation: 1, push }`.
- **lag** → when the push-forward task observes `broadcast::error::RecvError::Lagged(n)`, the driver must surface local overflow so the reconciler re-baselines. Two acceptable routes; pick one and test it: (a) if the runtime/seam exposes an overflow input, feed it; otherwise (b) emit a synthesized `ClientEvent::Lagged { room_id: None, dropped: n }` onto the bus through the kernel's existing `Emit` path. Either way the reconciler consumes `ClientEvent::Lagged` and issues `ResyncReason::LocalOverflow`.
- **`cancel_dial()` / total stop** → run `DirectEngineActor` teardown (§5.2). The kernel's `Input::Stop` drains accepted calls first; the driver awaits teardown before the stop future resolves (AC "Stop drains … and awaits teardown").
- **clock** → injected monotonic clock (1 tick = 1 ms), same as WsNative; used only by the runtime's timer/deadline machinery. DirectClient never arms a backoff timer, but per-call deadline timers still apply.

**`stable_principal = true`.** DirectClient is the one adapter that can certify it (`KernelConfig` doc). It has no live effect here because the driver never emits `Interrupted` (no replay window ever opens), but `true` is the honest value and is safe: any replay would target the same in-process engine + intact ledger. Document this; do not set `false`.

### 5.4 `OwnershipRegistry` — one live owner per canonical data dir

A process-global registry enforcing AC "One owner controls a canonical data directory" and "protected data-dir ownership":

- Key: the **canonicalized** data dir, using the same normalization `FfiHost::same_data_dir` uses (`identity::ensure_dir` then `Path::canonicalize`, falling back to the spelled path). "Same dir spelled differently" is the same owner.
- `acquire(dir) -> Result<OwnerToken, OwnershipError::AlreadyOwned>` — grants exclusive ownership if free; **fail-closed** if a live owner exists. A `OwnerToken`'s `Drop` releases the entry, so teardown (or a dropped actor) frees the dir for a subsequent `connect_direct` — supporting AC "repeated start/stop."
- The registry is `Mutex<HashMap<CanonicalDir, Weak<…>>>` (or a live-set); pruning dead weak entries on `acquire` keeps it bounded under start/stop churn.
- **Scope note (OQ-2):** this is a *process*-global guard, matching `FfiHost`'s process-singleton `HOST`. It does not protect against a *second OS process* opening the same on-disk dir; on Android the app is a single process, and the engine's own per-room `rooms.db`/blob locks remain the cross-process backstop. State the scope in the PR.

---

## 6. Module layout & feature gating

- New module `crates/jeliya-client/src/direct/` with submodules: `mod.rs` (public `connect_direct` + `DirectConfig`), `actor.rs` (`DirectEngineActor`, dispatch/push/teardown), `driver.rs` (`DirectDriver` + codec bridge), `ownership.rs` (`OwnershipRegistry`), `bridge.rs` (RawJson↔TypedCall/TypedReply helpers), `diag.rs` (redaction-safe tracing). **Do not** place any of this under `src/kernel/` or `src/reconcile/` — those directories are scanned for `tokio`/`std::time`/RNG (`boundaries.rs`).
- Gate the module once at the `lib.rs` declaration: `#[cfg(all(not(target_arch = "wasm32"), feature = "direct"))] mod direct;` and `pub use direct::{connect_direct, DirectConfig};` behind the same cfg.
- New default-off, **native-only** feature `direct` with **optional** dependencies (an optional dep compiles only when its feature is on, so `cargo check --all-targets` and the wasm build never pull them; jeliya-core transitively pulls `iroh-rooms`, a banned substring, so it *must* be optional):
  ```toml
  [features]
  direct = ["dep:tokio", "dep:jeliya-core", "dep:jeliya-codec", "dep:serde_json"]  # serde_json already a base dep; re-list only if needed

  [target.'cfg(not(target_arch = "wasm32"))'.dependencies]
  tokio = { version = "1", default-features = false, features = ["rt", "sync", "time", "macros"], optional = true }
  jeliya-core  = { path = "../jeliya-core",  optional = true }
  jeliya-codec = { path = "../jeliya-codec", optional = true }
  ```
  Mirror `jeliya-platform`'s per-feature CI precedent. `serde_json` is already a base dependency (confined to `RawJson`); the bridge's use of `serde_json::Value` stays in `pub(crate)`/private code so `no_serde_json_value_in_public_source` holds.
- `connect_direct(config: DirectConfig) -> ClientHandle` returns the shared handle (via `ClientHandle::from_backend`, the existing internal constructor). `DirectConfig` carries the data dir `PathBuf`, `KernelLimits`, jitter seed (unused but honest), and the request-channel bound.

---

## 7. Lifecycle mapping (explicit)

| Event | Kernel input | Observable `State` |
| --- | --- | --- |
| `handle.start()` | `Input::Start` → `Action::Dial{token}` | `Idle → Connecting` |
| first-open succeeds | `Input::Connected{token}` | `Connecting → Ready` |
| first-open fails / dir already owned | `Input::GateRefused{token}` | `Connecting → Failed` (no retry) |
| engine live, app suspended & resumed | *(none from driver)* — engine never disconnected | stays `Ready`; app calls `Reconciler::resume()` |
| push-forward sees `Lagged(n)` | overflow → `ClientEvent::Lagged` | stays `Ready`; reconciler resyncs |
| `handle.stop()` | `Input::Stop` (drain) then driver teardown | `Ready → Stopping → Stopped` |

DirectClient never enters `Interrupted`. There is no `Reconnect` resync reason on this adapter; resume is the only re-baseline trigger not caused by a gap/overflow.

---

## 8. Resume & resync wiring (no fabricated reconnect)

- The reconciler already re-baselines on resume: the app (Android `onResume`/foreground, in the platform/shell layer — out of scope here) calls `Reconciler::resume()`, which fans an authoritative `stream.resync`/`room.timeline` read through `ClientHandle::call` for every active room (`ResyncReason::Resume`). No `Disconnected`/`Connecting`/`Connected` cycle is synthesized.
- DirectClient's obligation is purely negative-plus-liveness: **do not** emit `Interrupted`/reconnect, and **keep the engine + push loop alive** across suspension so a resume re-read hits a live engine. If the OS killed the process during suspension, the next launch is a fresh `connect_direct` → first-open → `ResyncReason::Bootstrap` (a fresh start, not a reconnect).
- Missed pushes while frozen surface as `RecvError::Lagged` → `ClientEvent::Lagged` → `ResyncReason::LocalOverflow`, independent of the explicit resume call. Both converge on the single authoritative resync path (#169), satisfying "Resume triggers authoritative resync without fabricated reconnect."

---

## 9. Bounds (every lane bounded — AC "Mailbox and push lanes are bounded")

| Lane | Bound | Overflow behavior |
| --- | --- | --- |
| Kernel admitted-but-unsent queue | `KernelLimits.queue_depth` (count) + `outbound_bytes` | `CallError::QueueFull` (`Execution::DefinitelyNot`) |
| Kernel in-flight throttle | `KernelLimits.in_flight` | held queued, released as replies land (never refused) |
| Per-call deadline | `KernelLimits.default_call_deadline` | `CallError::Timeout` (`Execution::Unknown`) |
| Actor request mpsc (`requests_tx`) | `DirectConfig.request_channel_depth` (e.g. 256) | driver settles the call with a delivery-classified error (do **not** block the loop); tune so the kernel queue is the primary backpressure surface |
| Push broadcast (engine side) | engine's fixed capacity 1024 | `Lagged` → `ClientEvent::Lagged` → resync |
| Event fan-out per subscription | `event.rs` defaults (1024 events / 16 MiB, coalescing) | `ClientEvent::Lagged`, bounded/coalesced |
| Reconciler per-room buffers | `ReconcileLimits` defaults | forced fresh baseline, never silent drop |
| Ownership registry | pruned weak entries | fail-closed second owner |

No unbounded channel anywhere. The FFI host's `mpsc::unbounded_channel` for requests is deliberately **replaced** by a bounded channel here (the kernel provides the true admission bound above it; the actor channel is a small hand-off buffer).

---

## 10. Security & correctness invariants

1. **Serialized first-open for new-generation state.** Only one `Engine::new` per canonical dir at a time (registry), and the actor's single dispatch task serializes the first room-store opens (WAL guard). Concurrent `connect_direct` on the same dir → exactly one owner; the loser gets `AlreadyOwned` → `GateRefused` → `Failed`.
2. **Always-run push-loop join maintenance.** `start_push_loop()` runs at open, before any subscriber, for the engine's whole life; only teardown stops it.
3. **Protected data-dir ownership.** Fail-closed `OwnershipRegistry`; `OwnerToken` release is tied to teardown/drop.
4. **Deterministic teardown.** Fixed order, 10 s room-close bound, honest `clean` flag; an accepted call never loses its reply (dispatch drained before room close).
5. **No secret surfaces.** No token, no C ABI, no Dart port. The `principal_key` is a fixed non-secret string. Redaction-safe tracing only (reuse `Redacted`/`diag` patterns); never render `op_id` or payloads (§K15).
6. **New-generation only.** `Engine::new` opens the v2 storage generation; no import of Flutter/Dart/C-ABI data (that removal is #202).
7. **`may-have-executed` honesty is preserved** because the kernel's `CallError` classification is reused unchanged: never-enqueued → `DefinitelyNot`; dispatched-to-engine then cancelled/stopped → `Unknown`; decode of a returned reply fails → `Definitely`.

---

## 11. Boundary & CI additions

- **New boundary test** (twin of `jeliya-platform`/`supervisor` guards): assert `tokio`, `jeliya-core`, `jeliya-codec`, and `iroh-rooms` are **absent** from the `jeliya-ui` `web`/wasm dependency graph — i.e. the `direct` feature never leaks into the browser build. Also assert the default `jeliya-client` `--no-default-features` tree remains free of them (existing `library_dependency_tree_is_free_of_transport_and_ui_crates` already covers this once the deps are optional).
- **New CI step** (mirroring the `jeliya-platform`/`ws-native` per-feature precedent — the #172 review's MUST-FIX lesson was that a default-off feature with no CI step *never runs*): add to `.github/workflows/ci.yml`
  ```
  cargo clippy --locked -p jeliya-client --features direct --all-targets -- -D warnings
  cargo test  --locked -p jeliya-client --features direct
  ```
  Native target only (the feature is `cfg(not(target_arch = "wasm32"))`). Keep the existing wasm build of `jeliya-client` green (the module is cfg-gated out on wasm).
- `rustdoc -D warnings` clean; `#![deny(missing_docs)]` already applies.

---

## 12. Test strategy (maps every verification bullet + AC)

Unit/integration tests live in the `direct` feature. Prefer a deterministic single-thread tokio runtime (`rt`, `macros`) with paused time where feasible; use a **temp data dir** per test.

| Verification (issue §Verification) | Test |
| --- | --- |
| Concurrent first-open | Two `connect_direct` on the same temp dir concurrently: exactly one reaches `Ready`, the other `Failed` via `AlreadyOwned`; after the winner stops, a third `connect_direct` on the same dir succeeds. |
| Saturation | Flood dispatches past `queue_depth`/`outbound_bytes` → `CallError::QueueFull{resource,limit}`; past `request_channel_depth` with in-flight full → the delivery-classified refusal, never a hang. |
| Cancel before dequeue | Drop the call future while it is still queued (in-flight cap reached) → `Cancelled{DefinitelyNot}`; no engine call ran. |
| Cancel after dequeue | Drop the future after it reached the engine dispatch task → `Cancelled{Unknown}`; the engine effect may have run. |
| Stop during call | Issue a slow call, `stop()`; the call settles (reply or `Cancelled`), the stop future resolves only after teardown; `close_all_rooms` was awaited. |
| Lag | Force `broadcast` lag (fill 1024 while the forward task is parked) → a `ClientEvent::Lagged` is observed and the reconciler issues `ResyncReason::LocalOverflow`. |
| Repeated start/stop | N cycles of `connect_direct`/`stop` over the same dir: no leaked owner, no leaked task, ownership re-acquirable each time. |
| Unclean teardown | A room whose close hangs past 10 s → teardown returns `clean = false` deterministically; the stop future still resolves. |
| Complete protocol-v2 adapter conformance | Drive a representative slice of the v2 corpus operations end-to-end through `connect_direct` and assert view-level outcomes equal the WsNative adapter's for the same inputs (reply shapes, `CallError` classes, emitted `ClientEvent`s). This reuses the shared codec router + `execute_with`, so equality is expected by construction; the test guards regressions. Full-corpus A/B belongs to the #175 four-adapter parity suite. |

AC coverage: (1) grep/boundary + code review confirm no `handle_frame`/socket/token/portfile/Dart/C-ABI in `src/direct/**`; (2) bounds table tests; (3) a serialization test asserts overlapping dispatches reach the engine strictly ordered (e.g. an instrumented engine or ordered side effects); (4) ownership tests; (5) stop-drain tests; (6) resume test drives `Reconciler::resume()` and asserts a `stream.resync`/timeline read with **no** intervening `StateChanged` reconnect cycle; (7) the WS-parity test.

Real on-device Android instrumentation and the aarch64 build are gated by #160 and validated in the tests phase / #196; the deterministic host tests above are the code-phase gate.

---

## 13. Acceptance criteria checklist

- [ ] Direct path contains no JSON envelope / Dart / C ABI / socket / token / portfile (§4, §10.5); the only `serde` is the in-process struct-router the shared `ClientHandle` already forces, producing no bytes.
- [ ] Mailbox and push lanes are bounded (§9).
- [ ] Calls execute serially (single dispatch task, §5.2).
- [ ] One owner controls a canonical data directory (§5.4).
- [ ] Stop drains or explicitly cancels every accepted call and awaits teardown (§5.2, §5.3, kernel `Input::Stop`).
- [ ] Resume triggers authoritative resync without fabricated reconnect (§7, §8).
- [ ] Adapter contract tests match WS view-level outcomes (§12 parity test; structural via reused kernel/reconciler/router).

---

## 14. Risks & mitigations

1. **Shared-runtime merge coordination (§5.1).** #171/#172/#173 each "land the runtime." *Mitigation:* build against the runtime contract, not a copy; if not the first-lander, rebase onto the landed runtime; keep DirectClient-specific code in `src/direct/`. Call out the merge order in the PR.
2. **"No JSON" literalism.** A reviewer may read AC-1 as forbidding the in-process router's `serde`. *Mitigation:* §4 states the scope and the fixed shared-contract erasure explicitly; OQ-1 records the typed-seam follow-up.
3. **WAL race not actually guarded by `Engine::new` alone.** The race is per-room-store first-open, not `Engine::new`. *Mitigation:* rely on the serialized dispatch task (as the FFI host does), not on the open call; test concurrent first `room.activate`.
4. **Ownership scope (§5.4).** Process-global only. *Mitigation:* documented (OQ-2); engine per-room locks are the cross-process backstop; Android is single-process.
5. **Kernel machinery unused by DirectClient** (backoff, generation churn, replay-hold). *Mitigation:* the degenerate driver simply never triggers them; assert in tests that no backoff timer is ever armed and `generation` stays 1.
6. **`daemon.stop` in-process.** The engine sequences a 150 ms-delayed shutdown signal; the `shutdown_watch` task must run the same teardown as an explicit stop and not double-free the owner token. *Mitigation:* single teardown entry point, idempotent token release; port `FfiHost::watch_shutdown` carefully.

---

## 15. Open questions

- **OQ-1 (typed seam).** Should the shared contract gain a typed `ClientBackend` fast-path carrying `TypedCall` so the direct path has literally zero `serde`? Larger change to `ClientHandle`/`ClientBackend`/`ErasedCall` affecting all four adapters; **out of scope** for #173, worth a follow-up if AC-1 is read strictly.
- **OQ-2 (ownership scope).** Process-global vs on-disk lock file for the data dir. Recommend process-global for #173 (matches FFI host + single-process Android); revisit if desktop ever hosts DirectClient.
- **OQ-3 (lag surfacing).** Whether the shared runtime/seam should expose a first-class transport-overflow input for `Lagged`, or whether synthesizing `ClientEvent::Lagged` via `Emit` is sufficient. Decide with #171/#172 to keep the three adapters uniform.
- **OQ-4 (byte streams).** `file.share`/`file.read` byte streams (#269 stream lifecycle) depth in the direct path — likely deferred to the tests phase / a follow-up, as with #172's Binary-frame handling. Confirm whether the code-phase slice includes streamed ops or request/response only.
- **OQ-5 (`principal_key` value).** A fixed `"direct"` vs the local subject id once known. Fixed string is simplest and dedup-correct within a session; revisit if multi-principal in-process ever appears (it should not).

---

## 16. Clean-slate cutover (#202)

DirectClient opens only new-generation state. Once this candidate passes (and #196 qualifies the WebView trust boundary), #202 deletes `crates/jeliya-ffi` (the Dart bridge, C ABI, and `Engine::handle_frame` string seam) without importing any of its data. Nothing in this slice depends on or revives the FFI crate; the workspace already excludes it.
