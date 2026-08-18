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
use crate::kernel::transport::{Driver, DriverEvent, MediaEvent, StreamRecordIntent, WireFrame};
use crate::media::StreamMedia;
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
    /// Send one client-authored byte-stream control record (§S3); the driver
    /// frames it via `jeliya-codec` at the boundary.
    SendRecord(StreamRecordIntent),
    /// Fulfil one producer media grant (§S3): the driver reads, frames, and
    /// sends ≤ `up_to` bytes from the stream's registered source.
    ProduceData {
        /// The stream's wire id.
        id: jeliya_api::RequestId,
        /// The stream call.
        call_id: CallId,
        /// The maximum additional bytes the driver may send now.
        up_to: u64,
    },
    /// Hand one accepted inbound DATA range to the stream's registered sink
    /// (§S3).
    WriteSink {
        /// The stream's wire id.
        id: jeliya_api::RequestId,
        /// The stream call.
        call_id: CallId,
        /// The range's start offset.
        offset: u64,
        /// The range length in bytes.
        len: u64,
    },
    /// Register one stream's media under its dedup key, before the call is
    /// dispatched. The media types are `Send + Sync`, so the registration
    /// crosses the mailbox to the `!Send` driver half safely.
    RegisterMedia {
        /// The operation's dedup key.
        key: jeliya_api::OpId,
        /// The caller-registered source or sink.
        media: StreamMedia,
    },
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
            Action::SendRecord(intent) => self.mailbox.push_back(IoAction::SendRecord(intent)),
            Action::ProduceData { id, call_id, up_to } => self
                .mailbox
                .push_back(IoAction::ProduceData { id, call_id, up_to }),
            Action::WriteSink {
                id,
                call_id,
                offset,
                len,
            } => self.mailbox.push_back(IoAction::WriteSink {
                id,
                call_id,
                offset,
                len,
            }),
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

    fn register_stream_media(
        &self,
        key: jeliya_api::OpId,
        media: StreamMedia,
    ) -> Result<(), LocalError> {
        // The driver owns the media registry (§S3 media seam): enqueue the
        // registration into the mailbox under the shared lock (mirroring
        // `dispatch`'s Send) and wake the pump, so it lands before any later
        // stream send drains after it.
        {
            let mut shared = self.runtime.shared.lock().expect("runtime shared poisoned");
            shared
                .mailbox
                .push_back(IoAction::RegisterMedia { key, media });
        }
        self.runtime.pump_waker.wake();
        Ok(())
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

/// Report a synchronous send failure (Text or stream record) as the §K14
/// send/close race: an `Interrupted` on the generation the broken transport
/// was on, driven after the lock is dropped.
fn report_transport_loss(runtime: &Runtime) {
    let generation = runtime
        .shared
        .lock()
        .expect("runtime shared poisoned")
        .core
        .generation();
    runtime.drive(Input::Interrupted { generation });
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
                            report_transport_loss(this.runtime.as_ref());
                        }
                    }
                    IoAction::SendRecord(intent) => {
                        // A record send fails exactly like a Text send: the
                        // same §K14 race handling.
                        if this.driver.send_record(intent).is_err() {
                            report_transport_loss(this.runtime.as_ref());
                        }
                    }
                    IoAction::ProduceData { id, call_id, up_to } => {
                        this.driver.produce(id, call_id, up_to)
                    }
                    IoAction::WriteSink {
                        id,
                        call_id,
                        offset,
                        len,
                    } => this.driver.write_sink(id, call_id, offset, len),
                    IoAction::RegisterMedia { key, media } => {
                        this.driver.register_media(key, media)
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
                    DriverEvent::Connected { token, incarnation } => {
                        Input::Connected { token, incarnation }
                    }
                    DriverEvent::DialFailed { token } => Input::DialFailed { token },
                    DriverEvent::GateRefused { token } => Input::GateRefused { token },
                    DriverEvent::Interrupted { generation } => Input::Interrupted { generation },
                    DriverEvent::TimerFired(id) => Input::TimerFired(id),
                    DriverEvent::StreamFault { generation, id } => {
                        Input::StreamFault { generation, id }
                    }
                    DriverEvent::Media(media) => match media {
                        MediaEvent::Produced {
                            call_id,
                            sent_through,
                        } => Input::Produced {
                            call_id,
                            sent_through,
                        },
                        MediaEvent::SourceEnd { call_id, total } => {
                            Input::SourceEnd { call_id, total }
                        }
                        MediaEvent::SourceFailed { call_id } => Input::SourceFailed { call_id },
                        MediaEvent::SinkAccepted { call_id, through } => {
                            Input::SinkAccepted { call_id, through }
                        }
                        MediaEvent::SinkFailed { call_id } => Input::SinkFailed { call_id },
                    },
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
            // The configured stream bounds (§6) — the driver-side media
            // registry's inbound quarantine cap keys off the same
            // `stream_window_bytes`.
            core: Core::with_stream_limits(
                config.limits,
                config.jitter_seed,
                config.stable_principal,
                config.streams,
            ),
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
        Driver, DriverEvent, Inbound, StreamRecordIntent, StreamRecordMeta, TransportClosed,
        WireFrame, WireReply,
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
        /// Caller media registrations awaiting a stream op's send, keyed by
        /// dedup `OpId` (mirrors the real drivers' registries).
        registered: std::collections::HashMap<jeliya_api::OpId, crate::media::StreamMedia>,
        /// Per-bound-stream driver state, keyed by wire id.
        bound: std::collections::HashMap<jeliya_api::RequestId, BoundFakeStream>,
        /// Outbound stream-record observations: (wire id, kind, a, b) — a
        /// bound, never content (§S12).
        records: Vec<(u64, &'static str, u64, u64)>,
    }

    impl FakeInner {
        fn new() -> Self {
            Self {
                events: VecDeque::new(),
                waker: None,
                dials: Vec::new(),
                sends: Vec::new(),
                cancel_dial_called: false,
                registered: std::collections::HashMap::new(),
                bound: std::collections::HashMap::new(),
                records: Vec::new(),
            }
        }

        /// Push an event and wake the pump.
        fn push(&mut self, event: DriverEvent) {
            self.events.push_back(event);
            if let Some(w) = &self.waker {
                w.wake_by_ref();
            }
        }

        /// Bind a stream op's wire id to the caller's registration (moves
        /// it), exactly like the real drivers.
        fn bind(&mut self, wire_id: jeliya_api::RequestId, op_id: Option<jeliya_api::OpId>) {
            let media = op_id.and_then(|key| self.registered.remove(&key));
            let stream = BoundFakeStream {
                media,
                ..Default::default()
            };
            self.bound.insert(wire_id, stream);
        }

        /// Observe one outbound record intent as a redaction-safe view.
        fn record(&mut self, id: u64, kind: &'static str, a: u64, b: u64) {
            self.records.push((id, kind, a, b));
        }

        // ---- scripted daemon peer (the test drives these by hand) ---------

        /// Deliver the daemon's OPEN for `wire_id` on the live generation.
        fn open(&mut self, wire_id: u64, total: u64) {
            let id = jeliya_api::RequestId::new(wire_id).expect("wire id");
            self.push(DriverEvent::Inbound(Inbound::Record {
                generation: 1,
                record: StreamRecordMeta::Open {
                    id,
                    stream_id: stream_id_for(wire_id),
                    total,
                },
            }));
        }

        /// Deliver a daemon CREDIT (the client is the producer).
        fn credit(&mut self, wire_id: u64, accepted_through: u64, send_through: u64) {
            let id = jeliya_api::RequestId::new(wire_id).expect("wire id");
            self.push(DriverEvent::Inbound(Inbound::Record {
                generation: 1,
                record: StreamRecordMeta::Credit {
                    id,
                    stream_id: stream_id_for(wire_id),
                    accepted_through,
                    send_through,
                },
            }));
        }

        /// Deliver a daemon DATA payload (the client is the receiver):
        /// quarantine the bytes first, then hand the core the byte-free meta.
        fn deliver_data(&mut self, wire_id: u64, offset: u64, payload: &[u8]) {
            let id = jeliya_api::RequestId::new(wire_id).expect("wire id");
            let len = payload.len() as u64;
            if let Some(stream) = self.bound.get_mut(&id) {
                stream.inbound.insert(offset, payload.to_vec());
            }
            self.push(DriverEvent::Inbound(Inbound::Record {
                generation: 1,
                record: StreamRecordMeta::Data {
                    id,
                    stream_id: stream_id_for(wire_id),
                    offset,
                    len,
                },
            }));
        }

        /// Deliver the daemon's END (the client is the receiver).
        fn end(&mut self, wire_id: u64, offset: u64) {
            let id = jeliya_api::RequestId::new(wire_id).expect("wire id");
            self.push(DriverEvent::Inbound(Inbound::Record {
                generation: 1,
                record: StreamRecordMeta::End {
                    id,
                    stream_id: stream_id_for(wire_id),
                    offset,
                },
            }));
        }

        /// Deliver the terminal Text reply for `wire_id`.
        fn deliver_reply(&mut self, wire_id: u64, out: &str) {
            self.push(DriverEvent::Inbound(Inbound::Reply {
                generation: 1,
                id: jeliya_api::RequestId::new(wire_id).expect("wire id"),
                result: WireReply::Ok(RawJson::from_string(String::from(out))),
            }));
        }
    }

    /// One bound stream's fake-driver state: the caller's media, the
    /// producer's read position, and the receiver's inbound quarantine.
    #[derive(Default)]
    struct BoundFakeStream {
        media: Option<crate::media::StreamMedia>,
        produced: u64,
        inbound: std::collections::BTreeMap<u64, Vec<u8>>,
    }

    /// A nonzero connection-local stream id (the reference rig's convention:
    /// the core routes by call, never by the stream id).
    fn stream_id_for(wire_id: u64) -> u128 {
        (wire_id as u128).wrapping_add(1).max(1)
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
            let mut inner = self.inner.borrow_mut();
            if crate::kernel::replay::is_stream_op(frame.op) {
                inner.bind(frame.id, frame.op_id);
            }
            inner.sends.push((frame.id.get(), frame.op));
            Ok(())
        }

        fn media_event(&mut self, event: crate::kernel::transport::MediaEvent) {
            self.inner.borrow_mut().push(DriverEvent::Media(event));
        }

        fn send_record(&mut self, intent: StreamRecordIntent) -> Result<(), TransportClosed> {
            use crate::kernel::transport::StreamRecordIntent as I;
            let mut inner = self.inner.borrow_mut();
            match intent {
                I::Credit {
                    id,
                    accepted_through,
                    send_through,
                    ..
                } => inner.record(id.get(), "credit", accepted_through, send_through),
                I::End { id, offset, .. } => inner.record(id.get(), "end", offset, 0),
                I::Abort { id, high_water, .. } => inner.record(id.get(), "abort", high_water, 0),
                I::Ack { id, high_water, .. } => inner.record(id.get(), "ack", high_water, 0),
            }
            Ok(())
        }

        /// Real single-threaded fulfillment (mirrors the web registry): read
        /// the granted bytes from the bound source in codec-bounded chunks,
        /// observe each DATA record, and surface the media events.
        fn produce(&mut self, id: jeliya_api::RequestId, call_id: CallId, up_to: u64) {
            use crate::kernel::transport::MediaEvent;
            use crate::media::StreamMedia;
            let chunk_cap = jeliya_codec::max_stream_data_bytes(
                jeliya_codec::CodecBounds::default().max_frame_bytes,
            )
            .expect("default bounds frame a DATA record");
            let mut inner = self.inner.borrow_mut();
            let source = match inner.bound.get_mut(&id) {
                Some(stream) => match stream.media.as_ref() {
                    Some(StreamMedia::Source(source)) => Some(source.clone()),
                    _ => None,
                },
                None => None,
            };
            let Some(source) = source else {
                inner.push(DriverEvent::Media(MediaEvent::SourceFailed { call_id }));
                return;
            };
            let stream = inner.bound.get_mut(&id).expect("bound above");
            let mut position = stream.produced;
            let mut remaining = up_to;
            while remaining > 0 {
                let want = remaining.min(chunk_cap as u64) as usize;
                let mut buf = vec![0_u8; want];
                let read = source.read_at(position, &mut buf);
                if read == 0 {
                    if position >= source.len() {
                        inner.push(DriverEvent::Media(MediaEvent::SourceEnd {
                            call_id,
                            total: position,
                        }));
                    } else {
                        inner.push(DriverEvent::Media(MediaEvent::SourceFailed { call_id }));
                    }
                    break;
                }
                inner.record(id.get(), "data", position, read as u64);
                position += read as u64;
                remaining = remaining.saturating_sub(read as u64);
                inner.push(DriverEvent::Media(MediaEvent::Produced {
                    call_id,
                    sent_through: position,
                }));
                if let Some(stream) = inner.bound.get_mut(&id) {
                    stream.produced = position;
                }
            }
            if remaining == 0 && position >= source.len() {
                inner.push(DriverEvent::Media(MediaEvent::SourceEnd {
                    call_id,
                    total: position,
                }));
            }
        }

        /// Real hand-off (mirrors the web registry): take the contiguous
        /// quarantined range and write it to the bound sink.
        fn write_sink(
            &mut self,
            id: jeliya_api::RequestId,
            call_id: CallId,
            offset: u64,
            len: u64,
        ) {
            use crate::kernel::transport::MediaEvent;
            use crate::media::StreamMedia;
            let mut inner = self.inner.borrow_mut();
            let sink = match inner.bound.get_mut(&id) {
                Some(stream) => match stream.media.as_ref() {
                    Some(StreamMedia::Sink(sink)) => Some(sink.clone()),
                    _ => None,
                },
                None => None,
            };
            let Some(sink) = sink else {
                inner.push(DriverEvent::Media(MediaEvent::SinkFailed { call_id }));
                return;
            };
            // Take the contiguous range [offset, offset+len).
            let mut collected: Vec<u8> = Vec::new();
            let mut covered: u64 = 0;
            let stream = inner.bound.get_mut(&id).expect("bound above");
            while covered < len {
                let key = offset.saturating_add(covered);
                let needed = (len - covered) as usize;
                let Some(mut chunk) = stream.inbound.remove(&key) else {
                    inner.push(DriverEvent::Media(MediaEvent::SinkFailed { call_id }));
                    return;
                };
                if chunk.len() > needed {
                    let suffix_offset = key.saturating_add(needed as u64);
                    let suffix = chunk.split_off(needed);
                    stream.inbound.insert(suffix_offset, suffix);
                    collected.extend_from_slice(&chunk);
                    covered = len;
                } else {
                    collected.extend_from_slice(&chunk);
                    covered += chunk.len() as u64;
                }
            }
            match sink.write_at(offset, &collected) {
                Ok(()) => inner.push(DriverEvent::Media(MediaEvent::SinkAccepted {
                    call_id,
                    through: offset.saturating_add(len),
                })),
                Err(_) => inner.push(DriverEvent::Media(MediaEvent::SinkFailed { call_id })),
            }
        }

        fn register_media(&mut self, key: jeliya_api::OpId, media: crate::media::StreamMedia) {
            self.inner.borrow_mut().registered.insert(key, media);
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

    /// A driver exercising the trait's media defaults: it authors stream
    /// records (so the abort an honest failure provokes is observable) but
    /// moves no bytes — `produce`/`write_sink`/`register_media` are the
    /// provided honest-failure implementations.
    struct BareDriver {
        inner: std::rc::Rc<std::cell::RefCell<FakeInner>>,
    }

    impl Driver for BareDriver {
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

        fn media_event(&mut self, event: crate::kernel::transport::MediaEvent) {
            self.inner.borrow_mut().push(DriverEvent::Media(event));
        }

        fn send_record(&mut self, intent: StreamRecordIntent) -> Result<(), TransportClosed> {
            use crate::kernel::transport::StreamRecordIntent as I;
            let mut inner = self.inner.borrow_mut();
            match intent {
                I::Credit {
                    id,
                    accepted_through,
                    send_through,
                    ..
                } => inner.record(id.get(), "credit", accepted_through, send_through),
                I::End { id, offset, .. } => inner.record(id.get(), "end", offset, 0),
                I::Abort { id, high_water, .. } => inner.record(id.get(), "abort", high_water, 0),
                I::Ack { id, high_water, .. } => inner.record(id.get(), "ack", high_water, 0),
            }
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
        inner.borrow_mut().push(DriverEvent::Connected {
            token,
            incarnation: jeliya_api::Incarnation::new("runtime-test"),
        });
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
        inner.borrow_mut().push(DriverEvent::Connected {
            token,
            incarnation: jeliya_api::Incarnation::new("runtime-test"),
        });
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
        inner.borrow_mut().push(DriverEvent::Connected {
            token: token1,
            incarnation: jeliya_api::Incarnation::new("runtime-test"),
        });
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

    // ---- Stream media scenarios (#233 web media drive) -------------------
    //
    // The FakeDriver is a scripted daemon peer with REAL single-threaded
    // media fulfillment (mirroring the web registry): a registered
    // ByteSource is read in codec-bounded chunks at each ProduceData grant,
    // and quarantined inbound DATA is written to the registered ByteSink at
    // each WriteSink. These prove the mailbox runtime's stream wiring end
    // to end — RegisterMedia → bind-at-send → SendRecord/ProduceData/
    // WriteSink actions → Media events back as Inputs — with no browser.

    /// Reach Ready (generation 1) with the FakeDriver, as scenarios 1/4/5 do.
    fn make_ready() -> (
        crate::handle::ClientHandle,
        std::rc::Rc<std::cell::RefCell<FakeInner>>,
        LocalPool,
    ) {
        let (handle, inner, mut pool) = make();
        handle.start();
        pool.run_until_stalled();
        let token = inner.borrow().dials[0];
        inner.borrow_mut().push(DriverEvent::Connected {
            token,
            incarnation: jeliya_api::Incarnation::new("runtime-test"),
        });
        pool.run_until_stalled();
        assert_eq!(handle.state(), State::Ready);
        (handle, inner, pool)
    }

    /// Like [`make_ready`] but over the [`BareDriver`] (the trait's honest
    /// media defaults): no registered media can ever land.
    fn make_ready_bare() -> (
        crate::handle::ClientHandle,
        std::rc::Rc<std::cell::RefCell<FakeInner>>,
        LocalPool,
    ) {
        let inner = std::rc::Rc::new(std::cell::RefCell::new(FakeInner::new()));
        let driver = BareDriver {
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
        let mut pool = LocalPool::new();
        pool.spawner()
            .spawn_local(pump)
            .expect("spawn_local failed");
        handle.start();
        pool.run_until_stalled();
        let token = inner.borrow().dials[0];
        inner.borrow_mut().push(DriverEvent::Connected {
            token,
            incarnation: jeliya_api::Incarnation::new("runtime-test"),
        });
        pool.run_until_stalled();
        (handle, inner, pool)
    }

    /// Upload round-trip: `call_stream::<FileShare>` registers a real source
    /// through the handle, the FakeDriver answers OPEN + CREDIT, receives
    /// DATA/END records with offsets/lengths within the granted credit, and
    /// the terminal reply resolves the call.
    #[test]
    fn runtime_stream_upload_moves_real_bytes_through_the_mailbox() {
        use crate::handle::Dedup;
        use futures::executor::block_on;
        use jeliya_api::{FileShare, RoomId};

        let (handle, inner, mut pool) = make_ready();
        let key = jeliya_api::OpId::new("share-1");
        let payload: Vec<u8> = (0..200u32).map(|i| (i % 251) as u8).collect();
        handle
            .register_stream_media(key.clone(), crate::media::shared_bytes(payload))
            .expect("the runtime backend registers media");
        pool.run_until_stalled();

        let fut = handle.call_stream::<FileShare>(
            FileShare {
                room_id: RoomId::new("r1"),
                name: String::from("a.bin"),
                declared_bytes: 200,
                declared_content_type: String::from("application/octet-stream"),
            },
            Dedup::Key(key),
        );
        pool.run_until_stalled();
        let wire = {
            let b = inner.borrow();
            assert_eq!(b.sends.len(), 1, "the stream request reached the wire");
            b.sends[0].0
        };

        // Daemon OPENs the stream, then grants credit in two steps.
        inner.borrow_mut().open(wire, 200);
        pool.run_until_stalled();
        inner.borrow_mut().credit(wire, 0, 200);
        pool.run_until_stalled();
        inner.borrow_mut().credit(wire, 200, 200);
        pool.run_until_stalled();

        {
            let b = inner.borrow();
            let data: Vec<_> = b.records.iter().filter(|r| r.1 == "data").collect();
            assert!(!data.is_empty(), "at least one DATA record was produced");
            for record in &data {
                assert!(
                    record.2 + record.3 <= 200,
                    "DATA within granted credit: {record:?}"
                );
            }
            assert_eq!(
                data.iter().map(|r| r.3).sum::<u64>(),
                200,
                "every byte was produced"
            );
            let ends: Vec<_> = b.records.iter().filter(|r| r.1 == "end").collect();
            assert_eq!(ends.len(), 1, "exactly one END");
            assert_eq!(ends[0].2, 200, "END at the full total");
        }

        inner.borrow_mut().deliver_reply(
            wire,
            "{\"room_id\":\"r1\",\"file_id\":\"f1\",\"event_id\":\"e1\",\"pos\":0,\"bytes\":200,\"digest\":\"d\"}",
        );
        pool.run_until_stalled();
        let out = block_on(fut).expect("terminal reply resolves the stream");
        assert_eq!(out.bytes, 200);
        assert_eq!(out.file_id, jeliya_api::FileId::new("f1"));
    }

    /// Download round-trip: `call_stream::<FileRead>` registers a real
    /// `CollectedBytes` sink; the FakeDriver delivers OPEN/DATA/END plus the
    /// terminal reply, and the collected bytes match exactly.
    #[test]
    fn runtime_stream_download_collects_real_bytes_through_the_mailbox() {
        use crate::handle::Dedup;
        use futures::executor::block_on;
        use jeliya_api::{FileId, FileRead, FileReadOut, RoomId};

        let (handle, inner, mut pool) = make_ready();
        let key = jeliya_api::OpId::new("read-1");
        let (sink, media) = crate::media::collected_bytes();
        handle
            .register_stream_media(key.clone(), media)
            .expect("the runtime backend registers media");
        pool.run_until_stalled();

        let fut = handle.call_stream::<FileRead>(
            FileRead {
                room_id: RoomId::new("r1"),
                file_id: FileId::new("f1"),
            },
            Dedup::Key(key),
        );
        pool.run_until_stalled();
        let wire = {
            let b = inner.borrow();
            assert_eq!(b.sends.len(), 1);
            b.sends[0].0
        };

        let payload: Vec<u8> = (0..200u32).map(|i| (251 - i % 251) as u8).collect();
        inner.borrow_mut().open(wire, 200);
        pool.run_until_stalled();
        // The receiver grants credit on OPEN (observed as CREDIT records).
        {
            let b = inner.borrow();
            assert!(
                b.records.iter().any(|r| r.1 == "credit"),
                "the receiver grants credit on OPEN"
            );
        }
        inner.borrow_mut().deliver_data(wire, 0, &payload);
        pool.run_until_stalled();
        inner.borrow_mut().end(wire, 200);
        pool.run_until_stalled();

        assert_eq!(
            sink.len(),
            200,
            "the WriteSink hand-off wrote the quarantined bytes"
        );
        inner.borrow_mut().deliver_reply(
            wire,
            "{\"room_id\":\"r1\",\"file_id\":\"f1\",\"bytes\":200,\"declared_content_type\":\"application/octet-stream\"}",
        );
        pool.run_until_stalled();
        let out: FileReadOut = block_on(fut).expect("terminal reply resolves the stream");
        assert_eq!(out.bytes, 200);
        assert_eq!(sink.take(), payload, "the collected bytes match exactly");
    }

    /// An unregistered stream fails honestly through the trait's media
    /// defaults: the default `produce` surfaces `SourceFailed` as a
    /// `DriverEvent::Media`, the core aborts the stream (ABORT record on the
    /// wire) and settles the call — no hang, no fake success.
    #[test]
    fn runtime_unregistered_stream_fails_honestly_through_the_defaults() {
        use crate::handle::Dedup;
        use crate::CallError;
        use futures::executor::block_on;
        use jeliya_api::{FileShare, RoomId};

        let (handle, inner, mut pool) = make_ready_bare();
        let fut = handle.call_stream::<FileShare>(
            FileShare {
                room_id: RoomId::new("r1"),
                name: String::from("b.bin"),
                declared_bytes: 64,
                declared_content_type: String::from("application/octet-stream"),
            },
            Dedup::Key(jeliya_api::OpId::new("share-unregistered")),
        );
        pool.run_until_stalled();
        let wire = inner.borrow().sends[0].0;

        inner.borrow_mut().open(wire, 64);
        pool.run_until_stalled();
        inner.borrow_mut().credit(wire, 0, 64);
        pool.run_until_stalled();

        // The default produce reported SourceFailed: the core aborted the
        // stream (ABORT on the wire) and settled the call.
        {
            let b = inner.borrow();
            assert!(
                b.records.iter().any(|r| r.1 == "abort"),
                "the honest SourceFailure aborts the stream"
            );
            assert!(
                !b.records.iter().any(|r| r.1 == "data"),
                "no byte was ever produced"
            );
        }
        let err = block_on(fut).expect_err("an unregistered stream must fail, not hang");
        assert!(
            matches!(err, CallError::Timeout),
            "expected the core's honest stream-failure settle, got {err:?}"
        );
    }
}
