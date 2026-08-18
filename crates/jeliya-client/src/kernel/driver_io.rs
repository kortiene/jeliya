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

use crate::kernel::core::Input;
use crate::kernel::inflight::CallId;
use crate::kernel::timing::{Tick, TickDelta, TimerId};
use crate::kernel::transport::{StreamRecordIntent, WireFrame};

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
    fn from_intent(intent: &StreamRecordIntent) -> Self {
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

/// The deterministic producer source state the reference driver owns (§S3): the
/// total the daemon's OPEN admitted and how far the `i mod 251` source has been
/// read. Byte values are never stored — only offsets — so the driver is as
/// byte-bounded as the core.
pub(crate) struct StreamMedia {
    /// The reply-correlation id, so an outbound DATA observation is keyed by the
    /// wire id the test scripted.
    pub(crate) wire_id: u64,
    /// The source's total byte count (from the scripted OPEN).
    pub(crate) total: u64,
    /// How far the source has been read.
    pub(crate) produced: u64,
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

    /// Perform `Action::SendRecord`: frame the record via jeliya-codec and
    /// enqueue it to the live sink (native), or record its redaction-safe view
    /// (in-memory). The byte layout is never handled by the shell.
    fn send_record(&mut self, intent: StreamRecordIntent);

    /// Perform `Action::ProduceData`: fulfill the grant from the stream's
    /// registered byte source (native: real media; in-memory: the
    /// deterministic `i mod 251` rig), reporting `Produced`/`SourceEnd`
    /// through the pending-media queue. `id` is the stream's wire id — the
    /// key a real driver's media registry is organized by.
    fn produce(&mut self, id: jeliya_api::RequestId, call_id: CallId, up_to: u64);

    /// Perform `Action::WriteSink`: hand the accepted range to the stream's
    /// sink, reporting `SinkAccepted` through the pending-media queue. `id`
    /// is the stream's wire id (see [`DriverIo::produce`]).
    fn write_sink(&mut self, id: jeliya_api::RequestId, call_id: CallId, offset: u64, len: u64);

    /// Register one stream's media under its dedup `OpId`, before the call is
    /// dispatched. A real driver binds the registration to the wire id when
    /// it performs the stream op's `Action::Send`; the reference rig
    /// synthesizes its own deterministic media, so the default is a no-op that
    /// lets an unregistered stream fail honestly at `produce`/`write_sink`
    /// time (`SourceFailed`/`SinkFailed` on the real drivers).
    fn register_media(&mut self, _key: jeliya_api::OpId, _media: crate::media::StreamMedia) {}

    /// Drain media inputs the driver fulfilled during the current apply batch
    /// (`Produced`/`SourceEnd`/`SinkAccepted`), re-driven by the shell after
    /// the batch — mirroring [`DriverIo::take_send_failed`]. The default is
    /// for drivers with no in-shell media (the native adapter fulfills media
    /// from its own tasks and injects the inputs itself).
    fn take_pending_media(&mut self) -> VecDeque<Input> {
        VecDeque::new()
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
    /// The client-authored byte-stream records the core asked the driver to
    /// send since the last drain, as redaction-safe [`SentRecord`] views
    /// (§S3/§S12). Bounded FIFO exactly like `outbound` (§K12).
    pub(crate) outbound_records: VecDeque<SentRecord>,
    /// Records evicted from the bounded record log without being observed.
    pub(crate) outbound_records_overflow: u64,
    /// The deterministic media state per producer stream (§S3), keyed by
    /// [`CallId`]. A receiver stream has no source and never appears here;
    /// its sink accepts every delivered range.
    pub(crate) stream_media: HashMap<CallId, StreamMedia>,
    /// Media inputs the rig fulfilled during the current apply batch, drained
    /// by the shell's `drive` afterwards (§S3).
    pub(crate) pending_media: VecDeque<Input>,
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
            outbound_records: VecDeque::new(),
            outbound_records_overflow: 0,
            stream_media: HashMap::new(),
            pending_media: VecDeque::new(),
        }
    }

    /// Append one redaction-safe outbound-record observation to the bounded
    /// log (§K12): an undrained log evicts its oldest entry and counts the
    /// loss.
    fn record_outbound(&mut self, record: SentRecord) {
        if self.outbound_records.len() >= OUTBOUND_LOG_CAP {
            self.outbound_records.pop_front();
            self.outbound_records_overflow = self.outbound_records_overflow.saturating_add(1);
        }
        self.outbound_records.push_back(record);
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

    fn send_record(&mut self, intent: StreamRecordIntent) {
        self.record_outbound(SentRecord::from_intent(&intent));
    }

    fn produce(&mut self, _id: jeliya_api::RequestId, call_id: CallId, up_to: u64) {
        // The deterministic source produces exactly the granted bytes (the
        // core bounds `up_to` by credit, window, and total), frames them,
        // sends them, and reports how far it got (§S3). A single DATA
        // observation covers the whole grant; the byte content is the
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

    fn write_sink(&mut self, _id: jeliya_api::RequestId, call_id: CallId, offset: u64, len: u64) {
        // The deterministic sink accepts every delivered range contiguously
        // and reports its new accepted high-water (§S3).
        self.pending_media.push_back(Input::SinkAccepted {
            call_id,
            through: offset.saturating_add(len),
        });
    }

    fn take_pending_media(&mut self) -> VecDeque<Input> {
        std::mem::take(&mut self.pending_media)
    }

    fn take_send_failed(&mut self) -> bool {
        std::mem::take(&mut self.send_failed)
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
