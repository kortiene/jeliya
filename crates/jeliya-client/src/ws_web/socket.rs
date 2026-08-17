//! The `web_sys::WebSocket` [`Driver`]: the dial sequence (§6.1), the JS
//! callbacks that feed the runtime, generation tagging, `send`, and the
//! close-code mapping (§6.7).
//!
//! Everything here is `!Send` (browser handles, JS `Closure`s), which is
//! exactly why it lives behind the runtime's `spawn_local`'d pump and never on
//! the `Send + Sync` backend half (§5). Callbacks only ever **push** events and
//! read the driver's own state; the closures are torn down only from the pump
//! (`dial`/`cancel_dial`), never from within a running callback.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use jeliya_api::RequestId;
use jeliya_codec::{decode_client_frame, encode_request, ClientInbound, CodecBounds};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::spawn_local;
use web_sys::{AbortController, BinaryType, CloseEvent, Event, MessageEvent, WebSocket};

use crate::backend::RawJson;
use crate::kernel::timing::{Tick, TimerId};
use crate::kernel::transport::{
    Driver, DriverEvent, Inbound, TransportClosed, WireFrame, WireReply,
};

use super::diag::RedactedUrl;
use super::session::{Endpoint, SessionResolver};
use super::{session, timers};

/// The one protocol generation this client speaks.
const CLIENT_PROTOCOL: u64 = jeliya_codec::PROTOCOL;

/// The immutable dial configuration shared with each spawned dial task.
struct DialConfig {
    endpoint: Endpoint,
    session: Rc<dyn SessionResolver>,
    hello_timeout_ms: u32,
    bounds: CodecBounds,
}

/// A live kernel timer: the browser handle plus the closure kept alive for it.
struct KernelTimer {
    handle: i32,
    _closure: Closure<dyn FnMut()>,
}

/// The keep-alive owner of one socket's JS callbacks. Dropping it detaches the
/// callbacks; done only from the pump, never from within a callback.
struct SocketClosures {
    _onmessage: Closure<dyn FnMut(MessageEvent)>,
    _onclose: Closure<dyn FnMut(CloseEvent)>,
    _onerror: Closure<dyn FnMut(Event)>,
    _hello: Closure<dyn FnMut()>,
}

/// The `!Send` driver state, shared between the driver, the JS callbacks, and
/// the spawned dial tasks via `Rc<RefCell<_>>`.
struct DriverState {
    /// Events queued for the pump's `poll_event`.
    events: VecDeque<DriverEvent>,
    /// The pump's task waker, stored on `Pending`.
    waker: Option<Waker>,
    /// The current socket (opening or live).
    socket: Option<WebSocket>,
    /// The current socket's callbacks, kept alive.
    socket_closures: Option<SocketClosures>,
    /// The current socket's per-connection context.
    socket_ctx: Option<Rc<SocketCtx>>,
    /// The last-assigned connection generation, kept in lockstep with the core
    /// (which bumps by one on every `Connected`), so inbound frames are tagged
    /// with their socket's generation-at-connect and a replaced socket's late
    /// frame is fenced.
    generation: u64,
    /// The in-progress dial's token, if any (cleared once connected/cancelled).
    active_dial_token: Option<u64>,
    /// The in-progress dial's fetch abort handle.
    dial_abort: Option<AbortController>,
    /// Armed kernel timers, keyed by [`TimerId`].
    timers: HashMap<TimerId, KernelTimer>,
}

impl DriverState {
    /// Queue an event and wake the pump (idempotent; the pump re-registers each
    /// poll).
    fn push(&mut self, event: DriverEvent) {
        self.events.push_back(event);
        if let Some(waker) = &self.waker {
            waker.wake_by_ref();
        }
    }

    /// Detach and drop the current socket, its callbacks, its context, and any
    /// in-flight dial fetch. Called only from the pump (`dial`/`cancel_dial`),
    /// so no callback is dropped while it is executing.
    fn teardown_socket(&mut self) {
        if let Some(ws) = self.socket.take() {
            ws.set_onopen(None);
            ws.set_onmessage(None);
            ws.set_onclose(None);
            ws.set_onerror(None);
            let _ = ws.close();
        }
        if let Some(ctx) = self.socket_ctx.take() {
            if let Some(handle) = ctx.hello_timer.take() {
                timers::clear_timeout(handle);
            }
        }
        self.socket_closures = None;
        if let Some(abort) = self.dial_abort.take() {
            abort.abort();
        }
    }
}

/// One socket's per-connection context, shared by its callbacks.
struct SocketCtx {
    state: Rc<RefCell<DriverState>>,
    token: u64,
    /// The storage generation presented at the gate; the `hello` must match it.
    expected_sg: u64,
    bounds: CodecBounds,
    /// Set once a valid `hello` is seen; flips the message handler to steady
    /// state and disarms the pre-connect failure paths.
    validated: Cell<bool>,
    /// This socket's generation-at-connect, assigned at validation.
    socket_gen: Cell<u64>,
    /// Dedups `onerror`/`onclose` so a loss is signalled at most once.
    signalled_loss: Cell<bool>,
    /// The hello-deadline browser timer handle.
    hello_timer: Cell<Option<i32>>,
}

/// The browser WebSocket/session driver (#171).
pub(crate) struct WsWebDriver {
    state: Rc<RefCell<DriverState>>,
    config: Rc<DialConfig>,
}

impl WsWebDriver {
    /// Build a driver over `endpoint`, obtaining credentials via `session` and
    /// bounding the wait for a valid `hello` by `hello_timeout_ms`.
    pub(crate) fn new(
        endpoint: Endpoint,
        session: Rc<dyn SessionResolver>,
        hello_timeout_ms: u32,
    ) -> Self {
        let config = Rc::new(DialConfig {
            endpoint,
            session,
            hello_timeout_ms,
            bounds: CodecBounds::default(),
        });
        let state = Rc::new(RefCell::new(DriverState {
            events: VecDeque::new(),
            waker: None,
            socket: None,
            socket_closures: None,
            socket_ctx: None,
            generation: 0,
            active_dial_token: None,
            dial_abort: None,
            timers: HashMap::new(),
        }));
        Self { state, config }
    }
}

impl Driver for WsWebDriver {
    fn poll_event(&mut self, cx: &mut Context<'_>) -> Poll<DriverEvent> {
        let mut state = self.state.borrow_mut();
        match state.events.pop_front() {
            Some(event) => Poll::Ready(event),
            None => {
                state.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }

    fn send(&mut self, frame: WireFrame) -> Result<(), TransportClosed> {
        let bytes = encode_request(
            frame.id.get(),
            frame.op,
            frame.op_id.as_ref(),
            frame.input.as_str(),
            &self.config.bounds,
        )
        .map_err(|_| TransportClosed)?;
        let text = String::from_utf8(bytes).map_err(|_| TransportClosed)?;
        let state = self.state.borrow();
        match &state.socket {
            Some(ws) => ws.send_with_str(&text).map_err(|_| TransportClosed),
            None => Err(TransportClosed),
        }
    }

    fn dial(&mut self, token: u64) {
        {
            let mut state = self.state.borrow_mut();
            state.teardown_socket();
            state.active_dial_token = Some(token);
        }
        spawn_local(run_dial(self.state.clone(), self.config.clone(), token));
    }

    fn cancel_dial(&mut self) {
        let mut state = self.state.borrow_mut();
        state.active_dial_token = None;
        state.teardown_socket();
    }

    fn arm_timer(&mut self, id: TimerId, at: Tick) {
        let now = self.now();
        let delay_ms = at.0.saturating_sub(now.0).min(i32::MAX as u64) as i32;
        let state = self.state.clone();
        let closure = Closure::wrap(Box::new(move || {
            state.borrow_mut().push(DriverEvent::TimerFired(id));
            // Defer removing this fired one-shot so its own closure is not
            // dropped mid-execution.
            let state = state.clone();
            spawn_local(async move {
                state.borrow_mut().timers.remove(&id);
            });
        }) as Box<dyn FnMut()>);
        if let Some(handle) = timers::set_timeout(delay_ms, &closure) {
            self.state.borrow_mut().timers.insert(
                id,
                KernelTimer {
                    handle,
                    _closure: closure,
                },
            );
        }
    }

    fn cancel_timer(&mut self, id: TimerId) {
        if let Some(timer) = self.state.borrow_mut().timers.remove(&id) {
            timers::clear_timeout(timer.handle);
        }
    }

    fn now(&self) -> Tick {
        timers::now_tick()
    }
}

/// One dial attempt: fresh health, fresh credential, then open+validate the
/// socket (§6.1 steps 1–5). Fresh status/credentials every attempt is what
/// heals a restart or token rotation (AC-2, AC-5).
async fn run_dial(state: Rc<RefCell<DriverState>>, config: Rc<DialConfig>, token: u64) {
    let (http_base, ws_base) = match config.endpoint.resolve() {
        Ok(bases) => bases,
        Err(_) => return fail_dial(&state, token),
    };

    let abort = AbortController::new().ok();
    let signal = abort.as_ref().map(|a| a.signal());
    if let Some(abort) = &abort {
        state.borrow_mut().dial_abort = Some(abort.clone());
    }

    // 1. Daemon-status validation.
    let health = match session::fetch_health(&http_base, signal.as_ref()).await {
        Ok(health) => health,
        Err(_) => return fail_dial(&state, token),
    };
    if !dial_still_current(&state, token) {
        return;
    }
    // Protocol mismatch fails closed up front — the browser cannot read the
    // gate's 426 body, so this is where it is caught deterministically.
    if !(health.min_protocol <= CLIENT_PROTOCOL && CLIENT_PROTOCOL <= health.protocol) {
        return gate_refuse(&state, token);
    }
    let sg = health.storage_generation;

    // 2. Fresh credential (never logged).
    let (param, credential) = match config.session.resolve(&http_base).await {
        Ok(credential) => credential,
        Err(_) => return fail_dial(&state, token),
    };
    if !dial_still_current(&state, token) {
        return;
    }

    // 3. Open the socket (unvalidated until a valid `hello` arrives).
    let url = build_ws_url(&ws_base, sg, param, &credential);
    diag_connecting(&url);
    let ws = match WebSocket::new(&url) {
        Ok(ws) => ws,
        Err(_) => return fail_dial(&state, token),
    };
    ws.set_binary_type(BinaryType::Arraybuffer);
    wire_socket(&state, &config, ws, token, sg);
}

/// Whether `token` is still the driver's outstanding dial (not superseded or
/// cancelled between awaits).
fn dial_still_current(state: &Rc<RefCell<DriverState>>, token: u64) -> bool {
    state.borrow().active_dial_token == Some(token)
}

/// Report a recoverable dial failure, token-fenced.
fn fail_dial(state: &Rc<RefCell<DriverState>>, token: u64) {
    let mut state = state.borrow_mut();
    if state.active_dial_token == Some(token) {
        state.active_dial_token = None;
        state.dial_abort = None;
        state.push(DriverEvent::DialFailed { token });
    }
}

/// Report a terminal gate/protocol refusal, token-fenced.
fn gate_refuse(state: &Rc<RefCell<DriverState>>, token: u64) {
    let mut state = state.borrow_mut();
    if state.active_dial_token == Some(token) {
        state.active_dial_token = None;
        state.dial_abort = None;
        state.push(DriverEvent::GateRefused { token });
    }
}

/// Build `{ws_base}?v=2&sg=<sg>&<param>=<credential>` with the credential
/// URL-encoded. `cid` is omitted (§6.8). The raw URL is never retained in a
/// renderable form; diagnostics use [`RedactedUrl`].
fn build_ws_url(ws_base: &str, sg: u64, param: &str, credential: &str) -> String {
    let encoded = String::from(js_sys::encode_uri_component(credential));
    format!("{ws_base}?v=2&sg={sg}&{param}={encoded}")
}

/// A browser-safe diagnostic naming only the redacted endpoint.
fn diag_connecting(url: &str) {
    web_sys::console::debug_1(&JsValue::from_str(&format!(
        "ws_web: connecting to {}",
        RedactedUrl::new(url)
    )));
}

/// Attach the dial-phase and steady-state callbacks and arm the hello deadline.
fn wire_socket(
    state: &Rc<RefCell<DriverState>>,
    config: &Rc<DialConfig>,
    ws: WebSocket,
    token: u64,
    sg: u64,
) {
    // Only wire if this dial is still current (no await since the last check,
    // but this keeps the invariant explicit and closes the socket otherwise).
    if !dial_still_current(state, token) {
        let _ = ws.close();
        return;
    }

    let ctx = Rc::new(SocketCtx {
        state: state.clone(),
        token,
        expected_sg: sg,
        bounds: config.bounds,
        validated: Cell::new(false),
        socket_gen: Cell::new(0),
        signalled_loss: Cell::new(false),
        hello_timer: Cell::new(None),
    });

    let onmessage = {
        let ctx = ctx.clone();
        Closure::wrap(
            Box::new(move |ev: MessageEvent| on_message(&ctx, ev)) as Box<dyn FnMut(MessageEvent)>
        )
    };
    let onclose = {
        let ctx = ctx.clone();
        Closure::wrap(
            Box::new(move |ev: CloseEvent| handle_loss(&ctx, Some(ev.code())))
                as Box<dyn FnMut(CloseEvent)>,
        )
    };
    let onerror = {
        let ctx = ctx.clone();
        Closure::wrap(Box::new(move |_ev: Event| handle_loss(&ctx, None)) as Box<dyn FnMut(Event)>)
    };
    let hello = {
        let ctx = ctx.clone();
        Closure::wrap(Box::new(move || on_hello_deadline(&ctx)) as Box<dyn FnMut()>)
    };

    ws.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
    ws.set_onclose(Some(onclose.as_ref().unchecked_ref()));
    ws.set_onerror(Some(onerror.as_ref().unchecked_ref()));

    // A browser cannot read a failed-upgrade status, so bound the whole
    // "no valid hello" window with a driver-local deadline.
    let delay = config.hello_timeout_ms.min(i32::MAX as u32) as i32;
    ctx.hello_timer.set(timers::set_timeout(delay, &hello));

    let mut state = state.borrow_mut();
    state.socket = Some(ws);
    state.socket_closures = Some(SocketClosures {
        _onmessage: onmessage,
        _onclose: onclose,
        _onerror: onerror,
        _hello: hello,
    });
    state.socket_ctx = Some(ctx);
}

/// Route a message to the validation path or the steady-state path.
fn on_message(ctx: &Rc<SocketCtx>, ev: MessageEvent) {
    if ctx.validated.get() {
        on_steady_message(ctx, ev);
    } else {
        on_first_message(ctx, ev);
    }
}

/// The first frame must be a valid `hello` before "Connected" is emitted
/// (AC-3, §6.1 step 5).
fn on_first_message(ctx: &Rc<SocketCtx>, ev: MessageEvent) {
    let Some(text) = ev.data().as_string() else {
        // A non-text first frame is not a v2 hello.
        return protocol_fail(ctx);
    };
    match decode_client_frame(text.as_bytes(), &ctx.bounds) {
        ClientInbound::Hello(hello) => {
            if hello.protocol != CLIENT_PROTOCOL {
                return protocol_fail(ctx);
            }
            if hello.storage_generation != ctx.expected_sg {
                // A reset raced the health read: recoverable, re-read next time.
                cancel_hello_deadline(ctx);
                let mut state = ctx.state.borrow_mut();
                if state.active_dial_token == Some(ctx.token) {
                    state.active_dial_token = None;
                    state.dial_abort = None;
                    state.push(DriverEvent::DialFailed { token: ctx.token });
                }
                return;
            }
            // Validated: only now is the connection reported to the core.
            cancel_hello_deadline(ctx);
            ctx.validated.set(true);
            let mut state = ctx.state.borrow_mut();
            if state.active_dial_token != Some(ctx.token) {
                return;
            }
            let generation = state.generation.saturating_add(1);
            state.generation = generation;
            ctx.socket_gen.set(generation);
            state.active_dial_token = None;
            state.dial_abort = None;
            state.push(DriverEvent::Connected { token: ctx.token });
        }
        // A reply, push, or malformed first frame means this is not a
        // hello-first v2 daemon: fail closed rather than pretend Ready.
        ClientInbound::Reply { .. } | ClientInbound::Push(_) | ClientInbound::Malformed => {
            protocol_fail(ctx)
        }
    }
}

/// Steady-state frames, tagged with this socket's generation-at-connect.
fn on_steady_message(ctx: &Rc<SocketCtx>, ev: MessageEvent) {
    let generation = ctx.socket_gen.get();
    let inbound = match ev.data().as_string() {
        Some(text) => match decode_client_frame(text.as_bytes(), &ctx.bounds) {
            ClientInbound::Reply { id, result } => match RequestId::new(id) {
                Ok(request_id) => {
                    let reply = match result {
                        Ok(raw) => WireReply::Ok(RawJson::from_string(raw.get().to_string())),
                        Err(api) => WireReply::Err(api),
                    };
                    Inbound::Reply {
                        generation,
                        id: request_id,
                        result: reply,
                    }
                }
                Err(_) => Inbound::Malformed,
            },
            ClientInbound::Push(push) => Inbound::Push { generation, push },
            // A second hello, or anything unparseable, strands nothing.
            ClientInbound::Hello(_) | ClientInbound::Malformed => Inbound::Malformed,
        },
        // A Binary frame (byte-stream media) is out of #171's scope; drop it
        // safely — the core discards a Malformed inbound and never panics.
        None => Inbound::Malformed,
    };
    ctx.state.borrow_mut().push(DriverEvent::Inbound(inbound));
}

/// A first frame that is not a well-formed `hello`: terminal, token-fenced.
fn protocol_fail(ctx: &Rc<SocketCtx>) {
    cancel_hello_deadline(ctx);
    let mut state = ctx.state.borrow_mut();
    if state.active_dial_token == Some(ctx.token) {
        state.active_dial_token = None;
        state.dial_abort = None;
        state.push(DriverEvent::GateRefused { token: ctx.token });
    }
}

/// `onerror`/`onclose` handling. A browser cannot expose the gate's HTTP
/// status, so a pre-validation loss is a recoverable `DialFailed` (except a
/// clean `4001 protocol_unsupported`, which is terminal). A post-validation
/// loss is an `Interrupted`; the reconnect's fresh health read re-validates the
/// protocol, so no post-open code needs to fail closed here.
fn handle_loss(ctx: &Rc<SocketCtx>, code: Option<u16>) {
    if ctx.signalled_loss.replace(true) {
        return;
    }
    cancel_hello_deadline(ctx);
    let mut state = ctx.state.borrow_mut();
    if ctx.validated.get() {
        let generation = ctx.socket_gen.get();
        state.push(DriverEvent::Interrupted { generation });
    } else {
        if state.active_dial_token != Some(ctx.token) {
            return;
        }
        state.active_dial_token = None;
        state.dial_abort = None;
        let event = match code {
            Some(4001) => DriverEvent::GateRefused { token: ctx.token },
            _ => DriverEvent::DialFailed { token: ctx.token },
        };
        state.push(event);
    }
}

/// The hello deadline fired before a valid `hello`: recoverable.
fn on_hello_deadline(ctx: &Rc<SocketCtx>) {
    ctx.hello_timer.set(None);
    if ctx.validated.get() {
        return;
    }
    let mut state = ctx.state.borrow_mut();
    if state.active_dial_token == Some(ctx.token) {
        state.active_dial_token = None;
        state.dial_abort = None;
        state.push(DriverEvent::DialFailed { token: ctx.token });
    }
}

/// Disarm the hello deadline (its closure stays alive in `SocketClosures`).
fn cancel_hello_deadline(ctx: &Rc<SocketCtx>) {
    if let Some(handle) = ctx.hello_timer.take() {
        timers::clear_timeout(handle);
    }
}
