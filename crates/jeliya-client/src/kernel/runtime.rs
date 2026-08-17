//! The generic async runtime that binds any [`Driver`] to the sans-IO
//! [`Core`] (#171, §6.2).
//!
//! The kernel defined the seam and shipped only the deterministic in-memory
//! controller (`kernel/mod.rs`, feature `test-transport`). This module lands
//! the **real** runtime loop the seam deferred: it is transport-agnostic and is
//! reused verbatim by `WsNative` (#172) and `DirectClient` (#173).
//!
//! **The mailbox split (§5).** `ClientBackend: Send + Sync`, but on
//! `wasm32-unknown-unknown` a [`Driver`] holds `!Send` browser handles and the
//! crate is `#![forbid(unsafe_code)]`, so the driver cannot live behind the
//! `Send` seam. The runtime therefore has two halves:
//!
//! - The **`Send + Sync` backend** ([`RuntimeBackend`]) holds only `Send`
//!   state — the [`Core`], the reply senders, the [`EventBus`], the
//!   re-entrancy-safe deferred-wake delivery queue (reused from `kernel/mod.rs`
//!   via [`Deferred`]/[`DeferredWake`]), a `Send` outbound **IO-action
//!   mailbox**, and a `Send` [`AtomicWaker`]. `dispatch`/`start`/`stop`/`Cancel`
//!   step the core synchronously (dispatch stays eager); every action that
//!   needs the transport is enqueued into the mailbox and the pump is woken.
//! - The **`!Send`-capable pump** ([`Pump`], `spawn_local`'d by the platform)
//!   owns the [`Driver`]. It drains the mailbox (performing the real IO),
//!   surfaces the driver's events back into the core as [`Input`]s, and reads
//!   the platform clock via `driver.now()`.
//!
//! All correctness still lives in the pure core; this module only wires actions
//! out and events in, so its behaviour is identical on wasm and native.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll};

use futures::channel::oneshot;
use futures::future::BoxFuture;
use futures::task::AtomicWaker;

use crate::backend::{ClientBackend, ErasedCall, RawJson};
use crate::error::{CallError, LocalError};
use crate::event::{EventBus, EventSubscription, State};
use crate::handle::ClientHandle;
use crate::kernel::core::{Action, Core, Input};
use crate::kernel::inflight::CallId;
use crate::kernel::timing::{Tick, TimerId};
use crate::kernel::transport::{Driver, DriverEvent, WireFrame};
use crate::KernelConfig;

use super::{Deferred, DeferredWake};

/// One transport-bound action the core emitted, queued for the pump to perform.
/// Mirrors the browser-bound `Action` arms exactly; the settle/emit/close arms
/// go through the [`Deferred`] wake path instead, unchanged.
pub(crate) enum IoAction {
    /// Push one encoded frame onto the transport.
    Send(WireFrame),
    /// Begin one dial identified by `token`.
    Dial {
        /// The attempt's identity, echoed on every outcome.
        token: u64,
    },
    /// Cancel any in-progress dial/backoff.
    CancelDial,
    /// Arm a driver timer to fire at `at`.
    ArmTimer {
        /// The timer's identity.
        id: TimerId,
        /// When it should fire (logical time).
        at: Tick,
    },
    /// Cancel a previously-armed driver timer.
    CancelTimer(TimerId),
}

/// A `Send + Sync` reader of the platform's monotonic clock. On wasm it reads
/// `performance.now()` transiently (the `!Send` browser handle never escapes
/// the call), so it is safe to hold on the `Send` backend; native drivers pass
/// their own. Read **fresh on every core step** — never cached — so a call
/// dispatched after an idle gap gets a deadline measured from real time, not a
/// frozen one.
pub(crate) type RuntimeClock = Box<dyn Fn() -> Tick + Send + Sync>;

/// The locked runtime state the backend half and the pump share. Every field
/// is `Send`: the `!Send` driver lives only in the pump.
struct RuntimeShared {
    core: Core,
    bus: Arc<EventBus>,
    /// Per-call reply senders, keyed by [`CallId`] — settlement removes the
    /// sender, guaranteeing exactly-once delivery (mirrors `kernel/mod.rs`).
    senders: HashMap<CallId, oneshot::Sender<Result<RawJson, CallError>>>,
    /// The `Send` outbound IO-action mailbox the pump drains.
    mailbox: VecDeque<IoAction>,
}

impl RuntimeShared {
    /// Step the core at logical time `now` and split its actions: transport-bound
    /// ones into the mailbox, wake-producing ones into the returned [`Deferred`]
    /// batch (to be delivered after the lock drops).
    fn drive(&mut self, input: Input, now: Tick) -> Deferred {
        let mut deferred = Deferred::new(self.bus.clone());
        let actions = self.core.step(input, now);
        for action in actions {
            self.apply_one(action, &mut deferred);
        }
        deferred
    }

    fn apply_one(&mut self, action: Action, deferred: &mut Deferred) {
        match action {
            Action::Send(frame) => self.mailbox.push_back(IoAction::Send(frame)),
            Action::ArmTimer { id, at } => self.mailbox.push_back(IoAction::ArmTimer { id, at }),
            Action::CancelTimer(id) => self.mailbox.push_back(IoAction::CancelTimer(id)),
            Action::Dial { token } => self.mailbox.push_back(IoAction::Dial { token }),
            Action::CancelDial => self.mailbox.push_back(IoAction::CancelDial),
            Action::Settle(call_id, result) => {
                if let Some(sender) = self.senders.remove(&call_id) {
                    deferred.work.push(DeferredWake::Settle(sender, result));
                }
            }
            Action::DropSender(call_id) => {
                self.senders.remove(&call_id);
            }
            Action::Emit(event) => deferred.work.push(DeferredWake::Emit(event)),
            Action::CloseBus => deferred.work.push(DeferredWake::CloseBus),
        }
    }
}

/// The serialized deferred-wake delivery queue — the same re-entrancy-safe
/// single-drainer contract `kernel/mod.rs` documents, factored so this runtime
/// reuses it. Batches are enqueued while the `Shared` lock is held (queue order
/// == drive order) and drained after every lock drops, so a waker backed by an
/// inline browser executor cannot deadlock by re-entering the backend.
struct DeliveryQueue {
    queue: Mutex<VecDeque<Deferred>>,
    draining: AtomicBool,
}

impl DeliveryQueue {
    fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            draining: AtomicBool::new(false),
        }
    }

    fn enqueue(&self, deferred: Deferred) {
        if deferred.is_empty() {
            return;
        }
        self.queue
            .lock()
            .expect("delivery queue poisoned")
            .push_back(deferred);
    }

    fn drain(&self) {
        loop {
            if self.draining.swap(true, Ordering::AcqRel) {
                return;
            }
            loop {
                let next = {
                    self.queue
                        .lock()
                        .expect("delivery queue poisoned")
                        .pop_front()
                };
                match next {
                    Some(deferred) => deferred.deliver(),
                    None => break,
                }
            }
            self.draining.store(false, Ordering::Release);
            let empty = self
                .queue
                .lock()
                .expect("delivery queue poisoned")
                .is_empty();
            if empty {
                return;
            }
        }
    }
}

/// The shared runtime both halves hold via `Arc`.
struct Runtime {
    shared: Mutex<RuntimeShared>,
    delivery: DeliveryQueue,
    /// Woken by the backend whenever it enqueues IO for the pump.
    pump_waker: AtomicWaker,
    /// The fresh clock read on every core step (never cached).
    clock: RuntimeClock,
}

impl Runtime {
    /// Drive one input with globally serialized delivery: read the clock fresh,
    /// enqueue the deferred batch while holding the `Shared` lock, then drain
    /// outside every lock.
    fn drive(&self, input: Input) {
        let now = (self.clock)();
        {
            let mut shared = self.shared.lock().expect("runtime shared poisoned");
            let deferred = shared.drive(input, now);
            self.delivery.enqueue(deferred);
        }
        self.delivery.drain();
    }
}

/// The `Send + Sync` [`ClientBackend`] half. It never touches the driver: it
/// steps the core eagerly and hands transport work to the pump via the mailbox.
pub(crate) struct RuntimeBackend {
    runtime: Arc<Runtime>,
}

impl ClientBackend for RuntimeBackend {
    fn dispatch(&self, call: ErasedCall) -> BoxFuture<'static, Result<RawJson, CallError>> {
        let now = (self.runtime.clock)();
        let (receiver, call_id) = {
            let mut shared = self.runtime.shared.lock().expect("runtime shared poisoned");
            let call_id = shared.core.alloc_call_id();
            let (sender, receiver) = oneshot::channel();
            shared.senders.insert(call_id, sender);
            let deferred = shared.drive(Input::Dispatch { call_id, call }, now);
            self.runtime.delivery.enqueue(deferred);
            (receiver, call_id)
        };
        self.runtime.delivery.drain();
        // A dispatch may have queued an outbound frame (or, from Idle, nothing);
        // wake the pump so it drains the mailbox.
        self.runtime.pump_waker.wake();
        Box::pin(DispatchFuture {
            receiver,
            cancel: Some((Arc::downgrade(&self.runtime), call_id)),
        })
    }

    fn subscribe(&self) -> EventSubscription {
        // Clone the bus under the Shared lock, register outside it (mirrors
        // `kernel/mod.rs`: broadcast may re-enter Shared).
        let bus = {
            self.runtime
                .shared
                .lock()
                .expect("runtime shared poisoned")
                .bus
                .clone()
        };
        bus.subscribe()
    }

    fn state(&self) -> State {
        self.runtime
            .shared
            .lock()
            .expect("runtime shared poisoned")
            .core
            .state()
    }

    fn start(&self) {
        self.runtime.drive(Input::Start);
        self.runtime.pump_waker.wake();
    }

    fn stop(&self) -> BoxFuture<'static, ()> {
        let now = (self.runtime.clock)();
        let (done_tx, done_rx) = oneshot::channel();
        {
            let mut shared = self.runtime.shared.lock().expect("runtime shared poisoned");
            let mut deferred = shared.drive(Input::Stop, now);
            deferred.done = Some(done_tx);
            self.runtime.delivery.enqueue(deferred);
        }
        self.runtime.delivery.drain();
        self.runtime.pump_waker.wake();
        Box::pin(async move {
            let _ = done_rx.await;
        })
    }
}

/// The reply future returned by [`RuntimeBackend::dispatch`]. Dropping it before
/// it resolves feeds an `Input::Cancel` (mirrors `kernel/mod.rs`).
struct DispatchFuture {
    receiver: oneshot::Receiver<Result<RawJson, CallError>>,
    cancel: Option<(Weak<Runtime>, CallId)>,
}

impl Future for DispatchFuture {
    type Output = Result<RawJson, CallError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        match Pin::new(&mut this.receiver).poll(cx) {
            Poll::Ready(Ok(result)) => {
                this.cancel = None;
                Poll::Ready(result)
            }
            Poll::Ready(Err(_canceled)) => {
                this.cancel = None;
                Poll::Ready(Err(CallError::Local(LocalError::Backend)))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for DispatchFuture {
    fn drop(&mut self) {
        if let Some((weak, call_id)) = self.cancel.take() {
            if let Some(runtime) = weak.upgrade() {
                runtime.drive(Input::Cancel(call_id));
                runtime.pump_waker.wake();
            }
        }
    }
}

/// The `!Send`-capable pump: a single future the platform `spawn_local`s. It
/// owns the [`Driver`], drains the mailbox into it, and feeds the driver's
/// events back into the core.
pub(crate) struct Pump<D: Driver> {
    runtime: Arc<Runtime>,
    driver: D,
}

impl<D> Future for Pump<D>
where
    D: Driver + Unpin,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = self.get_mut();
        // Register for backend -> pump wakes (a dispatch/start/stop/cancel that
        // enqueued IO). Driver -> pump wakes arrive through `poll_event`'s `cx`.
        this.runtime.pump_waker.register(cx.waker());
        loop {
            // Take everything the core has queued for the transport.
            let actions: Vec<IoAction> = {
                let mut shared = this.runtime.shared.lock().expect("runtime shared poisoned");
                shared.mailbox.drain(..).collect()
            };
            let mut progressed = !actions.is_empty();
            for action in actions {
                match action {
                    IoAction::Send(frame) => {
                        if this.driver.send(frame).is_err() {
                            // The send/close race (§K14): report the loss on the
                            // generation the broken transport was on.
                            let generation = {
                                this.runtime
                                    .shared
                                    .lock()
                                    .expect("runtime shared poisoned")
                                    .core
                                    .generation()
                            };
                            this.runtime.drive(Input::Interrupted { generation });
                        }
                    }
                    IoAction::Dial { token } => this.driver.dial(token),
                    IoAction::CancelDial => this.driver.cancel_dial(),
                    IoAction::ArmTimer { id, at } => this.driver.arm_timer(id, at),
                    IoAction::CancelTimer(id) => this.driver.cancel_timer(id),
                }
            }
            // Drain every event the driver has ready right now.
            while let Poll::Ready(event) = this.driver.poll_event(cx) {
                progressed = true;
                let input = match event {
                    DriverEvent::Inbound(inbound) => Input::Inbound(inbound),
                    DriverEvent::Connected { token } => Input::Connected { token },
                    DriverEvent::DialFailed { token } => Input::DialFailed { token },
                    DriverEvent::GateRefused { token } => Input::GateRefused { token },
                    DriverEvent::Interrupted { generation } => Input::Interrupted { generation },
                    DriverEvent::TimerFired(id) => Input::TimerFired(id),
                };
                this.runtime.drive(input);
            }
            if !progressed {
                return Poll::Pending;
            }
            // Feeding events may have queued new IO; re-register and loop so a
            // single wake drains the core to a fixed point.
            this.runtime.pump_waker.register(cx.waker());
        }
    }
}

/// Build a kernel-backed handle over `driver`, returning the handle plus the
/// [`Pump`] the platform must `spawn_local` (wasm) or spawn on its runtime
/// (native). `clock` reads the platform's monotonic time and is invoked fresh
/// on every core step. Nothing runs until the pump is polled.
pub(crate) fn build<D: Driver>(
    config: KernelConfig,
    driver: D,
    clock: RuntimeClock,
) -> (ClientHandle, Pump<D>) {
    let runtime = Arc::new(Runtime {
        shared: Mutex::new(RuntimeShared {
            core: Core::new(config.limits, config.jitter_seed, config.stable_principal),
            bus: Arc::new(EventBus::new()),
            senders: HashMap::new(),
            mailbox: VecDeque::new(),
        }),
        delivery: DeliveryQueue::new(),
        pump_waker: AtomicWaker::new(),
        clock,
    });
    let handle = ClientHandle::from_backend(Arc::new(RuntimeBackend {
        runtime: Arc::clone(&runtime),
    }));
    let pump = Pump { runtime, driver };
    (handle, pump)
}

// ---------------------------------------------------------------------------
// Tests — generic runtime via an in-memory `Driver` (spec §9.1 step 3)
//
// Five kernel scenarios (connect/reply round-trip, disconnect classification,
// generation fencing, stop from each phase) reproduced **through the runtime**
// rather than just the sans-IO core, proving the runtime correctly wires
// Driver actions/events to and from `Input`s.
//
// The `FakeDriver` is `!Send` (uses `Rc<RefCell<_>>`) and lives only in the
// pump, mirroring the `WsWeb` mailbox-split design.  A plain `Box::new(||
// Tick(N))` satisfies `RuntimeClock` (`Send + Sync`).  `LocalPool` drives the
// pump alongside the dispatch futures; `run_until_stalled` advances both to a
// fixed point so test code can inspect state without timing dependencies.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    use futures::executor::LocalPool;
    use futures::task::LocalSpawnExt;

    use crate::event::State;
    use crate::kernel::timing::Tick;
    use crate::kernel::transport::{
        Driver, DriverEvent, Inbound, TransportClosed, WireFrame, WireReply,
    };
    use crate::KernelConfig;

    // ---- FakeDriver -------------------------------------------------------

    struct FakeInner {
        /// Events queued for `poll_event`.
        events: VecDeque<DriverEvent>,
        /// Waker to wake the pump when an event is pushed.
        waker: Option<std::task::Waker>,
        /// Tokens from `dial()` calls, in order.
        dials: Vec<u64>,
        /// (id, op) pairs from `send()` calls, in order.
        sends: Vec<(u64, &'static str)>,
        /// Whether `cancel_dial()` was called.
        cancel_dial_called: bool,
    }

    impl FakeInner {
        fn new() -> Self {
            Self {
                events: VecDeque::new(),
                waker: None,
                dials: Vec::new(),
                sends: Vec::new(),
                cancel_dial_called: false,
            }
        }

        /// Push an event and wake the pump.
        fn push(&mut self, event: DriverEvent) {
            self.events.push_back(event);
            if let Some(w) = &self.waker {
                w.wake_by_ref();
            }
        }
    }

    struct FakeDriver {
        inner: std::rc::Rc<std::cell::RefCell<FakeInner>>,
    }

    impl Driver for FakeDriver {
        fn poll_event(&mut self, cx: &mut Context<'_>) -> Poll<DriverEvent> {
            let mut inner = self.inner.borrow_mut();
            match inner.events.pop_front() {
                Some(ev) => Poll::Ready(ev),
                None => {
                    inner.waker = Some(cx.waker().clone());
                    Poll::Pending
                }
            }
        }

        fn send(&mut self, frame: WireFrame) -> Result<(), TransportClosed> {
            self.inner
                .borrow_mut()
                .sends
                .push((frame.id.get(), frame.op));
            Ok(())
        }

        fn dial(&mut self, token: u64) {
            self.inner.borrow_mut().dials.push(token);
        }

        fn cancel_dial(&mut self) {
            self.inner.borrow_mut().cancel_dial_called = true;
        }

        fn arm_timer(&mut self, _id: TimerId, _at: Tick) {}
        fn cancel_timer(&mut self, _id: TimerId) {}

        fn now(&self) -> Tick {
            Tick(1_000)
        }
    }

    fn make() -> (
        crate::handle::ClientHandle,
        std::rc::Rc<std::cell::RefCell<FakeInner>>,
        LocalPool,
    ) {
        let inner = std::rc::Rc::new(std::cell::RefCell::new(FakeInner::new()));
        let driver = FakeDriver {
            inner: std::rc::Rc::clone(&inner),
        };
        let (handle, pump) = build(
            KernelConfig {
                jitter_seed: 42,
                stable_principal: false,
                ..KernelConfig::default()
            },
            driver,
            Box::new(|| Tick(1_000)),
        );
        let pool = LocalPool::new();
        pool.spawner()
            .spawn_local(pump)
            .expect("spawn_local failed");
        (handle, inner, pool)
    }

    // ---- Scenario 1: connect → Ready --------------------------------------

    #[test]
    fn runtime_start_triggers_dial_and_connected_reaches_ready() {
        let (handle, inner, mut pool) = make();

        handle.start();
        pool.run_until_stalled();

        // The pump must have called dial() exactly once.
        let token = {
            let b = inner.borrow();
            assert_eq!(b.dials.len(), 1, "expected one dial");
            b.dials[0]
        };

        // Feed Connected — the runtime turns it into Input::Connected.
        inner.borrow_mut().push(DriverEvent::Connected { token });
        pool.run_until_stalled();

        assert_eq!(
            handle.state(),
            State::Ready,
            "handle must be Ready after Connected"
        );
    }

    // ---- Scenario 2: stop from Idle --------------------------------------

    #[test]
    fn runtime_stop_from_idle_reaches_stopped() {
        use futures::executor::block_on;

        let (handle, _inner, _pool) = make();
        // Stop before start: the `stop()` future resolves synchronously via
        // the delivery queue drain on the Send backend.
        block_on(handle.stop());
        assert_eq!(handle.state(), State::Stopped);
    }

    // ---- Scenario 3: stop while Connecting settles queued as DefinitelyNot

    #[test]
    fn runtime_stop_while_connecting_settles_queued_call_as_definitely_not() {
        use crate::error::{CallError, Execution};
        use futures::executor::block_on;
        use jeliya_api::RoomList;

        let (handle, _inner, mut pool) = make();

        handle.start();
        pool.run_until_stalled();
        assert_eq!(handle.state(), State::Connecting);

        // Dispatch a call while Connecting (will be queued, not sent).
        let fut = handle.call::<RoomList>(RoomList {}, crate::Dedup::None);

        // Stop — settles the queued call as Cancelled{DefinitelyNot}.
        let stop_fut = handle.stop();
        // Drain delivery queue (synchronous settle happens in stop()'s drain).
        pool.run_until_stalled();

        // The dispatch future must now resolve.
        let err = block_on(fut).expect_err("queued call must be cancelled");
        assert!(
            matches!(
                err,
                CallError::Cancelled {
                    execution: Execution::DefinitelyNot
                }
            ),
            "expected Cancelled{{DefinitelyNot}}, got {err:?}"
        );

        block_on(stop_fut);
        assert_eq!(handle.state(), State::Stopped);
    }

    // ---- Scenario 4: Interrupted while Ready transitions to Interrupted -----

    #[test]
    fn runtime_interrupted_transitions_ready_to_interrupted() {
        let (handle, inner, mut pool) = make();

        // Reach Ready.
        handle.start();
        pool.run_until_stalled();
        let token = inner.borrow().dials[0];
        inner.borrow_mut().push(DriverEvent::Connected { token });
        pool.run_until_stalled();
        assert_eq!(handle.state(), State::Ready);

        // Feed Interrupted on the current generation (1, the first Connected).
        inner
            .borrow_mut()
            .push(DriverEvent::Interrupted { generation: 1 });
        pool.run_until_stalled();

        // The core transitions to Interrupted (the "was Ready, now recovering"
        // state). The kernel arms a backoff timer before re-dialing; since
        // FakeDriver::arm_timer is a no-op, no second dial is issued yet — only
        // the state transition itself is proven here.
        assert_eq!(
            handle.state(),
            State::Interrupted,
            "handle must enter Interrupted after losing a Ready connection"
        );
        assert_eq!(
            inner.borrow().dials.len(),
            1,
            "no immediate re-dial before the backoff timer fires"
        );
    }

    // ---- Scenario 5: generation fencing drops stale inbound ---------------

    #[test]
    fn runtime_generation_fencing_drops_stale_inbound_reply() {
        use crate::error::{CallError, Execution};
        use futures::executor::block_on;
        use jeliya_api::{RequestId, RoomList};

        let (handle, inner, mut pool) = make();

        // Reach Ready (generation 1).
        handle.start();
        pool.run_until_stalled();
        let token1 = inner.borrow().dials[0];
        inner
            .borrow_mut()
            .push(DriverEvent::Connected { token: token1 });
        pool.run_until_stalled();
        assert_eq!(handle.state(), State::Ready);

        // Dispatch a call — the pump sends the frame; record the wire id.
        let fut = handle.call::<RoomList>(RoomList {}, crate::Dedup::None);
        pool.run_until_stalled();
        let wire_id = {
            let b = inner.borrow();
            assert!(!b.sends.is_empty(), "expected an outbound frame");
            b.sends[0].0
        };

        // Disconnect: core transitions to Connecting, in-flight call is
        // settled Unknown (not received before the connection dropped).
        inner
            .borrow_mut()
            .push(DriverEvent::Interrupted { generation: 1 });
        pool.run_until_stalled();

        // Now push a stale reply on generation 0 (old socket): the core must
        // fence it (drop with no effect on any live call).
        let stale_reply = DriverEvent::Inbound(Inbound::Reply {
            generation: 0, // stale: the socket that connection was on had gen=1
            id: RequestId::new(wire_id).unwrap(),
            result: WireReply::Ok(RawJson::from_string(String::from("{\"rooms\":[]}"))),
        });
        inner.borrow_mut().push(stale_reply);
        pool.run_until_stalled();

        // The dispatch future should have been settled as Unknown (by the
        // Interrupted), not as an Ok from the stale reply.
        let err = block_on(fut).expect_err("call interrupted before reply must fail");
        assert!(
            matches!(
                err,
                CallError::Disconnected {
                    execution: Execution::Unknown
                }
            ),
            "expected Disconnected{{Unknown}}, got {err:?}"
        );
    }
}
