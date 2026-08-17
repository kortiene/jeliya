//! The `DriverIo` seam: the five transport-touching effects the async shell
//! delegates so one tested delivery/wake machinery ([`crate::kernel`]'s
//! `Runtime`/`Deferred`) drives two very different drivers — the deterministic
//! in-memory reference and the real native WebSocket adapter (#172).
//!
//! The sans-IO [`Core`](crate::kernel::core::Core) emits `Action`s; the async
//! shell performs them. Of those, `Settle`/`DropSender`/`Emit`/`CloseBus` are
//! runtime-neutral (they touch reply senders and the event bus, which every
//! driver shares), while `Send`/`ArmTimer`/`CancelTimer`/`Dial`/`CancelDial`
//! plus the current-time read are exactly the effects a real driver must
//! perform against live I/O. Those, and only those, cross this seam.
//!
//! [`InMemoryIo`] realizes the seam with virtual state (a logical clock, a
//! recorded outbound log, an in-memory timer map, a dialing flag, and the
//! scripted send/close race), so the `test-transport` controller drives the
//! kernel with no wall clock, no RNG, and no scheduling dependence. The native
//! adapter provides its own `DriverIo` against tokio tasks and a real socket —
//! outside `src/kernel/**`, so the sans-IO boundary scan stays green.

use std::any::Any;
use std::collections::{HashMap, VecDeque};

use crate::kernel::timing::{Tick, TickDelta, TimerId};
use crate::kernel::transport::WireFrame;

/// The bound on the in-memory driver's outbound observation log: frames a test
/// never drains are evicted oldest-first (counted in `outbound_overflow`) so
/// the reference driver honours §K12's no-unbounded-collection guarantee even
/// under dispatch/cancel churn with no `take_outbound`.
const OUTBOUND_LOG_CAP: usize = 1024;

/// A redaction-safe view of one sent frame: the controller learns the wire id
/// it must reply to and the operation name, never the payload or `op_id`.
/// The in-memory outbound log stores exactly this view (redacted at append
/// time); publicly visible only through the feature-gated re-export in
/// `lib.rs`.
#[derive(Clone, Copy, Debug)]
pub struct SentFrame {
    /// The wire correlation id.
    pub id: u64,
    /// The operation's wire name.
    pub op: &'static str,
}

/// The transport-touching effects the async shell delegates. The shell owns
/// the reply senders, the event bus, and the serialized delivery queue; a
/// `DriverIo` owns only how a frame reaches the wire, how timers are realized,
/// how a dial is begun/cancelled, and what "now" is.
///
/// Object-safe (the shell holds `Box<dyn DriverIo>`) and `Send` (it lives
/// behind the `Shared` mutex shared across threads/tasks).
pub(crate) trait DriverIo: Send {
    /// The current logical time. The in-memory driver returns its virtual
    /// clock (advanced only by the controller); a native driver maps its
    /// monotonic clock to ticks. The core reads this once per `step`.
    fn now(&self) -> Tick;

    /// Perform `Action::Send`: encode and enqueue one frame to the live sink
    /// (native), or record its redaction-safe view (in-memory).
    fn send(&mut self, frame: WireFrame);

    /// Perform `Action::ArmTimer`.
    fn arm_timer(&mut self, id: TimerId, at: Tick);

    /// Perform `Action::CancelTimer`.
    fn cancel_timer(&mut self, id: TimerId);

    /// Perform `Action::Dial`: begin one dial attempt bound to `token`.
    fn dial(&mut self, token: u64);

    /// Perform `Action::CancelDial`: cancel any in-progress dial/backoff and,
    /// for a real driver, tear down the live connection's tasks (total stop).
    fn cancel_dial(&mut self);

    /// Whether a write failed *synchronously* during the current apply batch,
    /// to be re-fed to the core as `Interrupted` after the batch. The
    /// in-memory driver uses this for the scripted send/close race (§K14); a
    /// real async driver reports a broken pipe from its own write/read task via
    /// an injected `Interrupted` and returns `false` here. Consuming clears it.
    fn take_send_failed(&mut self) -> bool {
        false
    }

    /// Downcast hook for the `test-transport` controller, the only consumer
    /// that needs the concrete [`InMemoryIo`] (to advance the virtual clock,
    /// inspect timers, and read the outbound log). The native driver never
    /// downcasts; its impl returns `self` unused.
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

/// The deterministic in-memory `DriverIo`: virtual clock, recorded outbound
/// log, in-memory timer map, dialing flag, and the scripted send/close race.
/// Every field is clock-free and scheduling-independent.
pub(crate) struct InMemoryIo {
    /// The virtual clock — the sole source of time, advanced only by the
    /// controller's `advance`.
    pub(crate) now: Tick,
    /// Frames the core asked the transport to send, retained as redaction-safe
    /// metadata so the controller can learn the wire ids it must reply to.
    /// Bounded (§K12): an undrained log evicts its oldest entry and counts the
    /// loss.
    pub(crate) outbound: VecDeque<SentFrame>,
    /// Frames evicted from the bounded outbound log without being observed.
    pub(crate) outbound_overflow: u64,
    /// Armed timers: id → fire time. The controller fires the due ones as it
    /// advances the clock.
    pub(crate) timers: HashMap<TimerId, Tick>,
    /// Whether a dial/backoff is in progress.
    pub(crate) dialing: bool,
    /// Scripted send/close race (§K14 `fail_send`): when set, the next
    /// `Action::Send` drops its frame and records `send_failed` instead of
    /// reaching the transport.
    pub(crate) fail_next_send: bool,
    /// A send observed the broken transport during the current apply batch;
    /// the shell surfaces it as `Input::Interrupted` after the batch.
    pub(crate) send_failed: bool,
}

impl InMemoryIo {
    /// A fresh in-memory driver at logical time zero.
    pub(crate) fn new() -> Self {
        Self {
            now: Tick::ZERO,
            outbound: VecDeque::new(),
            outbound_overflow: 0,
            timers: HashMap::new(),
            dialing: false,
            fail_next_send: false,
            send_failed: false,
        }
    }

    /// Advance the virtual clock by `ticks`.
    pub(crate) fn advance(&mut self, ticks: u64) {
        self.now.advance(TickDelta::from_ticks(ticks));
    }
}

impl DriverIo for InMemoryIo {
    fn now(&self) -> Tick {
        self.now
    }

    fn send(&mut self, frame: WireFrame) {
        if self.fail_next_send || self.send_failed {
            // The scripted send/close race (§K14): the pipe breaks at the first
            // write, and a broken transport fails every later write in the same
            // batch too — no frame after the failure reaches the wire. The loss
            // surfaces to the core after this apply batch.
            self.fail_next_send = false;
            self.send_failed = true;
        } else {
            if self.outbound.len() >= OUTBOUND_LOG_CAP {
                self.outbound.pop_front();
                self.outbound_overflow = self.outbound_overflow.saturating_add(1);
            }
            // Redact at APPEND time: the log keeps only what take_outbound
            // exposes, so the payload and op_id drop with the frame here.
            self.outbound.push_back(SentFrame {
                id: frame.id.get(),
                op: frame.op,
            });
        }
    }

    fn arm_timer(&mut self, id: TimerId, at: Tick) {
        self.timers.insert(id, at);
    }

    fn cancel_timer(&mut self, id: TimerId) {
        self.timers.remove(&id);
    }

    fn dial(&mut self, _token: u64) {
        self.dialing = true;
    }

    fn cancel_dial(&mut self) {
        self.dialing = false;
    }

    fn take_send_failed(&mut self) -> bool {
        std::mem::take(&mut self.send_failed)
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
