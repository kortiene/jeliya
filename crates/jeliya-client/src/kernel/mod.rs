//! The bounded, lifecycle-aware client kernel (#168): the transport-independent
//! runtime that sits *behind* the `ClientBackend` trait #167 defined, giving
//! the seam its real machinery.
//!
//! The kernel is a **sans-IO state machine** ([`core`]) wrapped by a thin async
//! shell ([`KernelBackend`]) and driven by a transport the adapters implement
//! ([`transport`]). All correctness — hard request bounds, deterministic
//! settlement, correlation/operation ids, deadlines, cancellation at every
//! phase, generation fencing, capped jittered backoff, honest post-send
//! uncertainty, and total stop — lives in the pure core, so every fault is a
//! deterministic sequence of `step` calls, identical on wasm and native.
//!
//! The only new **public** surface is [`KernelLimits`] and [`KernelConfig`]
//! (re-exported from the crate root); everything else stays internal, exactly
//! as the seam's erasure does. The deterministic in-memory transport and its
//! [`KernelController`] ship behind the `test-transport` feature so the parity
//! suite (#175) and the kernel's own fault suite can drive the kernel against
//! the same controller the four real adapters are diffed against.

// The kernel's internal machinery is consumed by the four transport adapters
// (#171/#172/#173) and the fault suite. Until those land in-tree, some items
// are legitimately built-upon only under the `test-transport` feature or by the
// kernel's own unit tests; rather than scatter per-item attributes, allow dead
// code within the kernel module (mirroring the crate-level allowance the seam
// already carries for its pre-adapter backend plumbing). Genuine dead-code
// linting returns for the whole kernel the moment an adapter consumes it.
#![allow(dead_code)]

pub(crate) mod admission;
pub(crate) mod backoff;
pub(crate) mod core;
pub(crate) mod diag;
pub(crate) mod ids;
pub(crate) mod inflight;
pub(crate) mod replay;
pub(crate) mod timing;
pub(crate) mod transport;

pub use timing::TickDelta;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use futures::channel::oneshot;
use futures::future::BoxFuture;

use crate::backend::{ClientBackend, ErasedCall, RawJson};
use crate::error::{CallError, LocalError};
use crate::event::{EventBus, EventSubscription, State};
use crate::kernel::core::{Action, Core, Input};
use crate::kernel::inflight::CallId;
use crate::kernel::timing::{Tick, TimerId};
use crate::kernel::transport::WireFrame;

/// The kernel's hard bounds. Every field is explicit; none defaults silently to
/// "unbounded". The adapter/host chooses them, with the documented
/// [`Default`] as a conservative starting point.
///
/// The three request bounds are the heart of the issue (§K2): `queue_depth` and
/// `outbound_bytes` are admission-time refusals surfaced as
/// [`CallError::QueueFull`], while `in_flight` is a throttle that never refuses.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct KernelLimits {
    /// Max admitted-but-unsent calls before
    /// `QueueFull { resource: "queue_depth" }`.
    pub queue_depth: u32,
    /// Max queued outbound payload bytes before
    /// `QueueFull { resource: "outbound_bytes" }`. A message-count-only bound is
    /// insufficient; the queue is byte-bounded (protocol §Bounded resources).
    pub outbound_bytes: u64,
    /// Max sent-and-awaiting-reply calls. A **throttle**, not a rejection: when
    /// in-flight is at the cap, further admitted calls stay queued and are
    /// released as replies land. `in_flight_count <= in_flight` always holds.
    pub in_flight: u32,
    /// Absolute per-call deadline, as a logical-time delta. A timeout settles
    /// [`CallError::Timeout`] (`Execution::Unknown` — it may still land).
    pub default_call_deadline: TickDelta,
    /// Full-jitter reconnect backoff base delay.
    pub backoff_base: TickDelta,
    /// Full-jitter reconnect backoff cap (the ceiling of the exponential
    /// window).
    pub backoff_cap: TickDelta,
    /// The reconnect attempt ceiling. After this many failed dials the kernel
    /// transitions to `Failed` and settles every outstanding call honestly,
    /// rather than spinning forever (§K10).
    pub max_reconnect_attempts: u32,
}

impl Default for KernelLimits {
    /// Conservative defaults. Time deltas are in **logical ticks**; a host maps
    /// one tick to a real duration when it drives the kernel (for example, one
    /// tick per millisecond makes `default_call_deadline` a 30-second budget).
    fn default() -> Self {
        Self {
            queue_depth: 1024,
            outbound_bytes: 16 * 1024 * 1024,
            in_flight: 256,
            default_call_deadline: TickDelta(30_000),
            backoff_base: TickDelta(100),
            backoff_cap: TickDelta(30_000),
            max_reconnect_attempts: 8,
        }
    }
}

/// Kernel construction inputs that are not limits.
///
/// [`Default`] pairs the conservative [`KernelLimits::default`] with a zero
/// jitter seed (deterministic and reproducible; a host overrides it with
/// platform entropy at construction).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct KernelConfig {
    /// The hard bounds.
    pub limits: KernelLimits,
    /// Seeds the deterministic in-core jitter PRNG (§K10). Tests fix it; a host
    /// seeds it from platform entropy **at construction** — passed in, never a
    /// syscall inside the library. It is not a credential; it only decorrelates
    /// reconnect storms.
    pub jitter_seed: u64,
}

/// The driver-owned mutable state the async shell and the controller share: the
/// sans-IO core plus the real resources the core's `Action`s manipulate (the
/// event bus, the reply senders, the virtual clock, the armed timers, and the
/// in-memory transport buffers).
struct Shared {
    core: Core,
    bus: Arc<EventBus>,
    /// The virtual clock — the sole source of time, advanced only by the
    /// controller. Real adapters replace this with the platform clock.
    now: Tick,
    /// The per-call reply senders, keyed by [`CallId`]. Owning the senders here
    /// (not in the core) keeps the sans-IO core free of I/O while still
    /// guaranteeing exactly-once settlement: a `Settle` action removes the
    /// sender, so a second settle for the same id is a no-op.
    senders: HashMap<CallId, oneshot::Sender<Result<RawJson, CallError>>>,
    /// Armed timers: id → fire time. The controller fires the due ones as it
    /// advances the clock.
    timers: HashMap<TimerId, Tick>,
    /// Frames the core asked the transport to send, retained so the controller
    /// can learn the wire ids it must reply to.
    outbound: Vec<WireFrame>,
    /// Whether a dial/backoff is in progress.
    dialing: bool,
}

impl Shared {
    /// Perform one batch of core actions against the real resources.
    fn apply(&mut self, actions: Vec<Action>) {
        for action in actions {
            self.apply_one(action);
        }
    }

    fn apply_one(&mut self, action: Action) {
        match action {
            Action::Send(frame) => self.outbound.push(frame),
            Action::ArmTimer { id, at } => {
                self.timers.insert(id, at);
            }
            Action::CancelTimer(id) => {
                self.timers.remove(&id);
            }
            Action::Dial => self.dialing = true,
            Action::CancelDial => self.dialing = false,
            Action::Settle(call_id, result) => {
                if let Some(sender) = self.senders.remove(&call_id) {
                    // The receiver may already be dropped (a cancelled caller);
                    // sending into a dropped receiver is a harmless no-op.
                    let _ = sender.send(result);
                }
            }
            Action::DropSender(call_id) => {
                // The caller abandoned the future (queued-cancel or tombstone
                // creation); prune its reply sender without settling so the map
                // stays bounded (§K12). Its receiver is already gone, so this is
                // a pure removal; a second drop (e.g. a reclaim after a timeout
                // already settled) finds nothing and is a harmless no-op.
                self.senders.remove(&call_id);
            }
            Action::Emit(event) => self.bus.broadcast(event),
            Action::CloseBus => self.bus.close(),
        }
    }

    /// Step the core and immediately apply the resulting actions.
    fn drive(&mut self, input: Input) {
        let now = self.now;
        let actions = self.core.step(input, now);
        self.apply(actions);
    }
}

/// The kernel-backed [`ClientBackend`]: the async shell binding the sans-IO
/// core to the event bus, per-call reply oneshots, and the driver's resources.
/// It never spawns and never reads a wall clock — the driver owns both (§3).
pub(crate) struct KernelBackend {
    shared: Arc<Mutex<Shared>>,
}

impl KernelBackend {
    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().expect("kernel shared state poisoned")
    }
}

impl ClientBackend for KernelBackend {
    fn dispatch(&self, call: ErasedCall) -> BoxFuture<'static, Result<RawJson, CallError>> {
        let mut shared = self.lock();
        let call_id = shared.core.alloc_call_id();
        let (sender, receiver) = oneshot::channel();
        // Insert the sender before stepping so an immediate refusal (QueueFull,
        // post-stop) settles into a live channel.
        shared.senders.insert(call_id, sender);
        shared.drive(Input::Dispatch { call_id, call });
        Box::pin(DispatchFuture {
            receiver,
            cancel: Some((Arc::downgrade(&self.shared), call_id)),
        })
    }

    fn subscribe(&self) -> EventSubscription {
        self.lock().bus.subscribe()
    }

    fn state(&self) -> State {
        self.lock().core.state()
    }

    fn start(&self) {
        self.lock().drive(Input::Start);
    }

    fn stop(&self) -> BoxFuture<'static, ()> {
        // Initiation is eager (the refusal boundary is set now); the core
        // settles synchronously, so the returned future is already resolved.
        self.lock().drive(Input::Stop);
        Box::pin(async {})
    }
}

/// The reply future returned by [`KernelBackend::dispatch`]. Dropping it before
/// it resolves feeds an `Input::Cancel` to the core, so a caller abandoning the
/// future stops local delivery (and tombstones a sent call) without fabricating
/// remote cancellation (§K9).
struct DispatchFuture {
    receiver: oneshot::Receiver<Result<RawJson, CallError>>,
    cancel: Option<(Weak<Mutex<Shared>>, CallId)>,
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
                // Settled: disarm the cancel-on-drop hook.
                this.cancel = None;
                std::task::Poll::Ready(result)
            }
            std::task::Poll::Ready(Err(_canceled)) => {
                // The sender was dropped without settling — a backend-internal
                // invariant miss, classified Unknown near the send boundary.
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
            if let Some(shared) = weak.upgrade() {
                if let Ok(mut shared) = shared.lock() {
                    shared.drive(Input::Cancel(call_id));
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Deterministic in-memory driver + controller (the Verification substrate).
// ---------------------------------------------------------------------------

#[cfg(feature = "test-transport")]
pub use in_memory::{KernelController, SentFrame};

#[cfg(feature = "test-transport")]
mod in_memory {
    use super::*;
    use jeliya_api::{ApiError, RequestId};

    use crate::event::ClientEvent;
    use crate::handle::ClientHandle;
    use crate::kernel::transport::{Inbound, WireReply};

    impl ClientHandle {
        /// Build a kernel-backed handle over the deterministic in-memory driver,
        /// returning the handle plus a [`KernelController`] the test drives.
        ///
        /// This is the reference the four real adapters (#171/#172/#173) are
        /// diffed against (#175): no wall clock, no timers, no RNG syscall, and
        /// no scheduling dependence — the same guarantees the #167 mock gives
        /// the seam, now for the kernel.
        pub fn with_kernel(config: KernelConfig) -> (ClientHandle, KernelController) {
            let shared = Arc::new(Mutex::new(Shared {
                core: Core::new(config.limits, config.jitter_seed),
                bus: Arc::new(EventBus::new()),
                now: Tick::ZERO,
                senders: HashMap::new(),
                timers: HashMap::new(),
                outbound: Vec::new(),
                dialing: false,
            }));
            let handle = ClientHandle::from_backend(Arc::new(KernelBackend {
                shared: Arc::clone(&shared),
            }));
            (handle, KernelController { shared })
        }
    }

    /// A redaction-safe view of one sent frame: the controller learns the wire
    /// id it must reply to and the operation name, never the payload or `op_id`.
    #[derive(Clone, Copy, Debug)]
    pub struct SentFrame {
        /// The wire correlation id.
        pub id: u64,
        /// The operation's wire name.
        pub op: &'static str,
    }

    /// The test-driven controller half of the kernel — the deterministic
    /// in-memory driver. Every method is clock-free and scheduling-independent;
    /// nothing here observes real time.
    pub struct KernelController {
        shared: Arc<Mutex<Shared>>,
    }

    impl KernelController {
        fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
            self.shared.lock().expect("kernel shared state poisoned")
        }

        /// The current connection generation (0 before the first `connect`).
        pub fn generation(&self) -> u64 {
            self.lock().core.generation()
        }

        /// Whether a dial/backoff is in progress: an active dial attempt, or
        /// the armed reconnect-backoff timer between attempts — so an actively
        /// retrying client is distinguishable from one with no retry work.
        pub fn is_dialing(&self) -> bool {
            let shared = self.lock();
            shared.dialing || shared.core.backoff_armed()
        }

        /// Take the frames the kernel has asked the transport to send since the
        /// last drain, as redaction-safe [`SentFrame`] views.
        pub fn take_outbound(&self) -> Vec<SentFrame> {
            self.lock()
                .outbound
                .drain(..)
                .map(|frame| SentFrame {
                    id: frame.id.get(),
                    op: frame.op,
                })
                .collect()
        }

        /// Complete a dial: a live connection is established and passes the
        /// generation gate. Returns the fresh generation.
        pub fn connect(&self) -> u64 {
            let mut shared = self.lock();
            shared.dialing = false;
            shared.drive(Input::Connected);
            shared.core.generation()
        }

        /// Report a recoverable connection loss (a dropped connection or a
        /// send/close race).
        pub fn interrupt(&self) {
            self.lock().drive(Input::Interrupted);
        }

        /// Report a terminal generation-gate refusal (no auto-retry, §K7).
        pub fn gate_refused(&self) {
            self.lock().drive(Input::GateRefused);
        }

        /// Deliver a success reply for `wire_id` on the current generation.
        pub fn deliver_reply(&self, wire_id: u64, out_json: impl Into<String>) {
            let generation = self.generation();
            self.deliver_reply_at_generation(wire_id, out_json, generation);
        }

        /// Deliver a success reply tagged with an explicit generation — used to
        /// prove generation fencing drops a stale reply.
        pub fn deliver_reply_at_generation(
            &self,
            wire_id: u64,
            out_json: impl Into<String>,
            generation: u64,
        ) {
            let id = RequestId::new(wire_id).expect("wire id within range");
            self.lock().drive(Input::Inbound(Inbound::Reply {
                generation,
                id,
                result: WireReply::Ok(RawJson::from_string(out_json.into())),
            }));
        }

        /// Deliver a typed daemon error for `wire_id` on the current generation.
        pub fn deliver_error(&self, wire_id: u64, error: ApiError) {
            let generation = self.generation();
            let id = RequestId::new(wire_id).expect("wire id within range");
            self.lock().drive(Input::Inbound(Inbound::Reply {
                generation,
                id,
                result: WireReply::Err(error),
            }));
        }

        /// Deliver a live push on the current generation.
        pub fn deliver_push(&self, event: ClientEvent) {
            let generation = self.generation();
            self.deliver_push_at_generation(event, generation);
        }

        /// Deliver a push tagged with an explicit generation — used to prove a
        /// stale-generation push is fenced.
        pub fn deliver_push_at_generation(&self, event: ClientEvent, generation: u64) {
            self.lock()
                .drive(Input::Inbound(Inbound::Push { generation, event }));
        }

        /// Deliver a frame that parses to no envelope: it must strand nothing.
        pub fn deliver_malformed(&self) {
            self.lock().drive(Input::Inbound(Inbound::Malformed));
        }

        /// Advance the virtual clock by `ticks`, firing every timer now due (in
        /// ascending fire-time order).
        pub fn advance(&self, ticks: u64) {
            let mut shared = self.lock();
            shared.now.advance(TickDelta(ticks));
            loop {
                let now = shared.now;
                let due = shared
                    .timers
                    .iter()
                    .filter(|(_, at)| **at <= now)
                    // Tie-break equal fire times by timer id so the reference
                    // driver is deterministic when deadlines coincide (§K13).
                    .min_by_key(|(id, at)| (**at, id.0))
                    .map(|(id, _)| *id);
                let Some(id) = due else {
                    break;
                };
                shared.timers.remove(&id);
                shared.drive(Input::TimerFired(id));
            }
        }

        /// The number of outstanding ledger entries — asserted `0` after stop
        /// (no unbounded task or map survives, §K11/§K12).
        pub fn outstanding(&self) -> usize {
            self.lock().core.ledger_len()
        }

        /// The number of sent, non-cancelled calls — asserted `<= in_flight`.
        pub fn in_flight(&self) -> u32 {
            self.lock().core.in_flight_count()
        }

        /// The number of admitted-but-unsent calls.
        pub fn queued(&self) -> usize {
            self.lock().core.queued_len()
        }

        /// The number of calls held for replay across a reconnect.
        pub fn replay_held(&self) -> usize {
            self.lock().core.replay_hold_len()
        }

        /// The number of armed driver timers.
        pub fn armed_timers(&self) -> usize {
            self.lock().timers.len()
        }

        /// The number of live per-call reply senders held by the driver. On the
        /// dropped-future path this must fall as calls are cancelled/tombstoned,
        /// never growing without bound (§K12, AC-7); no `Settle` fires for a
        /// cancelled call, so the map is pruned by `Action::DropSender` instead.
        pub fn senders(&self) -> usize {
            self.lock().senders.len()
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::handle::Dedup;
        use crate::CallError;
        use futures::executor::block_on;
        use jeliya_api::{RoomList, RoomListOut};

        fn ready() -> (ClientHandle, KernelController) {
            let (handle, controller) = ClientHandle::with_kernel(KernelConfig::default());
            handle.start();
            controller.connect();
            assert_eq!(handle.state(), State::Ready);
            (handle, controller)
        }

        /// End-to-end: a call dispatched through the handle is sent on the wire,
        /// and its reply — delivered by the controller — resolves the typed
        /// future exactly once.
        #[test]
        fn dispatch_and_reply_round_trip() {
            let (handle, controller) = ready();
            block_on(async {
                let fut = handle.call::<RoomList>(RoomList {}, Dedup::None);
                let sent = controller.take_outbound();
                assert_eq!(sent.len(), 1, "the call reached the wire");
                controller.deliver_reply(sent[0].id, "{\"rooms\":[]}");
                let out: RoomListOut = fut.await.expect("reply resolves the future");
                assert!(out.rooms.is_empty());
            });
            assert_eq!(controller.outstanding(), 0, "the call is fully settled");
        }

        /// Dropping a sent call's future tombstones it locally — no remote
        /// cancel — and a subsequent real reply is absorbed, stranding nothing
        /// (§K9).
        #[test]
        fn dropping_a_sent_future_tombstones_the_call() {
            let (handle, controller) = ready();
            let fut = handle.call::<RoomList>(RoomList {}, Dedup::None);
            let sent = controller.take_outbound();
            assert_eq!(sent.len(), 1);
            assert_eq!(controller.in_flight(), 1);

            // Caller abandons the future: the in-flight slot frees but the id
            // stays reserved as a tombstone so a late reply is absorbed.
            drop(fut);
            assert_eq!(controller.in_flight(), 0);
            assert_eq!(controller.outstanding(), 1, "the tombstone reserves the id");

            controller.deliver_reply(sent[0].id, "{\"rooms\":[]}");
            assert_eq!(
                controller.outstanding(),
                0,
                "the late reply is absorbed and the tombstone reclaimed"
            );
        }

        /// AC-7/§K12: a client that routinely drops call futures must not grow
        /// the driver's per-call `senders` map. No `Settle` fires for a
        /// cancelled call, so both the queued-cancel and the sent-tombstone
        /// paths must prune the sender themselves — the leak this guards.
        #[test]
        fn cancelling_calls_prunes_reply_senders() {
            // in_flight=1 forces the second call to queue behind the first, so a
            // single fixture exercises both the sent-tombstone and the
            // queued-cancel cancel paths.
            let (handle, controller) = ClientHandle::with_kernel(KernelConfig {
                limits: KernelLimits {
                    in_flight: 1,
                    ..KernelLimits::default()
                },
                jitter_seed: 1,
            });
            handle.start();
            controller.connect();
            assert_eq!(handle.state(), State::Ready);

            let sent = handle.call::<RoomList>(RoomList {}, Dedup::None);
            let queued = handle.call::<RoomList>(RoomList {}, Dedup::None);
            let frames = controller.take_outbound();
            assert_eq!(frames.len(), 1, "in_flight=1 sends only the first call");
            assert_eq!(controller.senders(), 2, "both calls hold a reply sender");
            assert_eq!(controller.in_flight(), 1);
            assert_eq!(controller.queued(), 1);

            // Drop the queued future: the queued-cancel path removes the entry
            // outright and must prune its sender.
            drop(queued);
            assert_eq!(
                controller.senders(),
                1,
                "the queued call's sender is pruned on cancel"
            );
            assert_eq!(controller.queued(), 0);

            // Drop the sent future: the tombstone path keeps the ledger id
            // reserved but must still prune the sender.
            drop(sent);
            assert_eq!(
                controller.senders(),
                0,
                "the tombstoned call's sender is pruned even though the id lives on"
            );
            assert_eq!(
                controller.outstanding(),
                1,
                "only the tombstone reserves the id — no sender behind it"
            );

            // The late reply reclaims the tombstone; the map never regrows.
            controller.deliver_reply(frames[0].id, "{\"rooms\":[]}");
            assert_eq!(controller.outstanding(), 0, "tombstone reclaimed");
            assert_eq!(controller.senders(), 0, "no sender leaked across the churn");
        }

        /// `QueueFull` surfaces through the typed handle rather than being
        /// absorbed (§K2).
        #[test]
        fn queue_full_surfaces_through_the_handle() {
            let (handle, controller) = ClientHandle::with_kernel(KernelConfig {
                limits: KernelLimits {
                    queue_depth: 1,
                    ..KernelLimits::default()
                },
                jitter_seed: 1,
            });
            // Idle: both calls queue; the second overflows the depth-1 queue.
            let _first = handle.call::<RoomList>(RoomList {}, Dedup::None);
            let second = handle.call::<RoomList>(RoomList {}, Dedup::None);
            let err = block_on(second).expect_err("second call is refused");
            assert!(
                matches!(
                    err,
                    CallError::QueueFull {
                        resource: "queue_depth",
                        limit: 1
                    }
                ),
                "{err:?}"
            );
            // Keep the controller alive for the whole test.
            drop(controller);
        }

        /// Stop settles a pending call and closes the event stream; the state
        /// lands on `Stopped` (§K11).
        #[test]
        fn stop_settles_and_closes() {
            let (handle, controller) = ready();
            let fut = handle.call::<RoomList>(RoomList {}, Dedup::None);
            let _sent = controller.take_outbound();
            block_on(handle.stop());
            let err = block_on(fut).expect_err("a pending call is settled by stop");
            assert!(matches!(err, CallError::Cancelled { .. }), "{err:?}");
            assert_eq!(handle.state(), State::Stopped);
            assert_eq!(controller.outstanding(), 0);
        }
    }
}
