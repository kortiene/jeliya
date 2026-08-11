//! The deterministic in-process mock backend (#167 §D10), the reference
//! behaviour the real adapters (#168+) are measured against.
//!
//! Determinism means **no wall clock, no timers, no RNG, and no reliance on
//! task-scheduling order**: given the same script and the same sequence of
//! caller actions, the observable sequence of replies and events is identical
//! on wasm and native. This is achieved with a **controller-driven** model —
//! scripted calls stay pending until the test advances the mock via
//! [`MockController::deliver_next`], so ordering (including push-before-response)
//! is explicit rather than a race.
//!
//! The mock makes every AC-5 scenario expressible: responses
//! ([`Program::reply_ok`]), errors ([`Program::reply_err`]),
//! push-before-response ([`Program::emit_then_reply`]), gaps
//! ([`MockController::emit`] of a [`ClientEvent::Gap`], or a scripted
//! `stream.resync` returning [`jeliya_api::ApiError::ResyncRequired`]),
//! cancellation ([`Program::hang`] settled by [`crate::ClientHandle::stop`] or a
//! `call_stream` cancel), and shutdown (stop settles every pending call, emits
//! `StateChanged` to `Stopping` then `Stopped`, and ends every subscription).

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use futures::channel::oneshot;
use futures::future::BoxFuture;
use jeliya_api::{ApiError, Operation};

use crate::backend::{ClientBackend, ErasedCall, RawJson, RawOut};
use crate::error::{CallError, Execution, LocalError};
use crate::event::{ClientEvent, EventBus, EventSubscription, State};
use crate::handle::ClientHandle;

/// The programmed outcome of one scripted call.
///
/// Construct via the associated functions; the internal representation is kept
/// private so the erased `RawJson` output boundary never leaks into the public
/// API.
pub struct Program {
    step: Step,
}

/// The private, erased representation of a [`Program`].
enum Step {
    /// Resolve the call with a typed success output, serialized like the wire.
    ReplyOk(RawOut),
    /// Resolve the call with a wire error → [`CallError::Wire`].
    ReplyErr(ApiError),
    /// Emit these events to all subscribers, then resolve with the reply.
    EmitThenReply {
        /// Events broadcast before the reply resolves.
        before: Vec<ClientEvent>,
        /// The reply delivered after the events.
        reply: Box<Step>,
    },
    /// Never resolve on its own; only cancellation or `stop()` settles it. The
    /// `sent` flag fixes the [`Execution`] the resulting `Cancelled` /
    /// `Disconnected` error must carry.
    Hang {
        /// Whether bytes are modeled as having gone out.
        sent: bool,
    },
    /// Resolve with a client-side classification (`QueueFull`, `Timeout`, …).
    Local(CallError),
}

impl Step {
    /// If this step ultimately hangs — a bare [`Step::Hang`] or an
    /// [`Step::EmitThenReply`] chain ending in one — the `sent` flag of that
    /// hang. `None` for every step with a real terminal.
    fn hang_sent(&self) -> Option<bool> {
        match self {
            Step::Hang { sent } => Some(*sent),
            Step::EmitThenReply { reply, .. } => reply.hang_sent(),
            _ => None,
        }
    }
}

impl Program {
    /// Resolve the call with a typed success output. `O` names the operation
    /// whose paired `Output` this is, so the scripted reply is serialized
    /// exactly as the wire would carry it and the handle decodes it back into
    /// `O::Output`.
    pub fn reply_ok<O: Operation>(output: &O::Output) -> Self {
        let text = serde_json::to_string(output).expect("mock success output serializes");
        Program {
            step: Step::ReplyOk(RawJson::from_string(text)),
        }
    }

    /// Resolve the call with a wire error, delivered as [`CallError::Wire`].
    pub fn reply_err(error: ApiError) -> Self {
        Program {
            step: Step::ReplyErr(error),
        }
    }

    /// Emit `before` to every subscriber, then resolve with `reply`. Proves
    /// push-before-response ordering deterministically: a subscriber observes
    /// the pushes strictly before the caller's future resolves.
    pub fn emit_then_reply(before: Vec<ClientEvent>, reply: Program) -> Self {
        Program {
            step: Step::EmitThenReply {
                before,
                reply: Box::new(reply.step),
            },
        }
    }

    /// Never resolve on its own. `sent` fixes the [`Execution`] a later
    /// cancellation or `stop()` must carry: `false` ⇒ `DefinitelyNot` (queued,
    /// unsent), `true` ⇒ `Unknown` (bytes went out).
    pub fn hang(sent: bool) -> Self {
        Program {
            step: Step::Hang { sent },
        }
    }

    /// Resolve with a client-side [`CallError`] classification — a scripted
    /// `QueueFull`, `Timeout`, or other non-wire outcome. Use this to exercise
    /// that such classes never masquerade as a [`CallError::Wire`].
    pub fn local(error: CallError) -> Self {
        Program {
            step: Step::Local(error),
        }
    }
}

/// The scripted mock builder.
///
/// Program per-operation outcomes with [`on`](Self::on) (matched by wire name
/// and occurrence order), then [`build`](Self::build) the handle plus the
/// controller the test drives.
#[derive(Default)]
pub struct MockScript {
    programs: HashMap<&'static str, VecDeque<Step>>,
}

impl MockScript {
    /// An empty script.
    pub fn new() -> Self {
        Self::default()
    }

    /// Program the outcome of the next call to `op` (matched by wire name and
    /// occurrence order). Repeated calls to the same `op` queue in order.
    pub fn on(mut self, op: &'static str, program: Program) -> Self {
        self.programs.entry(op).or_default().push_back(program.step);
        self
    }

    /// Build the backend (behind a [`ClientHandle`]) plus a [`MockController`]
    /// the test drives. The handle and controller share one backend.
    pub fn build(self) -> (ClientHandle, MockController) {
        let inner = Arc::new(Mutex::new(MockInner {
            state: State::Idle,
            programs: self.programs,
            pending: VecDeque::new(),
            pending_wakers: Vec::new(),
            stopping: false,
            bus: Arc::new(EventBus::new()),
        }));
        let handle = ClientHandle::from_backend(Arc::new(MockBackend {
            inner: Arc::clone(&inner),
        }));
        (handle, MockController { inner })
    }
}

/// A call that has reached the mock and is awaiting resolution.
struct Pending {
    sender: oneshot::Sender<Result<RawJson, CallError>>,
    step: Step,
}

/// The shared mock state, guarded by one mutex. Lock ordering is always
/// `inner → bus → subscriber`; nothing acquires them in reverse, so the fan-out
/// broadcasts and the subscription polls never deadlock.
struct MockInner {
    state: State,
    programs: HashMap<&'static str, VecDeque<Step>>,
    pending: VecDeque<Pending>,
    /// Wakers parked by [`MockController::pending_call`], woken when a call
    /// registers (or the mock begins stopping, so a waiter never outlives
    /// the backend it waits on).
    pending_wakers: Vec<Waker>,
    stopping: bool,
    bus: Arc<EventBus>,
}

impl MockInner {
    /// Move to `to`, broadcasting the transition as a
    /// [`ClientEvent::StateChanged`].
    fn transition(&mut self, to: State) {
        let from = self.state;
        self.state = to;
        self.bus.broadcast(ClientEvent::StateChanged { from, to });
    }

    /// Fully resolve a deliverable step: broadcast any `before` events, then
    /// send the terminal reply. `Hang` is never passed here — its terminal is
    /// decided by cancellation / stop / disconnect.
    fn resolve(&mut self, sender: oneshot::Sender<Result<RawJson, CallError>>, step: Step) {
        match step {
            Step::ReplyOk(raw) => {
                let _ = sender.send(Ok(raw));
            }
            Step::ReplyErr(error) => {
                let _ = sender.send(Err(CallError::Wire(error)));
            }
            Step::Local(error) => {
                let _ = sender.send(Err(error));
            }
            Step::EmitThenReply { before, reply } => {
                for event in before {
                    self.bus.broadcast(event);
                }
                self.resolve(sender, *reply);
            }
            Step::Hang { .. } => {
                debug_assert!(false, "resolve() must not be called on a Hang step");
            }
        }
    }

    /// Broadcast every `before` event along an [`Step::EmitThenReply`] chain,
    /// discarding the chain's terminal. Used when the chain ends in a hang
    /// that must stay pending after its events are delivered.
    fn emit_chain_events(&mut self, step: Step) {
        if let Step::EmitThenReply { before, reply } = step {
            for event in before {
                self.bus.broadcast(event);
            }
            self.emit_chain_events(*reply);
        }
    }

    /// Deliver the next pending scripted step: the oldest pending call that is
    /// not a bare hang. An `EmitThenReply` chain ending in a hang delivers its
    /// events and then stays pending as that hang (settled later by stop,
    /// cancellation, or disconnect). Returns `false` if nothing is deliverable.
    fn deliver_next(&mut self) -> bool {
        // Purge abandoned work first: a cancelled call's reply channel is
        // closed, and its scripted step must never be consumed as if it were
        // the next observable delivery.
        self.pending.retain(|pending| !pending.sender.is_canceled());
        let index = self
            .pending
            .iter()
            .position(|pending| !matches!(pending.step, Step::Hang { .. }));
        match index {
            Some(index) => match self.pending[index].step.hang_sent() {
                Some(sent) => {
                    // The chain ends in a hang: deliver its events, leave the
                    // hang pending in place.
                    let step =
                        std::mem::replace(&mut self.pending[index].step, Step::Hang { sent });
                    self.emit_chain_events(step);
                    true
                }
                None => {
                    let pending = self.pending.remove(index).expect("index is in range");
                    self.resolve(pending.sender, pending.step);
                    true
                }
            },
            None => false,
        }
    }

    /// Settle every pending call as a connection loss: sent-and-pending as
    /// `Disconnected { Unknown }`, unsent as `Disconnected { DefinitelyNot }`.
    /// A non-hanging pending call is treated as sent (it reached the backend).
    fn drop_connection(&mut self) {
        for pending in std::mem::take(&mut self.pending) {
            let sent = pending.step.hang_sent().unwrap_or(true);
            let execution = if sent {
                Execution::Unknown
            } else {
                Execution::DefinitelyNot
            };
            let _ = pending
                .sender
                .send(Err(CallError::Disconnected { execution }));
        }
    }

    /// The `stop()` ordering (§D6): refuse new calls, settle all accepted work
    /// preserving may-have-executed, close all event streams, then land in
    /// `Stopped`.
    fn begin_stop(&mut self) {
        if self.stopping {
            return;
        }
        self.stopping = true;
        // 1. Stopping — new calls are refused from here (see `dispatch`).
        self.transition(State::Stopping);
        // 2. Settle all accepted work: a real terminal if one is available,
        //    else a Cancelled whose Execution preserves may-have-executed.
        for pending in std::mem::take(&mut self.pending) {
            match pending.step.hang_sent() {
                Some(sent) => {
                    let execution = if sent {
                        Execution::Unknown
                    } else {
                        Execution::DefinitelyNot
                    };
                    let _ = pending.sender.send(Err(CallError::Cancelled { execution }));
                }
                None => self.resolve(pending.sender, pending.step),
            }
        }
        // 3 & 4. Final Stopped transition, then close every subscription so it
        //        yields None after the Stopped event it just observed.
        self.transition(State::Stopped);
        self.bus.close();
        // A parked pending_call() waiter must observe the shutdown rather
        // than sleep past the backend's end.
        for waker in std::mem::take(&mut self.pending_wakers) {
            waker.wake();
        }
    }
}

/// The erased mock backend the handle wraps.
struct MockBackend {
    inner: Arc<Mutex<MockInner>>,
}

impl ClientBackend for MockBackend {
    fn dispatch(&self, call: ErasedCall) -> BoxFuture<'static, Result<RawJson, CallError>> {
        let mut inner = self.inner.lock().expect("mock poisoned");
        // A call begun after stop() never leaves the seam.
        if inner.stopping || matches!(inner.state, State::Stopping | State::Stopped) {
            return Box::pin(async {
                Err(CallError::Cancelled {
                    execution: Execution::DefinitelyNot,
                })
            });
        }
        let step = inner
            .programs
            .get_mut(call.op)
            .and_then(VecDeque::pop_front);
        match step {
            // An unscripted operation is a test-setup error, surfaced
            // deterministically rather than hanging forever.
            None => Box::pin(async { Err(CallError::Local(LocalError::Backend)) }),
            Some(step) => {
                let (sender, receiver) = oneshot::channel();
                inner.pending.push_back(Pending { sender, step });
                // The registration IS the wake source for pending_call():
                // a driver awaiting dispatch resumes because dispatch
                // happened, never because a scheduler guess paid off.
                for waker in inner.pending_wakers.drain(..) {
                    waker.wake();
                }
                Box::pin(async move {
                    match receiver.await {
                        Ok(result) => result,
                        // The controller/backend was dropped without settling.
                        Err(_canceled) => Err(CallError::Local(LocalError::Backend)),
                    }
                })
            }
        }
    }

    fn subscribe(&self) -> EventSubscription {
        self.inner.lock().expect("mock poisoned").bus.subscribe()
    }

    fn state(&self) -> State {
        self.inner.lock().expect("mock poisoned").state
    }

    fn start(&self) {
        let mut inner = self.inner.lock().expect("mock poisoned");
        if inner.state == State::Idle {
            inner.transition(State::Connecting);
        }
    }

    fn stop(&self) -> BoxFuture<'static, ()> {
        self.inner.lock().expect("mock poisoned").begin_stop();
        // The mock settles synchronously, so the stop future is already ready:
        // by the time this returns, every accepted call's terminal has been
        // delivered and every subscription has been closed.
        Box::pin(async {})
    }
}

/// The test-driven controller half of the mock. Every method is deterministic
/// and clock-free; nothing here observes real time or scheduling order.
pub struct MockController {
    inner: Arc<Mutex<MockInner>>,
}

impl MockController {
    /// Deliver the next pending scripted step (a reply and/or its `before`
    /// events). Returns `true` if a call was resolved, `false` if none was
    /// deliverable (all pending calls are hanging, or none are pending).
    pub fn deliver_next(&self) -> bool {
        self.inner.lock().expect("mock poisoned").deliver_next()
    }

    /// Resolve once at least one dispatched call is pending settlement (or
    /// the mock has begun stopping, so a waiter never outlives the backend).
    ///
    /// This is the event-driven complement to [`Self::deliver_next`] for
    /// drivers that share an executor with the code under test: a
    /// deliver-and-yield pump must GUESS how many passes the dispatching
    /// task needs, and a self-waking pump can starve it indefinitely on an
    /// executor that isn't round-robin fair. Awaiting this future instead
    /// resumes the driver exactly when `dispatch` registers a call — woken
    /// by the dispatch itself, deterministically and clock-free.
    pub fn pending_call(&self) -> PendingCall {
        PendingCall {
            inner: Arc::clone(&self.inner),
        }
    }

    /// Inject an out-of-band event (a [`ClientEvent::Gap`],
    /// [`ClientEvent::ResyncRequired`], a peer push, a [`ClientEvent::Lagged`],
    /// …) to every subscriber.
    pub fn emit(&self, event: ClientEvent) {
        self.inner
            .lock()
            .expect("mock poisoned")
            .bus
            .broadcast(event);
    }

    /// Drive a lifecycle transition the script asserts (for example
    /// `Interrupted` or `Failed`), broadcasting the
    /// [`ClientEvent::StateChanged`].
    ///
    /// `Stopping` and `Stopped` cannot be scripted here: those states carry the
    /// §D6 guarantee that accepted work has settled and subscriptions have
    /// closed, which only [`crate::ClientHandle::stop`] performs. Scripting
    /// them would broadcast the transition while leaving calls pending and
    /// streams open — a state the contract says cannot exist.
    ///
    /// # Panics
    ///
    /// Panics if `state` is [`State::Stopping`] or [`State::Stopped`].
    pub fn set_state(&self, state: State) {
        assert!(
            !matches!(state, State::Stopping | State::Stopped),
            "set_state cannot script {state:?}: shutdown must go through ClientHandle::stop(), \
             which settles pending calls and closes subscriptions as the state guarantees"
        );
        self.inner.lock().expect("mock poisoned").transition(state);
    }

    /// Force a connection loss: settle all sent-and-pending calls as
    /// `Disconnected { Unknown }` and unsent ones as
    /// `Disconnected { DefinitelyNot }`. Lifecycle state is left to the test to
    /// drive with [`set_state`](Self::set_state), so an asserted `StateChanged`
    /// sequence stays under the script's control.
    pub fn drop_connection(&self) {
        self.inner.lock().expect("mock poisoned").drop_connection();
    }
}

/// Future returned by [`MockController::pending_call`]: ready when a
/// dispatched call awaits settlement, or when the mock has begun stopping.
pub struct PendingCall {
    inner: Arc<Mutex<MockInner>>,
}

impl Future for PendingCall {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let mut inner = self.inner.lock().expect("mock poisoned");
        if !inner.pending.is_empty() || inner.stopping {
            return Poll::Ready(());
        }
        inner.pending_wakers.push(cx.waker().clone());
        Poll::Pending
    }
}
