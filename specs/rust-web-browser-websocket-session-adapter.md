# Browser WebSocket / session adapter (`WsWeb`) — implementation spec

- **Issue:** #171 — [Rust][Web]: Implement the browser WebSocket/session adapter
- **Program:** #156 (Dioxus clean-slate). Milestone M2 (client runtime & platform adapters).
- **Depends on:** #158 (lifecycle seam), #168 (bounded client kernel), #169 (authoritative reconciler). All three are landed on `main`.
- **Status:** spec — no production code in this document.
- **Owning crate/module:** `crates/jeliya-client` (a new `ws_web` transport module + a reusable async runtime), plus a small additive **client edge** in `crates/jeliya-codec`.

---

## 1. Outcome

Bind the shared, transport-independent client kernel (#168) to the **browser
WebSocket and `fetch` APIs**, producing a real `jeliya_client::ClientHandle`
that the shared Dioxus UI can drive on `wasm32-unknown-unknown` — with **no
Iroh, no `jeliya-core`, no native transport, no desktop supervision** in the
wasm dependency graph. `WsWeb` is the *sole* browser client for the v2 stack;
React transport compatibility is explicitly not required (clean-slate cutover).

This is the **first of the four adapters** the kernel seam was designed for
(`WsWeb` #171, `WsNative` #172, `DirectClient` #173, plus the reference mock).
Per the kernel's own contract
(`crates/jeliya-client/src/kernel/transport.rs`, "Runtime scope §K13"), the
**generic async runtime loop that binds a real `Driver`'s transport, clock, and
dialer to the core — and the public construction path for it — lands with this
slice** and is reused verbatim by #172/#173.

---

## 2. Ground truth (what already exists — verify before building)

### 2.1 The kernel seam and its transport traits
- `ClientHandle` (`src/handle.rs`) is `Arc<dyn ClientBackend>`; `ClientBackend`
  (`src/backend.rs`) is `Send + Sync`, object-safe, and carries only erased
  `RawJson` text plus routing facts (`op`, `mutating`, `op_id`). The typed
  encode/decode happens at the handle edges; the token/payload never appears in
  a public signature.
- The **sans-IO core** (`src/kernel/core.rs`) is a pure state machine. It
  consumes `Input` and returns `Vec<Action>`. Relevant inputs/actions for an
  adapter:
  - Inputs: `Start`, `Stop`, `Dispatch{call_id,call}`, `Connected{token}`,
    `Interrupted{generation}`, `DialFailed{token}`, `GateRefused{token}`,
    `Inbound(Inbound)`, `TimerFired(TimerId)`, `Cancel(call_id)`.
  - Actions: `Send(WireFrame)`, `ArmTimer{id,at}`, `CancelTimer(id)`,
    `Dial{token}`, `CancelDial`, `Settle(call_id,result)`,
    `DropSender(call_id)`, `Emit(ClientEvent)`, `CloseBus`.
  - Every dial outcome is **token-fenced** (`Dial{token}` → `Connected{token}` /
    `DialFailed{token}` / `GateRefused{token}`); every inbound frame is
    **generation-tagged** (`§K7`). A straggler from a retired attempt or a stale
    generation is dropped by the core, not the adapter.
- `src/kernel/transport.rs` **defines but does not implement** the seam an
  adapter fills: `Transport` (`send`, `poll_inbound`), `Driver` (`dial`,
  `cancel_dial`, `now`), and the frame types `WireFrame`, `WireReply`,
  `Inbound { Reply, Push, Malformed }`, `TransportClosed`. These are all
  `pub(crate)`; **an adapter therefore lives inside `jeliya-client`.**
- `src/kernel/mod.rs` already contains the reusable async-shell machinery:
  `Runtime { shared: Mutex<Shared>, delivery: Mutex<VecDeque<Deferred>>,
  draining: AtomicBool }`, `Deferred`/`DeferredWake`, and `KernelBackend`
  (`ClientBackend` impl) with **re-entrancy-safe deferred wake delivery** — a
  waker is invoked only after the `Shared` lock is dropped, and cross-thread
  delivery order equals drive order. This machinery was **built for exactly an
  inline / re-entrant executor**, which is what a browser is. `WsWeb` reuses it.
- The only concrete driver today is the deterministic in-memory
  `KernelController` (feature `test-transport`), which the four real adapters are
  diffed against by #175.
- `KernelConfig { limits: KernelLimits, jitter_seed, stable_principal }`.
  `stable_principal` gates auto-replay across a reconnect (`§K5`).

### 2.2 The codec (#164)
- `crates/jeliya-codec` "owns the protocol's *only* JSON". It currently exposes
  the **server edge**: `decode(bytes)`→`Frame::Request` (client→daemon requests,
  rejecting replies/pushes), `Reply` (derives `Serialize + Deserialize`),
  `push_to_bytes`, and the `gate(...)` (server-side generation gate).
- It does **not** yet expose a **client edge** (encode a request envelope;
  decode an inbound reply/push/hello). #171 adds it (see §6.3).
- Envelope discrimination is fixed: a **reply carries `id` and never `t`**; a
  **push carries `t` and never `id`**; `Hello` is a push-shaped frame tagged
  `{"t":"hello", …}` (`jeliya_api::Hello`, `#[serde(tag="t", rename="hello")]`).
  Request ids are capped at `MAX_REQUEST_ID` (browser-`Number`-safe).

### 2.3 The daemon handshake (`crates/jeliyad/src/serve.rs`, `docs/protocol-v2.md`)
The daemon's control surface on `127.0.0.1:<port>`:
- **Layer 0 — `GET /api/health`** (unauthenticated, loopback `Host` only):
  `{ ok, pid, port, version, protocol, min_protocol, storage_generation, limits }`.
  This is the **only pre-connection source of `storage_generation`**, which the
  client must present at the gate.
- **`GET /api/session`** (served only to a loopback-`Origin` browser or a
  same-origin `Sec-Fetch-Site` request): `{ "token": "<daemon-token>" }`, with
  `Cache-Control: no-store`. This is the v1 browser handshake, **retained for
  the served UI** (the pairing-code / `POST /api/session` ticket flow is a #166
  follow-up, not yet implemented).
- **Layer 1 — `GET /ws?v=2&sg=<storage-generation>&cid=<client_id>`** with a
  credential (`?token=<bearer>` **or** `?ct=<connect-ticket>`; `Authorization:
  Bearer` also works for native but a browser `WebSocket` cannot set headers).
  The gate runs **before** the upgrade, in fixed order: loopback `Host`,
  loopback `Origin` (if present), present+supported `v`, present+equal `sg`,
  constant-time credential, then capacity (`503`, last). A refused upgrade
  returns a bare `err` body with `426`/`401`/`403`/`503`. **A browser cannot
  read that body or status** — a failed WS upgrade surfaces only as a generic
  `onerror`/`onclose` (code 1006). This is why daemon-status validation (§6.1)
  is done up front against `/api/health`.
- **Layer 2 — `hello`**: the daemon's **first Text message after upgrade is
  exactly one** `{ "t":"hello", "protocol":2, "storage_generation":…, "limits":…,
  "subject": present|absent, "resume": fresh|resumed }`. `subject.state` is a
  tagged variant, never null. `resume` is always `fresh` for a browser today.

### 2.4 The React reference being replaced (`ui/src/lib/client.ts`)
The v1 client already: derives a same-origin ws URL in `PROD` (falls back to a
fixed default in dev; honours `?daemon=<port>`), **re-fetches the token on every
connect attempt** so a daemon restart heals, redacts token-bearing URLs, and
auto-reconnects with capped jittered backoff. `WsWeb` preserves these
behaviours and moves the request-lifecycle correctness into the shared kernel.
React transport compatibility is **not** a goal.

### 2.5 The integration point (out of scope to flip here)
`crates/jeliya-ui/src/compose.rs::web_composition()` builds `(ClientHandle,
MockController)` from the mock and is documented as the exact place the "live
adapter (`WsWeb`, #168/#171) replaces the mock behind the same handle." #171
delivers the constructor `web_composition` will call; **flipping
`web_composition` to it is the production UI cutover, an explicit non-goal.**

---

## 3. Scope and non-goals (from the issue)

**In scope:** browser dialing; fresh `/api/session` authentication on every
attempt; daemon-status validation; timers; frame transport; browser-safe
diagnostics; the reusable async runtime that binds a `Driver` to the kernel core.

**Non-goals:**
- File picking/upload itself — `PlatformServices` (#174) owns it.
- Native portfile access.
- Iroh / `jeliya-core` in WASM.
- Production UI cutover (flipping `web_composition`).
- The pairing-code / connect-ticket browser flow and `POST /api/session/ticket`
  (#166 follow-up). The adapter is built so this swaps in behind an injected
  resolver without touching the transport or kernel (§6.4).
- Binary byte-stream (`JBS2`) transport for file read/share media (rides on the
  kernel stream lifecycle #269 and browser file features, which are out of
  scope here). The transport must not crash on an inbound Binary frame; see §6.7.
- Cross-reconnect auto-replay of mutations (requires the daemon-incarnation
  fence, #270). `WsWeb` runs with `stable_principal = false`; see §6.8.

---

## 4. Crate / module layout and the wasm boundary

### 4.1 Where the code lives
Because the `Transport`/`Driver`/`Inbound`/`WireReply`/`Core`/`RawJson` types
are `pub(crate)` in `jeliya-client`, **the adapter must live in
`jeliya-client`.** Add:

```
crates/jeliya-client/src/
  kernel/
    runtime.rs        # NEW: generic async runtime binding any Driver to the core
    transport.rs      # extend the Driver trait (see §6.2)
  ws_web/
    mod.rs            # NEW: WsWeb public constructor + Driver impl (wasm-only)
    session.rs        # NEW: /api/health + /api/session fetch + endpoint resolution
    socket.rs         # NEW: web-sys WebSocket Transport + JS callback plumbing
    timers.rs         # NEW: setTimeout/performance.now clock + timer service
    diag.rs           # NEW: RedactedUrl + browser-safe diagnostics
```

### 4.2 The dependency-boundary invariant (AC-1)
`tests/boundaries.rs` scans the library tree with `cargo tree -p jeliya-client
--no-default-features --edges no-dev` (the **host/native** target) and asserts
absence of `iroh`, `websocket`, `tungstenite`, `tokio`, `dioxus`, `tao`, `wry`.
The wasm adapter's browser crates therefore go under a **target-cfg + feature
gate** so they never enter the native library tree:

```toml
# crates/jeliya-client/Cargo.toml
[features]
# The browser WebSocket/session adapter (#171). Pulls the wasm-only browser
# crates; default-off so the native library tree stays free of them and the
# `cargo tree` boundary test is unaffected (it runs on the host target).
ws-web = [
  "dep:wasm-bindgen", "dep:wasm-bindgen-futures", "dep:js-sys", "dep:web-sys",
  "dep:futures-channel", # if not already reachable via `futures`
]

[target.'cfg(target_arch = "wasm32")'.dependencies]
wasm-bindgen = { version = "0.2.126", optional = true }   # match the workspace-pinned wasm-bindgen (#176)
wasm-bindgen-futures = { version = "0.4", optional = true }
js-sys = { version = "0.3", optional = true }
web-sys = { version = "0.3", optional = true, features = [
  "WebSocket","MessageEvent","CloseEvent","BinaryType","Event",
  "Window","Request","RequestInit","Response","Headers","AbortController","AbortSignal",
  "Location","console",
] }
```

Rules:
- Every item in `ws_web/` is `#[cfg(all(target_arch = "wasm32", feature = "ws-web"))]`.
  The module is invisible to the native build entirely, so no native `cfg` fork
  leaks into shared code. The generic `kernel/runtime.rs` is **not** wasm-gated
  (it is transport-independent and reused by native #172/#173).
- `web-sys`'s crate name does not contain the banned substring `"websocket"`
  (the type is `web_sys::WebSocket`), so the boundary scan is unaffected — but
  because it is target-`wasm32` only, it never appears in the host tree anyway.
- The **wasm `web` graph** of `jeliya-ui` is separately audited by
  `scripts/check-jeliya-ui-wasm-graph.sh` (CI job `jeliya-ui-web`). When #171's
  cutover lands (later), that script must confirm `ws-web` adds no Iroh/native
  edge to the wasm graph.

### 4.3 Pin discipline
`wasm-bindgen` must equal the version already pinned by the reproducible web
build (#176; `wasm-bindgen-cli@0.2.126` in CI). A mismatch breaks the pinned
`wasm-bindgen` CLI contract. Reuse the exact locked version; do not float.

---

## 5. The `!Send` / eager-dispatch constraint (the pivotal design decision)

`ClientBackend: Send + Sync` and `dispatch`/`stop` return `BoxFuture`
(`+ Send`). On `wasm32-unknown-unknown` (single-threaded), the browser handles
(`web_sys::WebSocket`, JS `Closure`s, `setTimeout` ids) are **`!Send`**, and the
crate is `#![forbid(unsafe_code)]`, so an `unsafe impl Send` is not available.
Relaxing the seam's `Send + Sync` bound is a #167 public-contract change with a
native blast radius and is rejected here.

**Resolution — the mailbox split (unsafe-free, seam-preserving):**
- The **`Send + Sync` backend** holds only `Send` state: the `Core`, the reply
  `oneshot` senders, the `EventBus`, the deferred-wake delivery queue (all
  reused from `kernel/mod.rs`), **plus a `Send` outbound *IO-action mailbox***
  (`VecDeque<IoAction>`) and a `Send` waker (`futures::task::AtomicWaker`).
  `dispatch`/`start`/`stop`/`Cancel` step the `Core` synchronously (dispatch
  stays eager), and any action that requires the browser (`Send`, `Dial`,
  `CancelDial`, `ArmTimer`, `CancelTimer`) is **enqueued** into the mailbox and
  the waker is signalled. `Settle`/`Emit`/`CloseBus` go through the existing
  re-entrancy-safe `Deferred` path unchanged.
- The **`!Send` IO pump** (a single future `spawn_local`'d at construction) owns
  the `!Send` browser resources. It `await`s the mailbox waker, drains
  `IoAction`s, performs the JS IO, and also surfaces JS-callback events
  (inbound frames, timer fires, dial outcomes, closes) back into the backend as
  `Input`s (which re-step the `Core`, possibly re-filling the mailbox). Because
  it is `spawn_local`'d, it need not be `Send`.
- **JS callbacks** (`onopen`/`onmessage`/`onclose`/`onerror`, `setTimeout`)
  capture an `Rc<RefCell<WsWebIo>>` and an `Arc<Runtime>` (the `Send` backend)
  and translate browser events into backend `Input`s. All on one thread; the
  `oneshot` reply futures returned to the UI are `Send` because they await a
  `Send` `oneshot` and touch no JS.

This confines every `!Send` / wasm concern to `ws_web/`, leaves the seam and the
native kernel untouched, and reuses the existing re-entrancy-safe delivery queue
that the kernel was already built with (an inline browser executor is precisely
the case it documents).

> Decision: the mailbox split is preferred over (a) `unsafe impl Send`
> (forbidden) and (b) a `cfg`-conditional `Send` bound on `ClientBackend`
> (public seam change, native cost, scattered `cfg`). Recorded as **OQ-1** if a
> reviewer wants the trade-off revisited.

---

## 6. Design

### 6.1 The connection lifecycle (dial → validate → Ready)

Each `Action::Dial{token}` the core emits triggers one **dial sequence** in the
`WsWeb` driver. The sequence obtains **fresh credentials and fresh daemon status
on every attempt** (AC-2), so a restart / token rotation heals through the
normal reconnect loop:

1. **Daemon-status validation — `GET {http_base}/api/health`.**
   - On network failure/timeout → `Input::DialFailed{token}` (recoverable;
     backoff retry).
   - Read `{ protocol, min_protocol, storage_generation, limits }`. If the
     client's supported generation (`2`) is **not** in `[min_protocol,
     protocol]` → `Input::GateRefused{token}` → terminal `State::Failed`
     ("protocol mismatch fails closed", no blind retries — the browser could not
     otherwise read the gate's `426`). Retain `storage_generation` as the `sg`
     to present.
2. **Fresh credential — `GET {http_base}/api/session`** (injected resolver, §6.4).
   - On failure → `Input::DialFailed{token}` (a rotated token 401s the WS
     upgrade on the *next* fetch, which heals; a fetch error retries). The token
     is **never** logged; only its presence/absence is.
3. **Open the socket** with a URL built by the injected endpoint resolver:
   `{ws_base}/ws?v=2&sg=<storage_generation>[&cid=<client_id>]` plus the
   credential query param (`token` or `ct`). Construct
   `web_sys::WebSocket::new(url)`; set `binary_type = "arraybuffer"`. A
   constructor throw → `Input::DialFailed{token}`.
   - The **raw URL is used exactly once** to construct the socket and is never
     retained in a form that can be rendered; diagnostics use `RedactedUrl`
     (§6.6).
4. **`onopen`** does **not** mean connected. The socket is open but unvalidated.
   Arm a **hello deadline** (a browser `setTimeout`, kernel-independent, e.g.
   `hello_timeout_ms`); if it fires before a valid hello → `Input::DialFailed`.
5. **First inbound frame → protocol validation (AC-3).** Decode the first Text
   frame (via the codec client edge, §6.3):
   - It **must** be a `Hello` with `protocol == 2` **and** `storage_generation ==`
     the `sg` presented. On success, cancel the hello deadline, record this
     socket's generation-at-connect, wire the steady-state callbacks (§6.7), and
     **only now** feed `Input::Connected{token}` → the core bumps the generation
     and transitions to `State::Ready`. **"Connected" is emitted strictly after
     protocol validation.**
   - A `Hello` with an unsupported `protocol` → `Input::GateRefused{token}`
     (terminal).
   - A `Hello` whose `storage_generation` disagrees with the presented `sg` (a
     reset raced the health read) → `Input::DialFailed{token}` (re-fetch health
     next attempt reconciles).
   - **Any first frame that is not a well-formed `hello`** (a reply, a push, a
     malformed frame) → protocol violation → `Input::GateRefused{token}`. A v2
     daemon always sends hello-first; anything else means we are not talking to a
     compatible v2 daemon, and it must fail closed rather than pretend Ready.
6. **`onclose`/`onerror` before a valid hello** → `Input::DialFailed{token}`
   (the browser cannot expose the gate's HTTP status; a `401`/`503`/transient
   drop all present as a recoverable failure and heal via fresh
   credentials/backoff; a true `426` is caught deterministically at step 1). If
   a WebSocket application close code is available and is `4001`
   (`protocol_unsupported`) or `4006` (`storage_generation_mismatch`), map to
   `Input::GateRefused{token}` instead (terminal / re-derive on retry per the
   close-code table below).

**Close-code → input mapping (post-open, best-effort — browsers surface app
close codes on a clean close):**

| Close code | Meaning | Input |
|---|---|---|
| 1000 / 1001 / 1006 (no app code) | normal / going away / abnormal | `Interrupted{gen}` if Ready, else `DialFailed{token}` |
| 4001 | `protocol_unsupported` | `GateRefused{token}` |
| 4002 | `unauthenticated` | `DialFailed{token}` (fresh token next attempt) |
| 4003 | `not_ready` | `DialFailed{token}` |
| 4004 | `idle_timeout` | `Interrupted{gen}` |
| 4005 | `frame_too_large` | `Interrupted{gen}` |
| 4006 | `storage_generation_mismatch` | `DialFailed{token}` (re-read health) |
| 4007 | `malformed_frame` | `Interrupted{gen}` |

The core owns all backoff, the reconnect-attempt ceiling (`max_reconnect_attempts`
→ `Failed`), and the honest post-send classification. The driver only maps
browser events to token/generation-tagged inputs; it never invents lifecycle.

### 6.2 The reusable async runtime (`kernel/runtime.rs`) and the extended `Driver`

`#171` lands the **generic runtime** the kernel deferred, reused unchanged by
#172/#173. Extend the `Driver` trait (`transport.rs`) into a full event source +
action sink so the runtime is transport-agnostic:

```rust
pub(crate) enum DriverEvent {
    Inbound(Inbound),                 // already generation-tagged by the driver
    Connected { token: u64 },
    DialFailed { token: u64 },
    GateRefused { token: u64 },
    Interrupted { generation: u64 },
    TimerFired(TimerId),
}

pub(crate) trait Driver: 'static {   // NOTE: no longer `Send` — see §5; the
    // native drivers remain `Send`, only the bound is relaxed so a wasm driver
    // can hold `!Send` browser handles. The runtime that owns it is spawned by
    // the platform (spawn_local on wasm; the supervisor runtime on native).
    fn poll_event(&mut self, cx: &mut Context<'_>) -> Poll<DriverEvent>;
    fn send(&mut self, frame: WireFrame) -> Result<(), TransportClosed>;
    fn dial(&mut self);
    fn cancel_dial(&mut self);
    fn arm_timer(&mut self, id: TimerId, at: Tick);
    fn cancel_timer(&mut self, id: TimerId);
    fn now(&self) -> Tick;
}
```

The runtime is the `Send + Sync` `ClientBackend` half (steps the core eagerly on
`dispatch`/`start`/`stop`/`Cancel`, enqueues browser-bound `IoAction`s to the
mailbox) plus the `!Send`-capable pump that owns the `Driver`. On wasm the two
communicate via the `AtomicWaker` mailbox (§5); on native the same runtime can
run wholly inside one task. `IoAction` mirrors the browser-bound `Action`s
(`Send`, `Dial`, `CancelDial`, `ArmTimer`, `CancelTimer`); `driver.now()` reads
the platform clock (never `std::time` inside the library, per §3 boundary
invariants).

> The existing `KernelController` (feature `test-transport`) is left untouched
> as the sans-IO reference #175 diffs against; the new runtime is an additive
> code path. Where feasible, the in-memory driver should also be expressible as
> a `Driver` so #175 can diff the *same* runtime — recorded as **OQ-2**.

### 6.3 Codec client edge (additive, in `jeliya-codec`)

The adapter must not hand-roll JSON ("the codec owns the protocol's only JSON").
Add a minimal, bounded **client edge** mirroring the server edge:

- `pub fn encode_request(id: u64, op: &str, op_id: Option<&OpId>, input_json: &str) -> Vec<u8>`
  — assemble `{"id":…,"op":"…"[,"op_id":"…"],"in":<input_json>}` with `input_json`
  spliced verbatim (it is the kernel's already-serialized `RawJson`; do **not**
  re-parse it). Reject an `op` longer than `max_op_len`.
- `pub enum ClientInbound { Hello(Hello), Reply { id: u64, result: Result<Box<RawValue>, ApiError> }, Push(Push), Malformed }`
  and `pub fn decode_client_frame(bytes, bounds) -> ClientInbound`, applying the
  same frame-size / depth / array bounds as the server `decode`, and
  discriminating on `t`/`id` exactly as the record fixes (reply = `id`, no `t`;
  push = `t`, no `id`; hello = `t == "hello"`; neither/both = `Malformed`).
  - The success `out` is captured as `Box<serde_json::value::RawValue>` (raw
    text) so the kernel's `RawJson` carries the daemon's **exact bytes** — no
    re-encode, so kernel byte-accounting and `O::Output` decoding are faithful.

The `WsWeb` driver maps `ClientInbound` → the kernel `Inbound`:
`Reply{id, Ok(raw)}` → `Inbound::Reply{ generation, id, WireReply::Ok(RawJson) }`;
`Reply{id, Err(api)}` → `WireReply::Err(api)`; `Push` →
`Inbound::Push{ generation, push }`; `Malformed` → `Inbound::Malformed`. The
`Hello` case is consumed by the dial sequence (§6.1), never delivered as
`Inbound`. **Generation** is the socket's generation-at-connect (§6.1 step 5),
so a late frame from a replaced socket is fenced by the core.

### 6.4 Injected endpoint & session resolution (production vs dev/tests)

Per the issue ("inject same-origin resolution for production and explicit
overrides for development/tests"):

```rust
pub enum Endpoint {
    /// Production: derive same-origin from `window.location` (scheme→ws/wss,
    /// host verbatim), matching the daemon that served the SPA. Loopback host,
    /// no hardcoded port — tracks the daemon's actual port and port-collision
    /// fallback for free.
    SameOrigin,
    /// Dev/tests: explicit HTTP base + WS URL (e.g. from `?daemon=<port>` or a
    /// fixed value against a test-supervised daemon on another loopback port).
    Explicit { http_base: String, ws_url: String },
}

/// How the credential is obtained and placed on the URL. Injectable so the
/// #166 pairing/ticket flow swaps in later with no transport change.
pub trait SessionResolver {                 // object-safe, `!Send`-friendly
    /// Returns (query-param-name, credential-value) or an error to retry.
    fn resolve(&self, http_base: &str) -> BoxLocalFuture<'_, Result<(&'static str, String), SessionError>>;
}
```

- **Production default:** `GetTokenResolver` → `GET /api/session` → `("token",
  <token>)`. Same-origin, so no CORS.
- **Injectable follow-up:** a `TicketResolver` (`POST /api/session` /
  `/api/session/ticket` → `("ct", <ticket>)`) lands with #166 without touching
  `socket.rs`/`runtime.rs` — keeping the **raw token out of the URL entirely**,
  the architecture's preferred end state (Decision 7, daemon-token boundary).
- **Tests:** an `ExplicitResolver` supplies a known credential against a
  test-supervised daemon.

`WsWebConfig { endpoint: Endpoint, session: Box<dyn SessionResolver>, kernel:
KernelConfig, hello_timeout_ms: u32 }`.

### 6.5 Timers & clock

- **Clock:** `driver.now()` reads `performance.now()` (monotonic ms) mapped to
  `Tick` (one tick = one ms, matching `KernelLimits` defaults' documented
  mapping). Never `std::time`/`Date.now` inside the library.
- **Kernel timers** (`ArmTimer`/`CancelTimer` for call deadlines and reconnect
  backoff): a `TimerService` over `window.setTimeout`/`clearTimeout`. On fire,
  the closure feeds `Input::TimerFired(id)`. Store `{TimerId → i32 handle}` for
  cancellation; clear all on stop/drop.
- **Hello deadline** (§6.1 step 4) is a driver-local `setTimeout`, independent of
  kernel timers.

### 6.6 Diagnostics & token redaction (AC-4)

- A `RedactedUrl` newtype renders **only** `scheme://host/path` — the entire
  query string (which carries `token`/`ct`/`cid`) is dropped from every
  `Debug`/`Display`. Reuse the existing `kernel::diag::Redacted` for
  credential-bearing values.
- The driver **never** constructs a diagnostic, error, or console line from the
  raw URL, the token, the ticket, the `cid`, or any payload bytes. It may name
  the `op`, counts, limits, lifecycle state, generation, and close code.
- Browser diagnostics (if any) go to `web_sys::console` at most; the crate does
  not adopt a logging framework. A unit test builds a `RedactedUrl` from a
  token-bearing URL and asserts the rendering contains neither the token, the
  ticket, nor the `cid`.

### 6.7 Frame transport (steady state)

Once Ready, on this socket's callbacks:
- **`onmessage` Text** → `decode_client_frame` → mapped `Inbound` (tagged with
  the socket generation) → `backend` input.
- **`onmessage` Binary (`ArrayBuffer`)** → out of #171's scope (byte-stream
  media, #269 + browser file features). Decode is deferred; for now feed
  `Inbound::Malformed` (the core drops it, stranding nothing) **or** hold behind
  a future `stream` feature. It must **never panic** on a Binary frame. Recorded
  as **OQ-3** (whether to wire `jeliya-codec::decode_stream_record` now).
- **`onclose`/`onerror`** → the close-code table (§6.1). If Ready →
  `Interrupted{generation}`; the core reclassifies in-flight work (never-sent =
  `DefinitelyNot`; sent = `Unknown`) and schedules backoff.
- **`Action::Send`** → `ws.send_with_str(encode_request(...) as str)`. A send on
  a broken pipe returns `TransportClosed` → the runtime feeds
  `Interrupted{generation}` (the send/close race, `§K14`).

Outbound requests are encoded via the codec client edge; the kernel's
`WireFrame` supplies `id`/`op`/`op_id`/`input`.

### 6.8 Kernel configuration for `WsWeb`

- `stable_principal = false`. A socket adapter cannot certify continuity of the
  daemon's dedup scope across a reconnect until `hello` carries a daemon
  incarnation to fence on (#270; not wired into the kernel in this tree — the
  `Input::Connected` here carries only `token`). With replay disabled, a
  mutation interrupted mid-flight settles `Disconnected { Unknown }` and the
  **reconciler re-reads authoritatively** (#169) — the honest, safe default.
  Enabling replay is a follow-up gated on #270.
- `cid` is **omitted** (fresh ephemeral principal per connection) for now; with
  replay off it does not affect correctness. A stable per-tab `cid` is a
  follow-up tied to enabling replay.
- `jitter_seed` from `crypto.getRandomValues` (one `u64` at construction; it is
  not a credential, only decorrelates reconnect storms).
- `limits`: `KernelLimits::default()`, overridable via `WsWebConfig`. The
  daemon's advertised `limits` (from `hello`/health) are informational for #171;
  the kernel's own bounds are what protect the client.

### 6.9 Restart / token-rotation / push-gap convergence (AC-5)

`WsWeb` produces a `ClientHandle`; the reconciler (#169) sits **above** it and
owns convergence. #171's job is to feed the events faithfully:
- On a daemon **restart**: the socket drops (`Interrupted`), the core backs off
  and re-dials; the fresh health read supplies the (possibly new)
  `storage_generation`, the fresh `/api/session` supplies the (new) token, the
  new `hello` is validated, and `Connected` re-enters `Ready`. The reconciler,
  seeing `Ready` (and any `resync_required`), re-baselines each active room.
- On a **push gap / local overflow**: the kernel lifts `gap`/`resync_required`
  to `ClientEvent::Gap`/`ResyncRequired`, and the fan-out surfaces `Lagged`; the
  reconciler consumes them. `WsWeb` adds nothing here — it must simply deliver
  pushes and lifecycle transitions honestly on the fan-out bus.

The real-browser test (§9) proves the restart and push paths converge end to
end through the already-landed reconciler.

---

## 7. Security & correctness checklist (maps issue "Security and correctness")

- **Host/origin/session checks preserved.** The daemon enforces loopback
  `Host`/`Origin` and the six-step gate server-side; `WsWeb` presents `v=2`, the
  fresh `sg`, and a fresh credential, and validates `hello` before Ready. It
  never bypasses or weakens these.
- **Token redaction (AC-4).** §6.6. The token/ticket/cid never enter a URL that
  is rendered to diagnostics/logs. (Ending the raw token's presence in *any*
  URL is the #166 ticket-resolver follow-up; the resolver seam makes that a
  drop-in.)
- **Protocol validation before connected (AC-3).** `Connected`/`Ready` is fed to
  the core only after a `hello` with matching `protocol` and `storage_generation`
  (§6.1 step 5). Health validation (§6.1 step 1) fails a protocol mismatch closed
  up front.
- **Restart / token-rotation recovery (AC-5).** Fresh health + session per
  attempt (§6.1); the core's generation fence and backoff, plus the reconciler,
  converge.
- **Bounded, honest post-send uncertainty.** Inherited from the kernel (#168):
  never-sent vs may-have-executed, `QueueFull` visible, deterministic settlement,
  generation fencing — `WsWeb` only supplies token/generation-tagged inputs.

---

## 8. Implementation steps (ordered)

1. **Codec client edge.** Add `encode_request` and
   `decode_client_frame`/`ClientInbound` to `jeliya-codec` with bounds and
   discrimination mirroring the server edge; capture `out` as raw text. Unit
   tests: round-trip encode/decode; reply-vs-push-vs-hello discrimination;
   `id > MAX_REQUEST_ID`, `t`+`id` together, neither → `Malformed`;
   oversized/over-deep → bounded refusal. Golden vectors against the daemon's
   real `hello`/`reply`/`push` bytes.
2. **Extend the `Driver` trait** (`transport.rs`) to the event-source/action-sink
   shape (§6.2) and relax its `Send` bound (keep native drivers `Send`). Adjust
   the in-memory `KernelController`/test-transport as needed to keep the existing
   kernel fault suite green (red-before-green for any behavioural change).
3. **Generic runtime** (`kernel/runtime.rs`): the `Send + Sync` backend half
   (eager dispatch, `IoAction` mailbox, `AtomicWaker`) reusing `Runtime`/
   `Deferred`/`EventBus`; the pump that owns a `Driver` and bridges
   `DriverEvent`↔`Input`↔`IoAction`. Native unit tests drive it via an
   in-memory `Driver` (the existing controller substrate) to prove parity with
   `KernelController` semantics (settlement, fencing, stop, backoff).
4. **`ws_web/session.rs`**: `Endpoint` resolution (`SameOrigin` via
   `window.location`; `Explicit`), the `fetch`-based health read + status
   validation, and the `GetTokenResolver` / `SessionResolver` seam, with
   `AbortController` for cancellation.
5. **`ws_web/timers.rs`**: `performance.now()` clock + `setTimeout`/`clearTimeout`
   `TimerService`; the hello deadline.
6. **`ws_web/socket.rs`**: the `web_sys::WebSocket` `Driver` impl — the dial
   sequence (§6.1), JS callbacks feeding backend inputs, `send`, generation
   tagging, and the close-code table.
7. **`ws_web/diag.rs`**: `RedactedUrl` + redaction test.
8. **`ws_web/mod.rs`**: `WsWebConfig` and the public constructor
   `pub fn connect_ws_web(config: WsWebConfig) -> ClientHandle` (behind
   `#[cfg(all(target_arch="wasm32", feature="ws-web"))]`), which builds the
   runtime, spawns the pump via `wasm_bindgen_futures::spawn_local`, and returns
   the handle. Re-export from `lib.rs` under the same cfg.
9. **Cargo/CI** (§4.2, §10): feature `ws-web`, wasm target deps, a wasm build
   check, and the real-browser test job.
10. **Docs:** update `docs/dioxus-architecture.md` (Decision 4 status:
    `WsWeb` landed; the `<= mock` note) and any adapter table; note the #166
    ticket-resolver and #270 replay follow-ups. No stale counts.

---

## 9. Test strategy

**Fake transport alone is insufficient (AC-6).** Two tiers:

### 9.1 Native, deterministic (fast, every PR)
- Codec client-edge unit tests (step 1).
- Generic-runtime tests via an in-memory `Driver` (step 3): the five kernel
  scenarios (connect/reply round-trip, disconnect classification, generation
  fencing, timeout/tombstone, clean stop) reproduced **through the runtime**, not
  just the sans-IO core — proving the runtime wires actions/events correctly.
- `RedactedUrl` redaction test.
- The `tests/boundaries.rs` host-tree scan still passes (no new native
  transport/UI edge).

### 9.2 Real headless browser against a real supervised daemon (AC-6, Verification)
A dedicated CI job (mirroring the `jeliya-ui-web` job's toolchain: Rust 1.96.0,
`wasm32-unknown-unknown`, pinned `wasm-bindgen-cli@0.2.126`, Chromium):
- A **native harness** builds `jeliyad` and starts a **supervised** instance via
  `jeliya-supervisor` (#170) on a loopback port, capturing `{port, token}` and
  its `storage_generation`.
- A `wasm-bindgen-test` suite (`#[wasm_bindgen_test]`, `wasm_bindgen_test_configure!(run_in_browser)`)
  built with `--features ws-web` runs in headless Chromium (chromedriver). It
  constructs `WsWeb` with an `Explicit` endpoint + an `ExplicitResolver`/real
  `GetTokenResolver` pointed at the harness daemon (loopback `Origin` is
  accepted; `/api/session` mirrors CORS for loopback origins) and asserts:
  1. **Initial connect:** `start()` → observes `Connecting → Ready` only after a
     valid `hello`; a `room.list` call resolves.
  2. **Push:** subscribe to a room, cause an event, observe a `ClientEvent::Push`
     on the fan-out.
  3. **Daemon restart:** the harness restarts the supervised daemon (new
     token/possibly new `sg`); observe `Ready → Interrupted → … → Ready` and a
     post-restart call succeeding (fresh credentials + fresh health healed it).
  4. **Protocol mismatch:** point the client at a status/`hello` advertising an
     unsupported generation (a harness stub or a version knob) → observe terminal
     `Failed`, no Ready, and **no token in any surfaced diagnostic**.
  5. **Stop:** `stop()` settles outstanding work (`Cancelled`), closes the event
     stream, and reaches `Stopped`; the socket is closed.

> Orchestration note (native start/stop around an in-browser test) is the harness
> concern. Options: a `wasm-bindgen-test` runner wrapped by a script that
> starts/stops the daemon, or a Playwright test (reusing `crates/jeliya-ui/e2e`)
> loading a small wasm harness page that reports results. Recommend the
> `wasm-bindgen-test` path (standard "real headless browser" for Rust wasm);
> record the exact runner as **OQ-4**.

### 9.3 CI wiring
- Add `cargo build -p jeliya-client --features ws-web --target wasm32-unknown-unknown`
  to prove **AC-1** (wasm build, no Iroh/native).
- Add the real-browser job (§9.2) as a **non-required** check first; promote to
  required only after confirming the branch-protection contract (per the
  `jeliya-ui-web` precedent — required check names are load-bearing).

---

## 10. Acceptance criteria (from the issue) → how satisfied

- [ ] **`wasm32-unknown-unknown` builds without Iroh/native deps.** §4.2 feature
  gate + target-cfg deps; CI wasm build (§9.3); host-tree boundary scan
  unaffected.
- [ ] **Every attempt obtains fresh session credentials.** §6.1 steps 1–2 run
  per `Action::Dial`.
- [ ] **Connected emitted only after protocol validation.** §6.1 step 5;
  `Input::Connected` gated on a validated `hello`.
- [ ] **Token never appears in URLs exposed to diagnostics/logs.** §6.6
  `RedactedUrl` + redaction rule/test; resolver seam keeps the raw token out of
  the URL entirely under the #166 follow-up.
- [ ] **Restart and push-gap paths converge through #169.** §6.9; real-browser
  restart + push scenarios (§9.2).
- [ ] **Real-browser tests pass; fake transport alone insufficient.** §9.2.

---

## 11. Risks & open questions

- **OQ-1 — `!Send` strategy.** The mailbox split (§5) is the recommended
  unsafe-free, seam-preserving resolution. Confirm before implementing; the
  alternative (`cfg`-relaxed seam `Send` bound) has a native blast radius.
- **OQ-2 — runtime/controller unification.** Whether the in-memory driver is
  re-expressed as a `Driver` so #175 diffs the *same* runtime, or the
  `KernelController` stays a separate reference. Prefer unification if it does
  not disturb the existing kernel suite.
- **OQ-3 — inbound Binary handling.** Drop as `Malformed` now vs. wire
  `jeliya-codec` stream decoding immediately (couples to #269 + browser file
  features). Recommend drop-safe now.
- **OQ-4 — real-browser test runner.** `wasm-bindgen-test` + daemon-start
  wrapper vs. Playwright + wasm harness page. Recommend `wasm-bindgen-test`.
- **Browser cannot read a failed-upgrade status/body.** A `426`/`401`/`503` all
  present as generic `onerror`; deterministic protocol-mismatch detection is
  therefore done up front against `/api/health` (§6.1 step 1). Verify this is
  reliable across Chromium/Firefox.
- **`stable_principal = false` means no auto-replay.** Correct and safe today;
  mutations rely on the reconciler's re-read. Enabling replay is a #270-gated
  follow-up — do not enable it here.
- **`wasm-bindgen` pin drift** would break the #176 reproducible-build CLI
  contract. Reuse the locked version exactly (§4.3).
- **Dev-mode cross-origin.** `Explicit` endpoints against a daemon on another
  loopback port are cross-origin; `/api/session` mirrors CORS only for loopback
  origins and the WS gate accepts a loopback `Origin`. Tests must use loopback
  origins.

## 12. Follow-ups (explicitly out of this slice)
- #166 — pairing-code / connect-ticket `SessionResolver` (`?ct=`), removing the
  raw token from the URL.
- #270 — daemon-incarnation fence in `hello`/`Input::Connected`, enabling
  `stable_principal = true` and cross-reconnect mutation replay + a stable
  per-tab `cid`.
- #172 / #173 — `WsNative` / `DirectClient` reuse the §6.2 runtime.
- #175 — the fault-injected four-adapter parity suite.
- Production UI cutover — flip `jeliya-ui::compose::web_composition` from the
  mock to `connect_ws_web`.
- Browser Binary byte-stream transport (file read/share media), riding #269.
