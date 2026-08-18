//! The kernel-backed [`ClientBackend`] for the direct path, plus the serialized
//! wake-delivery machinery it shares with the driver loop.
//!
//! This mirrors the kernel's own async shell ([`crate::kernel`] `KernelBackend`):
//! the sans-IO [`Core`] lives behind a mutex, the backend's `dispatch` / `start`
//! / `stop` / cancel step it synchronously, and the driver loop feeds it inputs.
//! All correctness — bounded admission, in-flight throttle, per-call deadlines,
//! cancel-on-drop, exactly-once settlement, and total-stop drain ordering — is
//! the reused `Core`'s, so the direct adapter's view-level behaviour is
//! structurally identical to the WebSocket adapters (AC "adapter contract tests
//! match WS view-level outcomes").
//!
//! The core's side-effecting actions (`Send`, `ArmTimer`, `Dial`, …) are
//! forwarded — under the `Shared` lock, non-blocking — to the async driver loop
//! as [`DriverMsg`]s; the wake-producing actions (`Settle`, `Emit`, `CloseBus`)
//! are deferred and delivered **after** the lock drops, so an inline-executor
//! waker that re-enters the backend cannot deadlock (the same wake hygiene the
//! kernel documents).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Instant;

use futures::channel::oneshot;
use futures::future::BoxFuture;
use tokio::sync::{mpsc, watch};

use crate::backend::{ClientBackend, ErasedCall, RawJson};
use crate::direct::driver::DriverMsg;
use crate::error::{CallError, LocalError};
use crate::event::{ClientEvent, EventBus, EventSubscription, State};
use crate::kernel::core::{Action, Core, Input};
use crate::kernel::inflight::CallId;
use crate::kernel::timing::Tick;
use crate::KernelLimits;

/// One wake-producing unit of deferred work, delivered after the `Shared` lock
/// drops, in the core's action order.
enum DeferredWake {
    Settle(
        oneshot::Sender<Result<RawJson, CallError>>,
        Result<RawJson, CallError>,
    ),
    Emit(ClientEvent),
    CloseBus,
}

/// A batch of deferred wakes plus the bus to deliver events on.
#[must_use]
struct Deferred {
    bus: Arc<EventBus>,
    work: Vec<DeferredWake>,
}

impl Deferred {
    fn is_empty(&self) -> bool {
        self.work.is_empty()
    }

    fn deliver(self) {
        for wake in self.work {
            match wake {
                DeferredWake::Settle(sender, result) => {
                    let _ = sender.send(result);
                }
                DeferredWake::Emit(event) => self.bus.broadcast(event),
                DeferredWake::CloseBus => self.bus.close(),
            }
        }
    }
}

/// The driver-owned mutable state the backend and the driver loop share: the
/// sans-IO core, the event bus, the per-call reply senders, the driver-message
/// sender the core's actions are forwarded on, and the shared clock origin.
struct Shared {
    core: Core,
    bus: Arc<EventBus>,
    /// Per-call reply senders keyed by [`CallId`]. Owning them here (not in the
    /// core) keeps the core sans-IO while still guaranteeing exactly-once
    /// settlement: a `Settle`/`DropSender` removes the sender, so a second
    /// settle for the same id is a no-op.
    senders: std::collections::HashMap<CallId, oneshot::Sender<Result<RawJson, CallError>>>,
    /// The core's side-effecting actions are forwarded here to the driver loop.
    /// Bounded (§9): occupancy is capped by the kernel's own queue/in-flight
    /// limits, and `try_send` never blocks the lock holder — a (sized-out) full
    /// channel degrades a call to its deadline rather than deadlocking.
    driver_tx: mpsc::Sender<DriverMsg>,
    /// The clock origin shared with the driver loop; `now()` is `Instant`
    /// elapsed in milliseconds (1 tick = 1 ms).
    origin: Instant,
}

impl Shared {
    fn now(&self) -> Tick {
        Tick(self.origin.elapsed().as_millis() as u64)
    }

    /// Step the core and apply its actions, returning the wake work to deliver
    /// after the lock drops.
    fn drive(&mut self, input: Input) -> Deferred {
        let mut deferred = Deferred {
            bus: self.bus.clone(),
            work: Vec::new(),
        };
        let now = self.now();
        let actions = self.core.step(input, now);
        for action in actions {
            self.apply_one(action, &mut deferred);
        }
        deferred
    }

    fn apply_one(&mut self, action: Action, deferred: &mut Deferred) {
        match action {
            // Side-effects: forwarded to the async driver loop. `try_send`
            // never blocks; the channel is sized past the kernel's own bounds.
            Action::Send(frame) => {
                let _ = self.driver_tx.try_send(DriverMsg::Send(frame));
            }
            Action::ArmTimer { id, at } => {
                let _ = self.driver_tx.try_send(DriverMsg::ArmTimer { id, at });
            }
            Action::CancelTimer(id) => {
                let _ = self.driver_tx.try_send(DriverMsg::CancelTimer(id));
            }
            Action::Dial { token } => {
                let _ = self.driver_tx.try_send(DriverMsg::Dial { token });
            }
            Action::CancelDial => {
                let _ = self.driver_tx.try_send(DriverMsg::CancelDial);
            }
            // Wake-producing: deferred until the lock drops.
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
            // Unreachable on this adapter: the direct actor refuses every
            // `stream.*` operation (they are connection-scoped — the engine
            // dispatcher answers `SubscriptionUnknown`), so the core never
            // opens a stream here and never emits stream actions. The debug
            // assertion keeps that honest if the deferred stream work lands.
            Action::SendRecord(_) | Action::ProduceData { .. } | Action::WriteSink { .. } => {
                debug_assert!(
                    false,
                    "stream actions are unreachable on the direct adapter"
                );
            }
        }
    }
}

/// The shared runtime: the locked driver state plus the serialized delivery
/// queue. Wake batches are enqueued while the `Shared` lock is held (so queue
/// order equals drive order across every thread), then drained by a single
/// drainer after the lock drops — wakers run outside the lock, yet cross-thread
/// delivery cannot invert two drives' effects.
pub(crate) struct DirectRuntime {
    shared: Mutex<Shared>,
    delivery: Mutex<VecDeque<Deferred>>,
    draining: AtomicBool,
}

impl DirectRuntime {
    /// Build the runtime around a fresh core. `stable_principal` is `true`:
    /// DirectClient is the one adapter that certifies it (in-process, a daemon
    /// restart is a client restart), and it is safe because the driver never
    /// emits `Interrupted`, so no replay window ever opens (§5.3).
    pub(crate) fn new(
        limits: KernelLimits,
        jitter_seed: u64,
        driver_tx: mpsc::Sender<DriverMsg>,
        origin: Instant,
    ) -> Self {
        Self {
            shared: Mutex::new(Shared {
                core: Core::new(limits, jitter_seed, true),
                bus: Arc::new(EventBus::new()),
                senders: std::collections::HashMap::new(),
                driver_tx,
                origin,
            }),
            delivery: Mutex::new(VecDeque::new()),
            draining: AtomicBool::new(false),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn enqueue(&self, deferred: Deferred) {
        if deferred.is_empty() {
            return;
        }
        self.delivery
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_back(deferred);
    }

    fn drain_delivery(&self) {
        loop {
            if self.draining.swap(true, Ordering::AcqRel) {
                return;
            }
            loop {
                let next = {
                    let mut queue = self
                        .delivery
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    queue.pop_front()
                };
                match next {
                    Some(deferred) => deferred.deliver(),
                    None => break,
                }
            }
            self.draining.store(false, Ordering::Release);
            let empty = self
                .delivery
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty();
            if empty {
                return;
            }
        }
    }

    /// Drive one input with globally serialized delivery.
    pub(crate) fn drive(&self, input: Input) {
        {
            let mut shared = self.lock();
            let deferred = shared.drive(input);
            self.enqueue(deferred);
        }
        self.drain_delivery();
    }

    /// The current connection generation (0 before the first connect; 1 while
    /// Ready — DirectClient never reconnects).
    pub(crate) fn generation(&self) -> u64 {
        self.lock().core.generation()
    }

    /// The current lifecycle state.
    pub(crate) fn state(&self) -> State {
        self.lock().core.state()
    }

    /// Broadcast a local push-overflow marker so the reconciler re-baselines
    /// (`ResyncReason::LocalOverflow`). The bus is cloned under the lock and the
    /// broadcast runs outside it (broadcast takes the subscriber lock and may
    /// wake re-entrant consumers — holding `Shared` across it would be an ABBA
    /// inversion, exactly as `subscribe` guards against).
    pub(crate) fn emit_lagged(&self, dropped: u64) {
        let bus = { self.lock().bus.clone() };
        bus.broadcast(ClientEvent::Lagged {
            room_id: None,
            dropped,
        });
    }
}

/// The cloneable erased [`ClientBackend`] the shared [`ClientHandle`] wraps.
pub(crate) struct DirectBackend {
    runtime: Arc<DirectRuntime>,
    /// Set `true` once the engine is fully torn down, so `stop`'s future resolves
    /// only after teardown (AC "Stop … awaits teardown").
    torn_down: watch::Receiver<bool>,
}

impl DirectBackend {
    pub(crate) fn new(runtime: Arc<DirectRuntime>, torn_down: watch::Receiver<bool>) -> Self {
        Self { runtime, torn_down }
    }
}

impl ClientBackend for DirectBackend {
    fn dispatch(&self, call: ErasedCall) -> BoxFuture<'static, Result<RawJson, CallError>> {
        let (receiver, call_id) = {
            let mut shared = self.runtime.lock();
            let call_id = shared.core.alloc_call_id();
            let (sender, receiver) = oneshot::channel();
            // Insert before stepping so an immediate refusal (QueueFull,
            // post-stop) settles into a live channel.
            shared.senders.insert(call_id, sender);
            let deferred = shared.drive(Input::Dispatch { call_id, call });
            self.runtime.enqueue(deferred);
            (receiver, call_id)
        };
        self.runtime.drain_delivery();
        Box::pin(DispatchFuture {
            receiver,
            cancel: Some((Arc::downgrade(&self.runtime), call_id)),
        })
    }

    fn subscribe(&self) -> EventSubscription {
        // Clone the bus under the lock, register outside it (ABBA guard).
        let bus = { self.runtime.lock().bus.clone() };
        bus.subscribe()
    }

    fn state(&self) -> State {
        self.runtime.state()
    }

    fn start(&self) {
        self.runtime.drive(Input::Start);
    }

    fn stop(&self) -> BoxFuture<'static, ()> {
        // Eager initiation: the core settles every accepted call, closes the
        // bus, and emits `CancelDial` synchronously; the driver loop turns
        // `CancelDial` into engine teardown. The returned future resolves only
        // once the driver signals teardown complete.
        self.runtime.drive(Input::Stop);
        let mut torn_down = self.torn_down.clone();
        Box::pin(async move {
            while !*torn_down.borrow() {
                if torn_down.changed().await.is_err() {
                    break;
                }
            }
        })
    }
}

/// The reply future returned by [`DirectBackend::dispatch`]. Dropping it before
/// it resolves feeds `Input::Cancel` to the core, so a caller abandoning the
/// future stops local delivery (and tombstones a sent call) without fabricating
/// remote cancellation — the same cancel-on-drop the kernel backend provides.
struct DispatchFuture {
    receiver: oneshot::Receiver<Result<RawJson, CallError>>,
    cancel: Option<(Weak<DirectRuntime>, CallId)>,
}

impl std::future::Future for DispatchFuture {
    type Output = Result<RawJson, CallError>;

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let this = self.get_mut();
        match std::pin::Pin::new(&mut this.receiver).poll(cx) {
            std::task::Poll::Ready(Ok(result)) => {
                this.cancel = None;
                std::task::Poll::Ready(result)
            }
            std::task::Poll::Ready(Err(_canceled)) => {
                this.cancel = None;
                std::task::Poll::Ready(Err(CallError::Local(LocalError::Backend)))
            }
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }
}

impl Drop for DispatchFuture {
    fn drop(&mut self) {
        if let Some((weak, call_id)) = self.cancel.take() {
            if let Some(runtime) = weak.upgrade() {
                runtime.drive(Input::Cancel(call_id));
            }
        }
    }
}
