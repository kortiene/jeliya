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
pub(crate) mod streaming;
pub(crate) mod timing;
pub(crate) mod transport;

pub use timing::TickDelta;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use futures::channel::oneshot;
use futures::future::BoxFuture;

use crate::backend::{ClientBackend, ErasedCall, RawJson};
use crate::error::{CallError, LocalError};
use crate::event::{ClientEvent, EventBus, EventSubscription, State};
use crate::kernel::core::{Action, Core, Input};
use crate::kernel::inflight::CallId;
use crate::kernel::timing::{Tick, TimerId};

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

/// The served transfer bounds a stream is driven under (protocol §Served limits
/// / §Byte-stream framing, §6). A host reads these from the daemon's served
/// limits object and passes them in; none defaults silently to "unbounded".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct StreamLimits {
    /// Fixed connect allowance in a stream's absolute budget
    /// (`transfer_connect_allowance_ms`), as a [`TickDelta`].
    pub transfer_connect_allowance: TickDelta,
    /// The floor throughput the size-aware absolute budget is computed from
    /// (`transfer_floor_bits_per_second`). Zero is an invalid served
    /// configuration; the size term degrades to zero (the stall timer remains
    /// the effective bound) and construction debug-asserts against it.
    pub transfer_floor_bits_per_second: u64,
    /// Ticks per second — maps the served ms / bit-rate limits into the kernel's
    /// abstract tick unit. Default `1000` (1 tick = 1 ms), matching
    /// [`KernelLimits`].
    pub budget_ticks_per_second: u64,
    /// Zero-accepted-progress window before an honest stall failure
    /// (`transfer_stall_ms`), as a [`TickDelta`].
    pub transfer_stall: TickDelta,
    /// The byte-bounded per-stream read-ahead (producer) / quarantine (receiver)
    /// window — the client's cumulative-credit ceiling. Bounds memory
    /// independent of file size (§S5).
    pub stream_window_bytes: u64,
    /// The maximum number of concurrent streams the client will drive; a stream
    /// past it is refused before OPEN (bounds the stream table, §S11).
    pub max_concurrent_streams: u32,
}

impl Default for StreamLimits {
    /// Conservative, documented defaults: a 1 MiB window, a 64 kbit/s floor, a
    /// 30-second connect allowance and stall window (in the default 1 tick = 1
    /// ms convention), and up to 64 concurrent streams.
    fn default() -> Self {
        Self {
            transfer_connect_allowance: TickDelta::from_ticks(30_000),
            transfer_floor_bits_per_second: 64_000,
            budget_ticks_per_second: 1_000,
            transfer_stall: TickDelta::from_ticks(30_000),
            stream_window_bytes: 1024 * 1024,
            max_concurrent_streams: 64,
        }
    }
}

impl StreamLimits {
    /// Whether the served configuration is representable: a non-zero floor and
    /// ticks-per-second (§6). An invalid configuration does not panic in
    /// release — the size-aware deadline term degrades to zero and the stall
    /// timer remains the effective bound — but it debug-asserts so a misconfigured
    /// host is caught in tests.
    pub(crate) fn is_representable(&self) -> bool {
        self.transfer_floor_bits_per_second != 0 && self.budget_ticks_per_second != 0
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
    /// Whether the driver certifies **continuity of the daemon's complete
    /// dedup scope** across the replay window: a stable session principal (a
    /// stable `client_id`) AND the same daemon incarnation. The dedup ledger
    /// is keyed `(principal, op_id)` and is deliberately in-memory — a
    /// daemon restart empties it while the storage-generation gate still
    /// passes, so a replay against the new incarnation re-executes the
    /// mutation even under a stable `client_id`. `DirectClient` (in-process:
    /// daemon restart = client restart) can certify; a socket adapter can
    /// only do so once the protocol's `hello` carries a daemon incarnation
    /// identity to fence on (tracked as a protocol follow-up). **Defaults to
    /// `false` (replay disabled)**: an adapter must opt in, never the
    /// reverse (§K5).
    pub stable_principal: bool,
    /// The served byte-stream bounds (§6). Validated at construction (non-zero
    /// floor and ticks-per-second); an invalid served configuration degrades the
    /// size-aware deadline term to zero and debug-asserts.
    pub streams: StreamLimits,
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
    /// can learn the wire ids it must reply to. **Bounded** (K12) twice over:
    /// an undrained log evicts its oldest entry and counts the loss, and each
    /// entry is the redaction-safe METADATA view only — retaining the full
    /// `WireFrame` would keep every near-limit payload alive (up to the cap ×
    /// the 16 MiB byte budget) after admission already released it, since
    /// `take_outbound` never exposes more than the id and operation anyway.
    outbound: std::collections::VecDeque<SentFrame>,
    /// Frames evicted from the bounded outbound log without being observed.
    outbound_overflow: u64,
    /// Whether a dial/backoff is in progress.
    dialing: bool,
    /// Scripted send/close race (§K14 `fail_send`): when set, the next
    /// `Action::Send` drops its frame and records `send_failed` instead of
    /// reaching the transport.
    fail_next_send: bool,
    /// A send observed the broken transport during the current apply batch;
    /// `drive` surfaces it as `Input::Interrupted` after the batch.
    send_failed: bool,
    /// The client-authored byte-stream records the core asked the driver to send
    /// since the last drain, as redaction-safe [`SentRecord`] views (§S3/§S12).
    /// Bounded FIFO exactly like `outbound` (K12): an undrained log evicts its
    /// oldest entry and counts the loss.
    outbound_records: std::collections::VecDeque<SentRecord>,
    /// Records evicted from the bounded record log without being observed.
    outbound_records_overflow: u64,
    /// The deterministic media state per producer stream: the reference driver's
    /// `i mod 251` byte source, keyed by [`CallId`] (§S3). A receiver stream has
    /// no source and never appears here; its sink accepts every delivered range.
    stream_media: HashMap<CallId, StreamMedia>,
    /// Media inputs the driver must feed back to the core after the current apply
    /// batch — a `ProduceData`/`WriteSink` action is fulfilled synchronously by
    /// the deterministic source/sink, exactly as a real adapter's media loop
    /// would, and reported here (drained by `drive`, mirroring `send_failed`).
    pending_media: std::collections::VecDeque<Input>,
}

/// The deterministic producer source state the reference driver owns (§S3): the
/// total the daemon's OPEN admitted and how far the `i mod 251` source has been
/// read. Byte values are never stored — only offsets — so the driver is as
/// byte-bounded as the core.
struct StreamMedia {
    /// The reply-correlation id, so an outbound DATA observation is keyed by the
    /// same wire id the test sees.
    wire_id: u64,
    /// The source's total byte count (from the scripted OPEN).
    total: u64,
    /// How far the source has been read and framed.
    produced: u64,
}

/// Work that must happen **after** the `Shared` mutex is released: settling a
/// reply future or broadcasting an event invokes arbitrary wakers, and a
/// waker backed by an inline executor may re-enter the handle — `state()`,
/// another dispatch, a future drop — re-acquiring the same mutex and
/// deadlocking. Every `drive` returns one of these; the caller delivers it
/// with the lock already dropped.
/// One wake-producing unit of deferred work, in the core's action order —
/// partitioning by type would reorder cross-surface effects (e.g. a
/// settlement waker running before the `Stopping` transition it follows in
/// `on_stop`'s documented sequence).
enum DeferredWake {
    Settle(
        oneshot::Sender<Result<RawJson, CallError>>,
        Result<RawJson, CallError>,
    ),
    Emit(ClientEvent),
    CloseBus,
}

#[must_use]
struct Deferred {
    bus: Arc<EventBus>,
    work: Vec<DeferredWake>,
    /// Fired after this batch delivers — a synchronous caller (`stop`) that
    /// found another thread draining awaits it, so "already draining" is
    /// never mistaken for "already delivered".
    done: Option<oneshot::Sender<()>>,
}

impl Deferred {
    fn is_empty(&self) -> bool {
        self.work.is_empty() && self.done.is_none()
    }

    fn deliver(self) {
        for wake in self.work {
            match wake {
                DeferredWake::Settle(sender, result) => {
                    // The receiver may already be dropped; a no-op send is
                    // harmless.
                    let _ = sender.send(result);
                }
                DeferredWake::Emit(event) => self.bus.broadcast(event),
                DeferredWake::CloseBus => self.bus.close(),
            }
        }
        if let Some(done) = self.done {
            let _ = done.send(());
        }
    }
}

impl Shared {
    /// Perform one batch of core actions against the real resources.
    fn apply(&mut self, actions: Vec<Action>, deferred: &mut Deferred) {
        for action in actions {
            self.apply_one(action, deferred);
        }
    }

    fn apply_one(&mut self, action: Action, deferred: &mut Deferred) {
        match action {
            Action::Send(frame) => {
                if self.fail_next_send || self.send_failed {
                    // The scripted send/close race (§K14): the pipe breaks at
                    // the first write, and a broken transport fails every
                    // later write in the same batch too — no frame after the
                    // failure reaches the wire. The loss surfaces to the core
                    // after this apply batch.
                    self.fail_next_send = false;
                    self.send_failed = true;
                } else {
                    if self.outbound.len() >= OUTBOUND_LOG_CAP {
                        self.outbound.pop_front();
                        self.outbound_overflow = self.outbound_overflow.saturating_add(1);
                    }
                    // Redact at APPEND time: the log keeps only what
                    // take_outbound exposes, so the payload and op_id drop
                    // with the frame here instead of living in the log.
                    self.outbound.push_back(SentFrame {
                        id: frame.id.get(),
                        op: frame.op,
                    });
                }
            }
            Action::ArmTimer { id, at } => {
                self.timers.insert(id, at);
            }
            Action::CancelTimer(id) => {
                self.timers.remove(&id);
            }
            Action::Dial { .. } => self.dialing = true,
            Action::CancelDial => self.dialing = false,
            Action::Settle(call_id, result) => {
                if let Some(sender) = self.senders.remove(&call_id) {
                    // Delivered after the Shared lock drops (§K12 wake
                    // hygiene): sending invokes the receiver's waker.
                    deferred.work.push(DeferredWake::Settle(sender, result));
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
            Action::Emit(event) => deferred.work.push(DeferredWake::Emit(event)),
            Action::CloseBus => deferred.work.push(DeferredWake::CloseBus),
            Action::SendRecord(intent) => {
                // The driver frames the record via jeliya-codec at the boundary
                // (real adapters, #171/#172/#173); the reference driver records a
                // redaction-safe observation so tests can assert the control-plane
                // ordering (§S3/§S12). The byte layout is never handled here.
                self.record_outbound(SentRecord::from_intent(&intent));
            }
            Action::ProduceData { call_id, up_to } => {
                // The deterministic source produces exactly the granted bytes
                // (the core bounds `up_to` by credit, window, and total), frames
                // them, sends them, and reports how far it got (§S3). A single
                // DATA observation covers the whole grant; the byte content is the
                // `i mod 251` source and is never stored.
                let framed = self.stream_media.get_mut(&call_id).map(|media| {
                    let offset = media.produced;
                    let sent_through = media.produced.saturating_add(up_to).min(media.total);
                    media.produced = sent_through;
                    (media.wire_id, offset, sent_through, media.total)
                });
                if let Some((wire_id, offset, sent_through, total)) = framed {
                    let len = sent_through.saturating_sub(offset);
                    if len > 0 {
                        self.record_outbound(SentRecord {
                            id: wire_id,
                            kind: "data",
                            a: offset,
                            b: len,
                        });
                    }
                    self.pending_media.push_back(Input::Produced {
                        call_id,
                        sent_through,
                    });
                    if sent_through >= total {
                        self.pending_media
                            .push_back(Input::SourceEnd { call_id, total });
                    }
                }
            }
            Action::WriteSink {
                call_id,
                offset,
                len,
            } => {
                // The deterministic sink accepts every delivered range
                // contiguously and reports its new accepted high-water (§S3).
                self.pending_media.push_back(Input::SinkAccepted {
                    call_id,
                    through: offset.saturating_add(len),
                });
            }
        }
    }

    /// Append one redaction-safe outbound-record observation to the bounded log
    /// (K12): an undrained log evicts its oldest entry and counts the loss.
    fn record_outbound(&mut self, record: SentRecord) {
        if self.outbound_records.len() >= OUTBOUND_LOG_CAP {
            self.outbound_records.pop_front();
            self.outbound_records_overflow = self.outbound_records_overflow.saturating_add(1);
        }
        self.outbound_records.push_back(record);
    }

    /// Step the core, apply the resulting actions, and return the wake work
    /// the caller must deliver **after** releasing the `Shared` lock.
    fn drive(&mut self, input: Input) -> Deferred {
        let mut deferred = Deferred {
            bus: self.bus.clone(),
            work: Vec::new(),
            done: None,
        };
        let now = self.now;
        let actions = self.core.step(input, now);
        self.apply(actions, &mut deferred);
        // A scripted send failure is a connection loss observed at write
        // time: report it to the core exactly as a real driver would (§K14) —
        // tagged with the generation of the transport that broke.
        while std::mem::take(&mut self.send_failed) {
            let now = self.now;
            let generation = self.core.generation();
            let actions = self.core.step(Input::Interrupted { generation }, now);
            self.apply(actions, &mut deferred);
        }
        // The deterministic media seam is driven to completion within the same
        // drive (§S3): a `ProduceData`/`WriteSink` action is fulfilled by the
        // source/sink and reported back to the core, which may grant more or
        // finish — exactly the loop a real adapter's media task runs, but
        // synchronous and deterministic. Bounded: each producer step advances
        // toward its total and each sink step advances the accepted high-water,
        // so the queue drains.
        while let Some(input) = self.pending_media.pop_front() {
            let now = self.now;
            let actions = self.core.step(input, now);
            self.apply(actions, &mut deferred);
        }
        deferred
    }
}

/// The bound on the driver's outbound observation log: frames a test never
/// drains are evicted oldest-first (counted in `outbound_overflow`) so the
/// reference driver honours K12's no-unbounded-collection guarantee even
/// under dispatch/cancel churn with no `take_outbound`.
const OUTBOUND_LOG_CAP: usize = 1024;

/// The shared runtime: the locked driver state plus the serialized delivery
/// queue. The queue's boundedness rests on the async waker contract (`wake`
/// must not block): every batch is drained promptly by the active drainer,
/// and empty batches never enqueue. A waker that blocks indefinitely is a
/// broken executor contract, not a kernel state — the same assumption every
/// wake-delivery queue in the ecosystem makes. Deferred wake batches are enqueued **while still holding the
/// `Shared` lock** — so queue order is exactly drive order, across every
/// cloned handle and thread — and drained by a single drainer after the lock
/// drops, so wakers run outside the lock yet cross-thread delivery can never
/// invert two drives' effects.
struct Runtime {
    shared: Mutex<Shared>,
    delivery: Mutex<std::collections::VecDeque<Deferred>>,
    draining: std::sync::atomic::AtomicBool,
}

impl Runtime {
    /// Enqueue one deferred batch for serialized delivery — called while the
    /// `Shared` lock is held so queue order equals drive order. An empty
    /// batch (no wakes, no completion notification) is skipped on EVERY
    /// path, so no input shape can grow the queue behind an active drainer
    /// without deliverable work.
    fn enqueue(&self, deferred: Deferred) {
        if deferred.is_empty() {
            return;
        }
        self.delivery
            .lock()
            .expect("delivery queue poisoned")
            .push_back(deferred);
    }

    /// Drain the delivery queue as the single drainer. If another thread is
    /// already draining, it will deliver our batch too (it re-checks after
    /// clearing the flag), preserving FIFO order.
    fn drain_delivery(&self) {
        use std::sync::atomic::Ordering;
        loop {
            if self.draining.swap(true, Ordering::AcqRel) {
                return;
            }
            loop {
                let next = {
                    let mut queue = self.delivery.lock().expect("delivery queue poisoned");
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
                .expect("delivery queue poisoned")
                .is_empty();
            if empty {
                return;
            }
        }
    }
}

/// The kernel-backed [`ClientBackend`]: the async shell binding the sans-IO
/// core to the event bus, per-call reply oneshots, and the driver's resources.
/// It never spawns and never reads a wall clock — the driver owns both (§3).
pub(crate) struct KernelBackend {
    runtime: Arc<Runtime>,
}

impl KernelBackend {
    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.runtime
            .shared
            .lock()
            .expect("kernel shared state poisoned")
    }

    /// Drive one input with globally serialized delivery: the deferred batch
    /// is enqueued while the `Shared` lock is still held (queue order ==
    /// drive order across threads), then drained outside every lock.
    fn drive_serialized(&self, input: Input) {
        {
            let mut shared = self.lock();
            let deferred = shared.drive(input);
            self.runtime.enqueue(deferred);
        }
        self.runtime.drain_delivery();
    }
}

impl ClientBackend for KernelBackend {
    fn dispatch(&self, call: ErasedCall) -> BoxFuture<'static, Result<RawJson, CallError>> {
        let (receiver, call_id) = {
            let mut shared = self.lock();
            let call_id = shared.core.alloc_call_id();
            let (sender, receiver) = oneshot::channel();
            // Insert the sender before stepping so an immediate refusal
            // (QueueFull, post-stop) settles into a live channel.
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
        // Clone the bus under the Shared lock, register OUTSIDE it:
        // broadcast holds the subscriber lock while invoking wakers that may
        // re-enter Shared, so holding Shared while taking the subscriber
        // lock would be an ABBA inversion.
        let bus = { self.lock().bus.clone() };
        bus.subscribe()
    }

    fn state(&self) -> State {
        self.lock().core.state()
    }

    fn start(&self) {
        self.drive_serialized(Input::Start);
    }

    fn stop(&self) -> BoxFuture<'static, ()> {
        // Initiation is eager (the refusal boundary is set now); the core
        // settles synchronously, so the returned future is already resolved.
        // Stop's future resolves only after ITS batch delivers: if another
        // thread owns the drainer, "already draining" must not read as
        // "already delivered" — §K11's total-stop ordering includes the
        // settlements and the bus close.
        let (done_tx, done_rx) = oneshot::channel();
        {
            let mut shared = self.lock();
            let mut deferred = shared.drive(Input::Stop);
            deferred.done = Some(done_tx);
            self.runtime.enqueue(deferred);
        }
        self.runtime.drain_delivery();
        Box::pin(async move {
            let _ = done_rx.await;
        })
    }
}

/// The reply future returned by [`KernelBackend::dispatch`]. Dropping it before
/// it resolves feeds an `Input::Cancel` to the core, so a caller abandoning the
/// future stops local delivery (and tombstones a sent call) without fabricating
/// remote cancellation (§K9).
struct DispatchFuture {
    receiver: oneshot::Receiver<Result<RawJson, CallError>>,
    cancel: Option<(Weak<Runtime>, CallId)>,
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
            if let Some(runtime) = weak.upgrade() {
                let drove = match runtime.shared.lock() {
                    Ok(mut shared) => {
                        let deferred = shared.drive(Input::Cancel(call_id));
                        runtime.enqueue(deferred);
                        true
                    }
                    Err(_) => false,
                };
                if drove {
                    runtime.drain_delivery();
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Deterministic in-memory driver + controller (the Verification substrate).
// ---------------------------------------------------------------------------

/// A redaction-safe view of one sent frame: the controller learns the wire
/// id it must reply to and the operation name, never the payload or `op_id`.
/// Defined outside the gated driver module because the outbound log stores
/// exactly this view (redacted at append time); publicly visible only
/// through the feature-gated re-export in `lib.rs` — this module is private.
#[derive(Clone, Copy, Debug)]
pub struct SentFrame {
    /// The wire correlation id.
    pub id: u64,
    /// The operation's wire name.
    pub op: &'static str,
}

/// A redaction-safe view of one client-authored byte-stream control or DATA
/// record the kernel asked the driver to send (§S3/§S12). It carries only the
/// wire id, a kind tag, and two numeric offset/value fields — never a payload
/// byte, name, or digest. The two fields' meaning is per kind:
///
/// | `kind` | `a` | `b` |
/// |---|---|---|
/// | `"data"` | offset | length |
/// | `"credit"` | `accepted_through` | `send_through` |
/// | `"end"` | offset | 0 |
/// | `"abort"` | `high_water` | 0 |
/// | `"ack"` | `high_water` | 0 |
///
/// Publicly visible only through the feature-gated re-export in `lib.rs`.
#[derive(Clone, Copy, Debug)]
pub struct SentRecord {
    /// The stream's reply-correlation id.
    pub id: u64,
    /// The record kind tag (`"data"`, `"credit"`, `"end"`, `"abort"`, `"ack"`).
    pub kind: &'static str,
    /// The first numeric field (see the type-level table).
    pub a: u64,
    /// The second numeric field (see the type-level table).
    pub b: u64,
}

impl SentRecord {
    /// Build the observation for one outbound control-record intent.
    fn from_intent(intent: &crate::kernel::transport::StreamRecordIntent) -> Self {
        use crate::kernel::transport::StreamRecordIntent as I;
        match *intent {
            I::Credit {
                id,
                accepted_through,
                send_through,
                ..
            } => SentRecord {
                id: id.get(),
                kind: "credit",
                a: accepted_through,
                b: send_through,
            },
            I::End { id, offset, .. } => SentRecord {
                id: id.get(),
                kind: "end",
                a: offset,
                b: 0,
            },
            I::Abort { id, high_water, .. } => SentRecord {
                id: id.get(),
                kind: "abort",
                a: high_water,
                b: 0,
            },
            I::Ack { id, high_water, .. } => SentRecord {
                id: id.get(),
                kind: "ack",
                a: high_water,
                b: 0,
            },
        }
    }
}

#[cfg(feature = "test-transport")]
pub use in_memory::KernelController;

#[cfg(feature = "test-transport")]
mod in_memory {
    use super::*;
    use jeliya_api::{ApiError, RequestId};

    use crate::handle::ClientHandle;
    use crate::kernel::transport::{Inbound, StreamAbortReason, StreamRecordMeta, WireReply};

    impl ClientHandle {
        /// Build a kernel-backed handle over the deterministic in-memory driver,
        /// returning the handle plus a [`KernelController`] the test drives.
        ///
        /// This is the reference the four real adapters (#171/#172/#173) are
        /// diffed against (#175): no wall clock, no timers, no RNG syscall, and
        /// no scheduling dependence — the same guarantees the #167 mock gives
        /// the seam, now for the kernel.
        pub fn with_kernel(config: KernelConfig) -> (ClientHandle, KernelController) {
            debug_assert!(
                config.streams.is_representable(),
                "StreamLimits floor and ticks-per-second must be non-zero (§6)"
            );
            let runtime = Arc::new(Runtime {
                shared: Mutex::new(Shared {
                    core: Core::with_stream_limits(
                        config.limits,
                        config.jitter_seed,
                        config.stable_principal,
                        config.streams,
                    ),
                    bus: Arc::new(EventBus::new()),
                    now: Tick::ZERO,
                    senders: HashMap::new(),
                    timers: HashMap::new(),
                    outbound: std::collections::VecDeque::new(),
                    outbound_overflow: 0,
                    dialing: false,
                    fail_next_send: false,
                    send_failed: false,
                    outbound_records: std::collections::VecDeque::new(),
                    outbound_records_overflow: 0,
                    stream_media: HashMap::new(),
                    pending_media: std::collections::VecDeque::new(),
                }),
                delivery: Mutex::new(std::collections::VecDeque::new()),
                draining: std::sync::atomic::AtomicBool::new(false),
            });
            let handle = ClientHandle::from_backend(Arc::new(KernelBackend {
                runtime: Arc::clone(&runtime),
            }));
            (handle, KernelController { runtime })
        }
    }

    /// The test-driven controller half of the kernel — the deterministic
    /// in-memory driver. Every method is clock-free and scheduling-independent;
    /// nothing here observes real time.
    pub struct KernelController {
        runtime: Arc<Runtime>,
    }

    impl KernelController {
        fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
            self.runtime
                .shared
                .lock()
                .expect("kernel shared state poisoned")
        }

        /// Drive one input with globally serialized delivery (see
        /// `KernelBackend::drive_serialized`).
        fn drive_serialized(&self, input: Input) {
            {
                let mut shared = self.lock();
                let deferred = shared.drive(input);
                self.runtime.enqueue(deferred);
            }
            self.runtime.drain_delivery();
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

        /// Script the next `Action::Send` to fail at the transport — the
        /// send/close race §K14 names: the pipe breaks exactly as the kernel
        /// writes. Every frame in the failing batch is dropped and the loss
        /// surfaces to the core as `Interrupted` after the batch applies.
        ///
        /// Everything the failing batch had flushed is classified as sent —
        /// held if replayable, `Disconnected { Unknown }` otherwise. That is
        /// deliberate: a real transport cannot prove that writes queued
        /// behind a failed one never left the host (socket buffering), so
        /// the reference driver mirrors the weakest honest claim rather than
        /// a `DefinitelyNot` no adapter could truthfully make. `Unknown` is
        /// the safe direction — it only ever withholds a provable-negative,
        /// never invites an unguarded retry.
        pub fn fail_send(&self) {
            self.lock().fail_next_send = true;
        }

        /// Take the frames the kernel has asked the transport to send since the
        /// last drain, as redaction-safe [`SentFrame`] views.
        pub fn take_outbound(&self) -> Vec<SentFrame> {
            self.lock().outbound.drain(..).collect()
        }

        /// Complete a dial: a live connection is established and passes the
        /// generation gate. Returns the fresh generation.
        pub fn connect(&self) -> u64 {
            let generation = {
                let mut shared = self.lock();
                shared.dialing = false;
                let token = shared
                    .core
                    .pending_dial()
                    .expect("connect() with no dial in progress");
                let deferred = shared.drive(Input::Connected { token });
                self.runtime.enqueue(deferred);
                shared.core.generation()
            };
            self.runtime.drain_delivery();
            generation
        }

        /// Complete a dial with an explicit token — used to prove a straggler
        /// completion from a retired attempt is fenced. An accepted token
        /// (the outstanding attempt's) also clears the dialing flag, exactly
        /// as `connect` does; a rejected stale token leaves it untouched.
        pub fn connect_at_token(&self, token: u64) {
            {
                let mut shared = self.lock();
                if shared.core.pending_dial() == Some(token) {
                    shared.dialing = false;
                }
                let deferred = shared.drive(Input::Connected { token });
                self.runtime.enqueue(deferred);
            }
            self.runtime.drain_delivery();
        }

        /// Refuse a dial with an explicit token — used to prove a refusal
        /// from a retired (e.g. stop-cancelled) attempt is fenced.
        pub fn gate_refused_at_token(&self, token: u64) {
            self.drive_serialized(Input::GateRefused { token });
        }

        /// The outstanding dial attempt's token, if a dial is in progress.
        pub fn pending_dial(&self) -> Option<u64> {
            self.lock().core.pending_dial()
        }

        /// Fail the outstanding dial attempt (it never connected): consumes a
        /// retry from the budget and schedules the backoff.
        pub fn fail_dial(&self) {
            let token = {
                let shared = self.lock();
                shared
                    .core
                    .pending_dial()
                    .expect("fail_dial() with no dial in progress")
            };
            self.drive_serialized(Input::DialFailed { token });
        }

        /// Report a recoverable connection loss (a dropped connection or a
        /// send/close race).
        pub fn interrupt(&self) {
            let generation = self.lock().core.generation();
            self.drive_serialized(Input::Interrupted { generation });
        }

        /// Report a loss tagged with the generation the (possibly retired)
        /// transport was connected under — used to prove a stale teardown
        /// from a replaced connection is fenced.
        pub fn interrupt_at_generation(&self, generation: u64) {
            self.drive_serialized(Input::Interrupted { generation });
        }

        /// Report a terminal generation-gate refusal (no auto-retry, §K7).
        pub fn gate_refused(&self) {
            let token = {
                let shared = self.lock();
                shared
                    .core
                    .pending_dial()
                    .expect("gate_refused() with no dial in progress")
            };
            self.drive_serialized(Input::GateRefused { token });
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
            self.drive_serialized(Input::Inbound(Inbound::Reply {
                generation,
                id,
                result: WireReply::Ok(RawJson::from_string(out_json.into())),
            }));
        }

        /// Deliver a typed daemon error for `wire_id` on the current generation.
        pub fn deliver_error(&self, wire_id: u64, error: ApiError) {
            let generation = self.generation();
            let id = RequestId::new(wire_id).expect("wire id within range");
            self.drive_serialized(Input::Inbound(Inbound::Reply {
                generation,
                id,
                result: WireReply::Err(error),
            }));
        }

        /// Deliver a live wire push on the current generation. Takes the
        /// **protocol shape** (`jeliya_api::Push`): the wire can only produce
        /// protocol pushes, so lifecycle transitions and local-overflow
        /// signals cannot be fabricated through this path even in tests.
        pub fn deliver_push(&self, push: jeliya_api::Push) {
            let generation = self.generation();
            self.deliver_push_at_generation(push, generation);
        }

        /// Deliver a push tagged with an explicit generation — used to prove a
        /// stale-generation push is fenced.
        pub fn deliver_push_at_generation(&self, push: jeliya_api::Push, generation: u64) {
            self.drive_serialized(Input::Inbound(Inbound::Push { generation, push }));
        }

        /// Deliver a frame that parses to no envelope: it must strand nothing.
        pub fn deliver_malformed(&self) {
            self.drive_serialized(Input::Inbound(Inbound::Malformed));
        }

        /// Advance the virtual clock WITHOUT firing any due timer: the one
        /// knob event-ordering tests need to process a transport event at or
        /// past a deadline before its `TimerFired` input runs (the production
        /// driver interleaves these arbitrarily).
        pub fn advance_clock_only(&self, ticks: u64) {
            self.lock().now.advance(TickDelta(ticks));
        }

        /// Advance the virtual clock by `ticks`, firing every timer now due (in
        /// ascending fire-time order).
        pub fn advance(&self, ticks: u64) {
            self.lock().now.advance(TickDelta(ticks));
            loop {
                let drove = {
                    let mut shared = self.lock();
                    let now = shared.now;
                    let due = shared
                        .timers
                        .iter()
                        .filter(|(_, at)| **at <= now)
                        // Tie-break equal fire times by timer id so the
                        // reference driver is deterministic when deadlines
                        // coincide (§K13).
                        .min_by_key(|(id, at)| (**at, id.0))
                        .map(|(id, _)| *id);
                    match due {
                        Some(id) => {
                            shared.timers.remove(&id);
                            let deferred = shared.drive(Input::TimerFired(id));
                            self.runtime.enqueue(deferred);
                            true
                        }
                        None => false,
                    }
                };
                // Deliver with the lock dropped, then look for the next due
                // timer (a settle may re-enter and arm or cancel timers).
                if drove {
                    self.runtime.drain_delivery();
                } else {
                    break;
                }
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

        /// Frames evicted from the bounded outbound log without being
        /// observed by `take_outbound` — 0 in any test that drains.
        pub fn outbound_overflow(&self) -> u64 {
            self.lock().outbound_overflow
        }

        /// The number of live per-call reply senders held by the driver. On the
        /// dropped-future path this must fall as calls are cancelled/tombstoned,
        /// never growing without bound (§K12, AC-7); no `Settle` fires for a
        /// cancelled call, so the map is pruned by `Action::DropSender` instead.
        pub fn senders(&self) -> usize {
            self.lock().senders.len()
        }

        // -- stream lifecycle scripting (#269) ------------------------------
        //
        // The controller is the deterministic in-memory stream driver (§S3): it
        // delivers OPEN/CREDIT/DATA/END/ABORT/ACK, drives the `i mod 251` source
        // and a receiver-accepted sink, and exposes the outbound records and the
        // AC-7 bounds. Every method is clock-free; the media flows synchronously
        // within `drive`.

        /// Deliver the daemon's OPEN for `wire_id`, admitting `total` bytes, on
        /// the current generation. For a producer stream this also arms the
        /// deterministic `i mod 251` source of `total` bytes.
        pub fn open(&self, wire_id: u64, total: u64) {
            let generation = self.generation();
            let id = RequestId::new(wire_id).expect("wire id within range");
            {
                let mut shared = self.lock();
                if let Some(call_id) = shared.core.wire_to_call(id, generation) {
                    shared.stream_media.insert(
                        call_id,
                        StreamMedia {
                            wire_id,
                            total,
                            produced: 0,
                        },
                    );
                }
            }
            self.deliver_record(
                StreamRecordMeta::Open {
                    id,
                    stream_id: stream_id_for(wire_id),
                    total,
                },
                generation,
            );
        }

        /// Deliver a daemon CREDIT for `wire_id` (the client is the producer).
        pub fn credit(&self, wire_id: u64, accepted_through: u64, send_through: u64) {
            let generation = self.generation();
            self.deliver_record(
                StreamRecordMeta::Credit {
                    id: RequestId::new(wire_id).expect("wire id within range"),
                    stream_id: stream_id_for(wire_id),
                    accepted_through,
                    send_through,
                },
                generation,
            );
        }

        /// Deliver a daemon DATA range for `wire_id` (the client is the
        /// receiver); the deterministic sink accepts it and advances credit.
        pub fn deliver_data(&self, wire_id: u64, offset: u64, len: u64) {
            let generation = self.generation();
            self.deliver_record(
                StreamRecordMeta::Data {
                    id: RequestId::new(wire_id).expect("wire id within range"),
                    stream_id: stream_id_for(wire_id),
                    offset,
                    len,
                },
                generation,
            );
        }

        /// Deliver the daemon's END for `wire_id` at `offset` (the client is the
        /// receiver).
        pub fn end(&self, wire_id: u64, offset: u64) {
            let generation = self.generation();
            self.deliver_record(
                StreamRecordMeta::End {
                    id: RequestId::new(wire_id).expect("wire id within range"),
                    stream_id: stream_id_for(wire_id),
                    offset,
                },
                generation,
            );
        }

        /// Deliver a daemon ABORT for `wire_id` with a high-water mark; the
        /// client ACKs and settles the stream (§S4).
        pub fn abort(&self, wire_id: u64, high_water: u64) {
            let generation = self.generation();
            self.deliver_record(
                StreamRecordMeta::Abort {
                    id: RequestId::new(wire_id).expect("wire id within range"),
                    stream_id: stream_id_for(wire_id),
                    high_water,
                    reason: StreamAbortReason::Cancelled,
                },
                generation,
            );
        }

        /// Deliver a daemon ACK for the client's ABORT of `wire_id`.
        pub fn ack(&self, wire_id: u64, high_water: u64) {
            let generation = self.generation();
            self.deliver_record(
                StreamRecordMeta::Ack {
                    id: RequestId::new(wire_id).expect("wire id within range"),
                    stream_id: stream_id_for(wire_id),
                    high_water,
                },
                generation,
            );
        }

        /// Deliver a record tagged with an explicit generation — used to prove a
        /// stale-generation record is fenced (§S10).
        pub fn deliver_data_at_generation(
            &self,
            wire_id: u64,
            offset: u64,
            len: u64,
            generation: u64,
        ) {
            self.deliver_record(
                StreamRecordMeta::Data {
                    id: RequestId::new(wire_id).expect("wire id within range"),
                    stream_id: stream_id_for(wire_id),
                    offset,
                    len,
                },
                generation,
            );
        }

        fn deliver_record(&self, record: StreamRecordMeta, generation: u64) {
            self.drive_serialized(Input::Inbound(Inbound::Record { generation, record }));
        }

        /// Take the client-authored outbound stream records since the last drain,
        /// as redaction-safe [`SentRecord`] views (§S3).
        pub fn take_outbound_records(&self) -> Vec<SentRecord> {
            self.lock().outbound_records.drain(..).collect()
        }

        /// Records evicted from the bounded record log without being observed —
        /// 0 in any test that drains.
        pub fn outbound_records_overflow(&self) -> u64 {
            self.lock().outbound_records_overflow
        }

        /// The number of installed streams (active + finalizing + tombstoned) —
        /// asserted within `max_concurrent_streams`, and `0` after stop (§S11).
        pub fn streams(&self) -> usize {
            self.lock().core.stream_count()
        }

        /// The number of armed per-stream timers — asserted `<= 2 ·` active
        /// streams, and `0` after every stream is terminal (§S11).
        pub fn stream_timers(&self) -> usize {
            self.lock().core.stream_timers()
        }

        /// Total bytes reserved across every active stream's window — asserted
        /// `<= streams() · stream_window_bytes` (§S11).
        pub fn stream_window_bytes_reserved(&self) -> u64 {
            self.lock().core.stream_window_bytes_reserved()
        }
    }

    /// A nonzero connection-local stream id for the reference driver, derived
    /// from the wire id. The core adopts OPEN's `stream_id` and ignores it on
    /// later records (routing is by call), so any nonzero value serves.
    fn stream_id_for(wire_id: u64) -> u128 {
        (wire_id as u128).wrapping_add(1).max(1)
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
                stable_principal: true,
                streams: StreamLimits::default(),
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
                stable_principal: true,
                streams: StreamLimits::default(),
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
