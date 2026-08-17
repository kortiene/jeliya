# Native WebSocket adapter with fresh daemon discovery (#172)

Status: IMPLEMENTED — the codec client direction, the `DriverIo` refactor (with
the in-memory suite green byte-for-byte), the native `ws-native` adapter, and the
wasm-graph boundary guard have landed. The ignored real-daemon integration matrix
(§10.3) is deferred to the dedicated tests phase.
Owner issue: #172 `[Rust][Desktop]: Implement the native WebSocket adapter with fresh daemon discovery`.
Program: #156 (Dioxus clean-slate). Blocked by #168 (bounded kernel), #169 (authoritative resync), #170 (jeliyad supervisor).

## 1. Outcome

Bind the transport-independent client kernel (#168) to a **native async WebSocket
transport** that dials a loopback `jeliyad`, using the reusable
`jeliya_supervisor::TargetResolver` (#170) as the injected source of *verified*
loopback endpoint + token facts. The result is `WsNative`: the sole native
protocol-v2 WebSocket client the Dioxus desktop shell uses. Dart behaviour is not
retained.

The kernel deliberately ships **only** the deterministic in-memory driver today;
its transport seam (`crates/jeliya-client/src/kernel/transport.rs`) states the
real "async runtime loop that binds a real `Driver`'s transport, clock, and dialer
to the core … lands with the first adapter slice." #172 is that slice for native.
It therefore delivers two things:

1. **A reusable real async driver runtime** (`DriverIo` seam) that runs the
   sans-IO `Core` against live I/O, timers, and a dialer — infrastructure #171
   (browser) and #173 (`DirectClient`) reuse by supplying their own `DriverIo`.
2. **The native WebSocket `DriverIo`**: dial + generation/hello agreement, framed
   send/receive via the protocol-v2 codec (#164), reconnect, and total stop.

## 2. Context and evidence (ground truth read for this spec)

- **Kernel seam** — `crates/jeliya-client/src/kernel/transport.rs` defines the
  `pub(crate)` `Transport` and `Driver` traits, `WireFrame` (outbound; `Debug`
  already redacts `op_id`+payload), `Inbound` (`Reply{generation,id,result}` /
  `Push{generation,jeliya_api::Push}` / `Malformed`), `WireReply` (`Ok(RawJson)` /
  `Err(ApiError)`), and `TransportClosed`. Adapters live *inside* jeliya-client
  because these types are `pub(crate)`.
- **Kernel core** — `crates/jeliya-client/src/kernel/core.rs` `Input`/`Action`:
  the driver feeds `Connected{token}`, `Interrupted{generation}`,
  `DialFailed{token}`, `GateRefused{token}`, `Inbound(..)`, `TimerFired(id)`; the
  core emits `Send(WireFrame)`, `ArmTimer{id,at:Tick}`, `CancelTimer(id)`,
  `Dial{token}`, `CancelDial`, `Settle`, `DropSender`, `Emit`, `CloseBus`.
  Every dial outcome is token-fenced; every inbound is generation-fenced.
- **Async shell today** — `crates/jeliya-client/src/kernel/mod.rs` holds
  `KernelBackend` (the `ClientBackend` impl: `dispatch`/`subscribe`/`state`/
  `start`/`stop`), `Runtime` (locked `Shared` + serialized `delivery` queue +
  `draining` flag), `Deferred`/`DeferredWake` (post-lock wake hygiene against ABBA
  re-entrancy), and — behind `test-transport` — `KernelController` + the in-memory
  driver. `Shared::apply_one` performs the transport-touching actions (`Send`,
  `ArmTimer`, `CancelTimer`, `Dial`, `CancelDial`) against **in-memory** state.
  These five arms are exactly what a real driver must perform differently; the
  rest (`Settle`/`DropSender`/`Emit`/`CloseBus`) are already runtime-neutral.
- **Supervisor resolver** — `crates/jeliya-supervisor/src/target.rs`:
  `TargetResolver::resolve()` re-reads+re-validates the portfile on **every** call
  (`validate::validate_portfile`, `capture_lock=false`) and yields a `DialTarget`
  with `ws_url()` (token-free, `ws://127.0.0.1:<port>/ws?v=<p>&sg=<sg>`, loopback
  by construction) and `bearer()` (per-start token, `Redacted`, native-only).
  Errors are the closed `SupervisorError` set (`src/error.rs`). Validation order
  (`src/validate.rs`): data-dir binding → loopback → declared generation → PID-
  bound health → served generation → token shape. `TargetResolver` is `Clone` and
  carries no `Sidecar`, so a transport can hold it without being able to kill the
  daemon.
- **Codec** — `crates/jeliya-codec`: owns the protocol's *only* JSON. Today it
  encodes/decodes the **daemon** direction only (`decode`→`Frame::Request`,
  `encode_reply`, `push_to_bytes`). There is **no client-direction encoder for a
  request envelope nor decoder for a reply/push envelope** — #172 must add one
  (see §6). `PROTOCOL = 2`, `MIN_PROTOCOL = 2`, `MAX_REQUEST_ID`.
- **Wire protocol** — `docs/protocol-v2.md`: gate on the upgrade
  (`GET /ws?v=2&sg=<sg>`, loopback `Host`/`Origin`, `Authorization: Bearer
  <token>` for native, refusals `426`/`401`/`403`/`503`); **Layer 2 `hello`** is
  the daemon's *first Text message after upgrade*, exactly one, `{t:"hello",
  protocol, storage_generation, limits, subject, resume}`
  (`jeliya_api::Hello`, `crates/jeliya-api/src/push.rs`). Every hello/request/
  reply/push is one UTF-8 **Text** message; every byte-stream record is one
  **Binary** message; no WebSocket compression; no JSON `null` carries meaning.
- **Boundary constraints** — `crates/jeliya-client/tests/boundaries.rs`:
  (a) the library tree scanned with `--no-default-features --edges no-dev` must
  contain **no** `tokio`/`tungstenite`/`tokio-tungstenite`/`iroh`/`websocket`/
  `dioxus`/`tao`/`wry`; (b) **no `serde_json::Value`** in any `src/**` file;
  (c) `src/kernel/**` and `src/reconcile/**` must carry no `std::time`/
  `Instant::now`/`SystemTime`/`getrandom`/`rand::`/`tokio` token (sans-IO scans).
  `crates/jeliya-supervisor/tests/boundaries.rs` additionally asserts
  `jeliya-supervisor` is **absent from `jeliya-ui`'s `web` wasm graph**.
- **Daemon WS stack** — `crates/jeliyad/Cargo.toml` uses `tokio-tungstenite =
  "0.30"` (server via `hyper-tungstenite`), so the workspace lock already carries
  it; the client adapter aligns to `tokio-tungstenite = "0.30"`.

## 3. Scope / non-goals

In scope (issue): native WebSocket I/O, `Authorization: Bearer` header, target
resolution on every attempt, daemon-status/protocol agreement before "connected",
reconnect/stop, and redacted diagnostics.

Non-goals (issue): spawning/stopping `jeliyad` (#170 owns process ownership; the
adapter holds only a `TargetResolver`, never a `Sidecar`); browser same-origin
sessions (#171); platform file services (#174); direct core calls (#173).

Explicitly **not** re-implemented here: the resync/reconcile logic — reconnect
just produces the correct lifecycle events + generation bumps that #169's
`Reconciler` already consumes.

## 4. Owning module & feature story

Because the `Transport`/`Driver` seam is `pub(crate)`, all of #172 lands **inside
`jeliya-client`**, behind a **default-off, native-only** feature, mirroring the
existing `example`/`dioxus` precedent that already keeps a heavy optional tree out
of the default `--no-default-features` boundary scan.

### 4.1 New feature `ws-native`

```toml
# crates/jeliya-client/Cargo.toml
[features]
ws-native = ["dep:tokio", "dep:tokio-tungstenite", "dep:http", "dep:url", "dep:jeliya-supervisor"]

[dependencies]
tokio = { version = "1", features = ["rt", "net", "time", "io-util", "sync"], optional = true }
tokio-tungstenite = { version = "0.30", optional = true }        # aligns with jeliyad
http = { version = "1", optional = true }                        # build the upgrade Request + headers
url = { version = "2", optional = true }
jeliya-supervisor = { path = "../jeliya-supervisor", optional = true }
```

- `rt` (not `rt-multi-thread`): the library **borrows the caller's runtime** and
  uses `tokio::spawn`; it never constructs a runtime (same rule the supervisor
  follows). No `macros`.
- All new source is gated `#[cfg(all(feature = "ws-native", not(target_arch =
  "wasm32")))]` and lives under `src/adapter/` (a fresh directory) — **outside**
  `src/kernel/**` and `src/reconcile/**`, so it may legitimately use `tokio`/
  `std::time` without tripping the sans-IO scans, while those scans keep the core
  pure.
- The default library build, the wasm build, and the MSRV/clippy
  `--all-targets --no-default-features` jobs compile none of it, so
  `boundaries.rs` (a) stays green unchanged.

### 4.2 Feature-unification / wasm safety

Cargo unifies features per package. The web (wasm) binary must never enable
`jeliya-client/ws-native`; `jeliya-ui`'s `web` feature must not name it. This
keeps `tokio`/`tokio-tungstenite`/`jeliya-supervisor` out of the wasm graph and
preserves `jeliya-supervisor`'s existing "absent from the `jeliya-ui` web graph"
assertion. #172 **adds a jeliya-client boundary test** that resolves the
`jeliya-ui` `web` feature tree for `wasm32-unknown-unknown` and asserts none of
`tokio`, `tokio-tungstenite`, `jeliya-supervisor`, `ws-native` appears — the
jeliya-client-side twin of the supervisor guard.

## 5. Module layout (new, under `crates/jeliya-client/src/adapter/`)

```
src/adapter/
  mod.rs          // #[cfg]-gated re-exports: connect_ws_native, TargetSource, Dial,
                  //   DialResolveError, NativeClient, NativeClientConfig, NativeError.
  source.rs       // the injected TargetSource seam + Dial/DialResolveError; the
                  //   provided `impl TargetSource for jeliya_supervisor::TargetResolver`.
  runtime.rs      // the reusable real async driver runtime: the DriverIo seam + the
                  //   RealDriver task loop that steps the Core against live effects.
  ws_native.rs    // the native WebSocket DriverIo: dial/hello agreement, framed
                  //   send/recv, close-code classification, stop.
  clock.rs        // monotonic Instant → logical Tick mapping (1 tick = 1 ms).
```

`src/lib.rs` gains, under the same `cfg`, `mod adapter;` and public re-exports.

### 5.1 Refactor to make the async shell reusable (touches #168, red-before-green)

The runtime shell in `kernel/mod.rs` is currently welded to the in-memory driver.
Extract the transport-touching effects behind a `pub(crate)` seam so both the
in-memory controller and the native driver reuse the *same* delivery/wake
machinery (the `Deferred`/`Runtime`/`drain_delivery` logic is subtle and must not
be duplicated).

```rust
// kernel/mod.rs (or a new kernel/driver_io.rs), pub(crate)
pub(crate) trait DriverIo: Send {
    fn send(&mut self, frame: WireFrame);        // encode + enqueue to the live sink, or record
    fn arm_timer(&mut self, id: TimerId, at: Tick);
    fn cancel_timer(&mut self, id: TimerId);
    fn dial(&mut self, token: u64);              // begin one dial attempt
    fn cancel_dial(&mut self);                   // cancel dial/backoff (total stop)
}
```

- `Shared` becomes generic over (or holds a boxed) `DriverIo`; `apply_one`'s
  `Send`/`ArmTimer`/`CancelTimer`/`Dial`/`CancelDial` arms delegate to it. The
  `Settle`/`DropSender`/`Emit`/`CloseBus` arms are unchanged (already neutral).
- The existing in-memory driver becomes `struct InMemoryIo` implementing
  `DriverIo` by recording into the same `outbound`/`timers`/`dialing` fields it
  uses today; `KernelController` keeps its exact public surface. **All existing
  `test-transport` and `kernel_fault` tests must stay green byte-for-byte** — this
  is the acceptance gate for the refactor.
- The native side (`runtime.rs`) provides `NativeIo` implementing `DriverIo` by
  handing effects to async tasks (see §7), and a `RealDriver` that owns the
  `Runtime`, exposes a `KernelBackend`-shaped `ClientBackend` to the handle, and
  a re-entrancy-safe `inject(Input)` the async tasks call (the moral equivalent of
  `KernelController::drive_serialized`, driven by real events instead of tests).

Rationale for this seam over a parallel runtime: duplicating the deferred-wake /
delivery-queue / ABBA-avoidance logic is the single highest-risk option; one
tested shell with two `DriverIo` impls keeps the wake hygiene proven once.

## 6. Codec: the client direction (new, in `jeliya-codec`)

The architecture forbids JSON escaping the codec, and `jeliya-client` forbids
`serde_json::Value` in its source; therefore request-envelope assembly and
reply/push decoding **belong in the codec**, not the adapter.

Add a small client-facing surface to `jeliya-codec`:

```rust
// jeliya-codec/src/client.rs (new), re-exported from lib.rs
/// Encode one outbound request envelope as protocol-v2 Text bytes.
/// `in_json` is the already-serialized `in` object (the kernel's RawJson text);
/// the codec assembles `{ "id", "op", ["op_id"], "in": <in_json> }`.
pub fn encode_request(id: u64, op: &str, op_id: Option<&OpId>, in_json: &str) -> Vec<u8>;

/// Decode one inbound Text message the daemon may send a client.
pub enum ClientFrame {
    Hello(Hello),                                   // the Layer-2 first frame
    Reply { id: u64, result: Result<String, ApiError> }, // out re-serialized to text
    Push(Push),
    Malformed(ApiError),                            // decodes to no usable frame
}
pub fn decode_client_frame(bytes: &[u8], bounds: &CodecBounds) -> Result<ClientFrame, CodecError>;
```

- `decode_client_frame` distinguishes `hello` (has `t:"hello"`), a reply (has
  `id`, no `t`), and a push (has `t`, no `id`) exactly as the record fixes; it
  applies the same bounded-parse ceilings as the daemon decoder and re-serializes
  `out` back to a text string so `jeliya-client` never touches `serde_json::Value`.
- Golden vectors + the existing v2 conformance corpus validate the round trip
  (`jeliya-codec/tests/golden.rs`).
- The adapter's `ws_native.rs` translates `ClientFrame` into the kernel's
  `pub(crate) Inbound`/`WireReply` and into `Hello` handling (§7.3). Binary
  messages map to the stream path (#269 owns stream lifecycle; #172 wires
  Binary→`Inbound` stream records but adds no new stream semantics).

## 7. The native transport: dial → agreement → serve → reconnect → stop

### 7.1 Injected target source (`source.rs`)

The resolver is **injected**, never constructed by the adapter (dependency
inversion; the adapter cannot spawn or kill a daemon):

```rust
pub struct Dial { pub url: url::Url, pub bearer: jeliya_supervisor::Redacted<String> }

pub enum DialResolveError {
    /// Wrong daemon / attack shape: fail closed, surface the reset path, no auto-retry.
    Terminal(String),
    /// Not-yet / torn / no-listener: recoverable, drive backoff and retry.
    Transient(String),
}

pub trait TargetSource: Send + Sync + 'static {
    fn resolve(&self) -> futures::future::BoxFuture<'static, Result<Dial, DialResolveError>>;
}
```

Provided glue (the literal "reuse the supervisor" edge), gated with the
`jeliya-supervisor` dep:

```rust
impl TargetSource for jeliya_supervisor::TargetResolver {
    fn resolve(&self) -> BoxFuture<'static, Result<Dial, DialResolveError>> {
        let this = self.clone();                       // TargetResolver is Clone; no Sidecar captured
        Box::pin(async move {
            match this.resolve().await {
                Ok(t) => Ok(Dial { url: t.ws_url().clone(), bearer: /* move redacted token */ }),
                Err(e) => Err(classify(e)),
            }
        })
    }
}
```

`classify(SupervisorError)` — the **fail-closed vs. retry** table (§8):

| `SupervisorError` (resolve-reachable) | `DialResolveError` | Core input |
|---|---|---|
| `GenerationMismatch` | **Terminal** | `GateRefused{token}` → reset path |
| `NonLoopback` | **Terminal** | `GateRefused{token}` |
| `DataDirMismatch` | **Terminal** | `GateRefused{token}` |
| `Handshake` (dial-URL build bug) | **Terminal** | `GateRefused{token}` |
| `Stale` (no healthy daemon) | Transient | `DialFailed{token}` → backoff |
| `PortfileMissing` | Transient | `DialFailed{token}` |
| `PortfileUnreadable` (torn / bad token) | Transient | `DialFailed{token}` |
| `Wedged` | Transient | `DialFailed{token}` |

The redacted `bearer` is exposed **only** at the moment the header is built, never
stored on the driver beyond the live connection, never logged, never surfaced to
the handle.

### 7.2 Dial (native `DriverIo::dial(token)`)

On `Action::Dial{token}` the runtime spawns one dial task bound to `token`:

1. `source.resolve().await`.
   - `Err(Terminal)` → `inject(GateRefused{token})` (no retry).
   - `Err(Transient)` → `inject(DialFailed{token})` (backoff/retry, budgeted).
2. Build the upgrade `http::Request` from `Dial.url` with headers:
   `Authorization: Bearer <token>`, `Host: 127.0.0.1:<port>`, `Origin:
   http://127.0.0.1` (or omitted — the daemon accepts absent `Origin`), plus the
   RFC 6455 upgrade headers `tokio-tungstenite` fills in. The `?v=&sg=` query is
   already on the URL. **No compression** subprotocol/extension is offered.
3. `tokio_tungstenite::connect_async(request)` bounded by a connect deadline.
   Classify the handshake outcome by HTTP status (from the tungstenite error /
   response):

   | Result | Core input | Rationale |
   |---|---|---|
   | `101` upgrade | proceed to §7.3 | |
   | `401` unauthenticated | `DialFailed{token}` | token rotated on restart; next resolve heals |
   | `403` forbidden_origin | `GateRefused{token}` | should not happen on loopback; fail closed |
   | `426` protocol/storage mismatch | `GateRefused{token}` | wrong generation; reset path |
   | `503` resource_exhausted | `DialFailed{token}` | transient capacity |
   | connect refused / reset / TLS/other | `DialFailed{token}` | daemon not up yet |

### 7.3 Protocol agreement (the hello gate) — never "connected" before agreement

After `101`, **do not** report `Connected` yet. Read the first **Text** message,
bounded by a short hello deadline, and require it to be a valid
`ClientFrame::Hello` whose `protocol == expected.protocol` and
`storage_generation == expected.storage_generation`:

- Valid, matching hello → `inject(Connected{token})` (the core bumps generation).
  Capture `hello.subject` / `hello.resume` / `hello.limits` on the connection for
  surfacing (feeds #169 reconcile and #178's connection snapshot; #172 stores them
  but adds no new public field beyond what the seam already exposes).
- Non-hello first frame, or a hello with a mismatched generation, or an
  application close `4001`/`4006` → `GateRefused{token}` (terminal). This is the
  **third** independent agreement check (resolver health + daemon gate + hello),
  so "connected before protocol agreement" is impossible.
- Connection dropped before hello, or hello timeout → `DialFailed{token}`
  (transient).

Only once `Connected{token}` is injected does the runtime start the per-connection
read/write tasks and mark this connection's generation live.

### 7.4 Serve (per live connection)

- **Send** (`DriverIo::send(frame)`): `codec::encode_request(frame.id, frame.op,
  frame.op_id.as_ref(), frame.input.as_str())` → one WebSocket **Text** message
  pushed to the connection's bounded write channel; the write task flushes it.
  A write error → `inject(Interrupted{generation})` (the send/close race the
  kernel already models). `frame.input`/`op_id` are never logged (WireFrame Debug
  already redacts them).
- **Receive**: the read task pulls each WebSocket message:
  - Text → `codec::decode_client_frame`:
    - `Reply{id,result}` → `Inbound::Reply{generation, id, WireReply::Ok(text)|Err(ApiError)}`.
    - `Push(push)` → `Inbound::Push{generation, push}`.
    - `Hello(..)` after the first is a protocol violation → `Interrupted` +
      close (a second hello is malformed per the record).
    - `Malformed(..)` → `Inbound::Malformed` (strands nothing).
  - Binary → decode as a stream record (`jeliya_codec::decode_stream_record*`) and
    feed the kernel's stream path (#269); no new stream semantics here.
  - Close / EOF / read error → `inject(Interrupted{generation})`.
  Every `Inbound` is tagged with **this connection's** generation, so a delayed
  frame from a replaced connection is fenced by the core (§K7).

### 7.5 Timers & clock (`clock.rs`, native `DriverIo::arm_timer`/`cancel_timer`)

- `now()`: `(Instant::now() - base).as_millis()` clamped to `Tick`, **1 tick =
  1 ms** (matches the `KernelLimits` default comment: 30 000 ticks ⇒ 30 s
  deadline). `base` is captured once at construction.
- `arm_timer{id,at}`: spawn/replace a `tokio::time::sleep_until(base + at_ms)` task
  keyed by `id`; on fire, `inject(TimerFired(id))`. `cancel_timer(id)` aborts it.
  Backoff, per-call deadlines, hello/connect deadlines, and stall/stream deadlines
  all flow through the core's logical timers — the driver only realizes them.

### 7.6 Reconnect (routes through #169)

A live-connection loss → read/write task injects `Interrupted{generation}` → the
core drives capped, jittered backoff (`ArmTimer`) and re-issues `Dial{token'}`.
`resolve()` runs again on **every** attempt, so a restart's new port/token/pid
heals transparently; a restart that changed generation is caught (Terminal). The
kernel bumps the connection generation on the next `Connected`; #169's
`Reconciler` observes the generation change and issues `stream.resync`/roster
rebuilds. #172 adds no resync logic — it only guarantees the correct
lifecycle+generation signal.

### 7.7 Stop (total)

`handle.stop()` → core total stop: `CancelDial`, settle every outstanding call,
`CloseBus`. The native runtime, on `cancel_dial`/stop, aborts the dial task, the
read/write tasks, and all timer tasks, and drops the socket (sending a WebSocket
Close best-effort). No task, socket, or timer survives — asserted by the leak
checks the kernel fault suite already models (`outstanding()==0`, no armed timers)
plus a native "all spawned tasks joined/aborted" assertion.

### 7.8 Replay policy (`stable_principal`)

`NativeClientConfig` sets `KernelConfig.stable_principal = false` by default
(replay disabled) — the safe default the kernel documents for a socket adapter:
the dedup ledger is in-memory and a daemon restart empties it, so replaying a
mutation against a new incarnation could double-execute. With replay off,
in-flight mutations across a reconnect settle honestly as `Disconnected{Unknown}`
and #169 reconciles state. When #270 (`hello` daemon incarnation) lands and the
kernel's `Connected` carries an incarnation to fence on, #172 can thread
`hello`'s incarnation through and opt in; that is a **follow-up**, not part of this
slice, and the spec must not enable replay without it.

## 8. Security & correctness invariants (issue "Security and correctness")

- **Never trust portfile host data.** The dial URL is `ws://127.0.0.1:<port>` built
  by the supervisor from a hardcoded loopback + validated port; a portfile
  advertising a non-loopback `ws`/`http` is `NonLoopback` → Terminal. The adapter
  never parses a host out of untrusted portfile fields.
- **Token stays native and redacted.** The bearer is a `Redacted<String>` exposed
  only when composing the `Authorization` header; it is never placed in a URL,
  a log line, a `Debug`, a Dioxus prop, or a DOM attribute, and the `ClientHandle`
  exposes only typed calls — so the token is unreachable from WebView JS.
  Diagnostics reuse `kernel::diag::Redacted` and `supervisor::Redacted`; any RPC/
  transport error text is scrubbed of the token before it enters a surfaced error
  (mirroring `Sidecar::stop_adopted`'s redaction).
- **No "connected" before health + protocol agreement** (§7.3): resolver health
  proof + daemon upgrade gate + `hello` generation match, all required.
- **Fail closed on stale/malformed discovery**: Terminal errors do not auto-retry;
  transient ones retry within the bounded reconnect budget and then settle to
  `Failed`. A malformed/torn portfile never yields a dialed connection.
- **Generation & token fencing**: every `Inbound`/dial outcome is generation/token
  tagged; stale frames and retired dials cannot tear down a successor (core §K7).
- **Bounded everything**: connect/hello/read deadlines; bounded write channel;
  frame size bounded by codec `CodecBounds` + served `max_frame_bytes` from
  `hello.limits`; no unbounded buffering of an inbound message.

## 9. Error model & observability

- Public `NativeError` (the constructor/driver surface) is a small closed enum:
  `Runtime` (no tokio runtime / spawn failure), `Config` (bad limits). Per-call
  and lifecycle errors continue to flow through the existing `CallError`/`State`/
  `ClientEvent` surface — #172 adds **no** new public call-error variants (the
  seam already pre-wired them, #167/#168).
- Structured, **redacted** tracing at the adapter boundary only: dial start/result
  (status class, never token), generation transitions, reconnect attempts (n/max),
  stop. Never the bearer, never a frame payload, never an `op_id`.
- Lifecycle `State`/`StateChanged` events are the core's; the adapter only injects
  the inputs that produce them.

## 10. Test strategy

Two tiers. Unit/CI tests must not require a daemon; the real-daemon matrix is the
issue's verification list.

### 10.1 Unit / deterministic (CI, `--features ws-native`)

- **`DriverIo` refactor regression**: the entire `test-transport` + `kernel_fault`
  suite passes unchanged with `InMemoryIo` as a `DriverIo` impl (the refactor's
  acceptance gate).
- **Fake `TargetSource`**: a scripted resolver (returns `Ok`/`Terminal`/
  `Transient` on demand) drives the runtime with a **fake in-process WebSocket**
  (a loopback `tokio` listener speaking the minimum: accept upgrade, send a
  scripted `hello`, echo/deny) to assert, without `jeliyad`:
  - resolve runs on **every** dial attempt (counter);
  - Terminal resolve → `GateRefused`/reset, no retry; Transient → backoff+retry;
  - `401`→retry heals with a rotated token; `426`→terminal; `503`→retry;
  - no `Connected` until a matching `hello`; wrong-generation `hello`→terminal;
    missing `hello`→transient;
  - request Text framing round-trips a reply; a push surfaces as an event; a
    malformed frame strands nothing;
  - reconnect bumps generation and re-resolves;
  - stop aborts all tasks/timers/socket (`outstanding()==0`, no armed timers, the
    fake server observes a closed socket).
- **`clock.rs`**: monotonic Instant→Tick mapping is monotonic and 1 ms/tick.
- **codec client direction**: golden vectors for `encode_request` and
  `decode_client_frame` (hello/reply/push/malformed), plus a v2-corpus round-trip.

### 10.2 Boundary / structural (CI)

- Existing `boundaries.rs` (a)/(b)/(c) stay green (feature default-off; adapter
  under `src/adapter/**`, not kernel/reconcile; no `serde_json::Value`).
- **New** jeliya-client boundary test: the `jeliya-ui` `web` (wasm32) feature tree
  contains none of `tokio`, `tokio-tungstenite`, `jeliya-supervisor`, `ws-native`.
- The supervisor's own "absent from `jeliya-ui` web graph" test remains green.

### 10.3 Real-daemon integration matrix (issue "Verification")

An ignored-by-default integration test (`#[ignore]`, run explicitly, e.g.
`cargo test -p jeliya-client --features ws-native -- --ignored`) that spawns a
real `jeliyad` (via the supervisor or a scripted harness) and exercises, each
mapping to an AC:

1. **Token rotation** — restart the daemon (new token); an active client heals on
   reconnect via re-resolve. (AC: token rotation heals.)
2. **Stale portfile** — leave a portfile with no live daemon; the client stays
   `Connecting`/retries and never dials a bad endpoint, fails closed after budget.
   (AC: stale/malformed discovery fails closed.)
3. **Abrupt daemon death** — SIGKILL the daemon mid-session; the client observes
   the loss, backs off, and (if respawned) reconnects; the gap routes through
   #169. (AC: gap/reconnect routes through #169.)
4. **Same-generation adoption** — restart same generation (new pid/port/token);
   reconnect succeeds and heals. (AC: exact-generation adoption heals.)
5. **Exact-version mismatch** — point the resolver's expected generation off by
   one (or run a mismatched daemon); resolve → `GenerationMismatch` → terminal
   `GateRefused`, reset path, never connected. (AC: stale/malformed fails closed +
   only verified endpoints dialed.)
6. **Reconnect** — drop and restore the connection; generation bumps, calls resume.
7. **Stop** — `handle.stop()` tears down cleanly; no lingering task/socket.
8. **Loopback-only** — assert the only endpoint ever dialed is
   `127.0.0.1`/loopback and the token only ever appears in the request header.

## 11. Acceptance criteria (from the issue) → where satisfied

- Resolver runs on **every** connection attempt — §7.2 step 1; unit counter test;
  matrix #1/#4/#6.
- Only **verified loopback** endpoints dialed — supervisor `DialTarget` (hardcoded
  `127.0.0.1` + validated port); §8; matrix #8.
- Auth tokens **native and redacted** — §8; `Redacted`; unit + matrix #8.
- **Exact-generation adoption and token rotation heal** — §7.6/§8; matrix #1/#4.
- **Stale/malformed discovery fails closed** — §7.1 table, §7.3; matrix #2/#5.
- **Gap/reconnect routes through #169** — §7.6; matrix #3/#6.

## 12. Risks & mitigations

- **Refactoring #168's tested async shell** (the `DriverIo` seam). *Mitigation*:
  keep `InMemoryIo` behaviourally identical; the full `test-transport`/
  `kernel_fault` suite passing unchanged is the gate; make the change
  red-before-green with no test weakened.
- **Building the first real async runtime loop** (subtle wake/ABBA/re-entrancy).
  *Mitigation*: reuse the existing `Deferred`/delivery machinery via the shared
  shell; do not duplicate it. `inject(Input)` mirrors `drive_serialized`.
- **Codec scope creep** (client direction). *Mitigation*: additive `client.rs` in
  jeliya-codec with golden + corpus coverage; keeps all JSON in the codec and
  honours jeliya-client's no-`Value` rule.
- **Feature unification pulling tokio/supervisor into wasm**. *Mitigation*:
  default-off `ws-native`; web feature never names it; new boundary test guards it.
- **`tokio-tungstenite` version drift vs. the daemon**. *Mitigation*: pin to
  `0.30` (jeliyad's), already in the lock; no new major.
- **Replay double-execution across restart**. *Mitigation*: `stable_principal =
  false` until #270 provides an incarnation fence; never enable replay in this
  slice.

## 13. Open questions

1. **Sequencing vs. #171.** The kernel note names #171 as the "first adapter
   slice" that introduces the runtime loop, but #172 is *not* listed as blocked by
   #171. This spec has #172 build the reusable `DriverIo` runtime (native), which
   #171/#173 then reuse. If #171 is intended to land the shared runtime first, that
   work moves to #171 and #172 supplies only `NativeIo`. **Recommendation**: land
   the shared runtime here (native is not blocked and needs it); document it as the
   substrate #171/#173 consume.
2. **Home of the supervisor→`TargetSource` bridge.** Provided as `impl TargetSource
   for jeliya_supervisor::TargetResolver` inside jeliya-client (ws-native), which
   makes jeliya-client optionally depend on jeliya-supervisor. Alternative: keep
   jeliya-client supervisor-free (pure `TargetSource`) and place the bridge in the
   eventual desktop shell crate. **Recommendation**: the in-crate provided impl —
   it is the literal "reuse the supervisor/target resolver" and stays behind the
   default-off feature.
3. **`Origin` header on the native upgrade.** The daemon accepts an absent
   `Origin` and requires loopback if present. **Recommendation**: omit `Origin` on
   native (nothing requires it) to reduce surface; revisit if the daemon ever
   mandates it.
4. **Binary/stream wiring depth.** #269 owns stream lifecycle; #172 must route
   Binary messages into the kernel stream path but should add no stream semantics.
   Confirm the exact `Inbound` shape #269 expects for a decoded stream record
   before wiring (it may be a follow-up if #269's driver-facing shape is not yet
   `pub(crate)` on this base).
5. **Where does a `jeliyad` come from for reconnect after death?** #172 holds only
   a resolver, not a `Sidecar`; respawn is the desktop shell's supervisor
   responsibility (out of scope). Confirm the shell keeps the `Sidecar` alive so
   the resolver eventually re-validates a fresh daemon.

## 14. Rollout / rollback

- **Rollout**: additive, default-off `ws-native`. No behaviour changes for any
  existing build (default library, wasm, MSRV/clippy, jeliya-ui web). Landing
  order within the PR: (1) codec client direction + tests; (2) `DriverIo` refactor
  with the in-memory suite green; (3) native `DriverIo` + runtime + constructor;
  (4) unit + boundary tests; (5) ignored real-daemon matrix.
- **Rollback**: because everything is behind `ws-native` and no default surface
  changes, reverting the feature (or not enabling it) fully removes the adapter;
  the kernel's in-memory driver and every other crate are untouched. The `DriverIo`
  refactor is the only non-gated change; it is behaviour-preserving and reverts
  with the PR.
```
