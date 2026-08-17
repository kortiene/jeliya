//! The sans-IO state machine (§K1–§K11).
//!
//! All bounded-request, lifecycle, cancellation, and retry logic lives in this
//! pure, synchronous core: it is a function of its inputs, with **no async, no
//! I/O, no clock, and no RNG syscall**. [`Core::step`] consumes one [`Input`]
//! and the current logical time, mutates bounded internal state, and returns a
//! `Vec<Action>` for the driver to perform. It **never blocks and never
//! awaits**, so every fault the Verification section names — timeout at each
//! phase, send/close race, reconnect exhaustion, stop — is an ordinary,
//! deterministic sequence of `step` calls, identical on wasm and native.

use std::collections::VecDeque;

use jeliya_api::Incarnation;

use crate::backend::{ErasedCall, RawJson};
use crate::error::{CallError, Execution};
use crate::event::{ClientEvent, State};
use crate::kernel::admission::Admission;
use crate::kernel::backoff::Backoff;
use crate::kernel::ids::{GenerationCounter, IdAllocator};
use crate::kernel::inflight::{CallId, Entry, Ledger, Phase};
use crate::kernel::replay::{is_stream_op, ReplayPolicy};
use crate::kernel::streaming::{
    stream_deadline_at, StreamOutcome, StreamPhase, StreamRole, StreamTable,
};
use crate::kernel::timing::{Tick, TimerId};
use crate::kernel::transport::{
    Inbound, StreamRecordIntent, StreamRecordMeta, WireFrame, WireReply,
};
use crate::kernel::StreamLimits;
use crate::KernelLimits;

/// One input to the core. Every non-determinism a real transport would
/// introduce (a reply, a disconnect, the passage of time) enters here as an
/// ordinary value, which is what makes the fault suite exhaustive (§K1).
pub(crate) enum Input {
    /// A caller issued one operation. `call_id` is the local identity the
    /// driver uses to route the reply and observe a dropped future.
    Dispatch {
        /// The local call identity.
        call_id: CallId,
        /// The erased call to admit.
        call: ErasedCall,
    },
    /// Begin connecting/activating (idempotent).
    Start,
    /// Graceful total stop (§K11).
    Stop,
    /// A live connection was established and passed the generation gate.
    /// Carries the dial token the completing attempt was issued under
    /// (`Action::Dial`): a completion whose token is not the outstanding
    /// dial's — a straggler from a retired attempt, or a duplicate — is
    /// dropped.
    Connected {
        /// The completing dial attempt's token.
        token: u64,
        /// The daemon incarnation this connection reported in `hello`. The
        /// driver lifts it verbatim; the core compares it against the last
        /// one to fence replay across a daemon restart (§K5). Non-optional
        /// (D5): every live connection has one — `DirectClient` supplies a
        /// per-instance constant, a socket adapter forwards `hello.incarnation`.
        incarnation: Incarnation,
    },
    /// A **live** (`Ready`) connection was lost; recoverable via backoff
    /// (§K6, §K10). Tagged with the generation the lost transport was
    /// connected under, so a delayed close callback from a replaced
    /// connection cannot tear down its successor; a dial that never
    /// connected reports [`Input::DialFailed`] instead.
    Interrupted {
        /// The originating transport's connection generation.
        generation: u64,
    },
    /// A dial attempt failed before connecting. Carries the attempt's token
    /// so a duplicate or retired-dial failure cannot burn a retry from the
    /// budget or arm a spurious backoff.
    DialFailed {
        /// The failing dial attempt's token.
        token: u64,
    },
    /// A terminal generation-gate refusal that carries no auto-retry (§K7).
    /// Token-fenced like every other dial outcome.
    GateRefused {
        /// The refused dial attempt's token.
        token: u64,
    },
    /// One inbound frame arrived, tagged with its generation (§K7).
    Inbound(Inbound),
    /// A driver timer fired.
    TimerFired(TimerId),
    /// A caller dropped or cancelled its future (§K9).
    Cancel(CallId),
    /// The producer's media source produced bytes for a `ProduceData` grant:
    /// the driver has framed and sent them, and reports how far it got (§S3).
    Produced {
        /// The stream call.
        call_id: CallId,
        /// The exclusive offset the driver has now sent through.
        sent_through: u64,
    },
    /// The producer's media source reached EOF, reporting the final total (§S3).
    SourceEnd {
        /// The stream call.
        call_id: CallId,
        /// The total bytes the source yielded.
        total: u64,
    },
    /// The producer's media source failed to yield bytes (§S3).
    SourceFailed {
        /// The stream call.
        call_id: CallId,
    },
    /// The receiver's media sink accepted a delivered DATA range, reporting its
    /// new contiguous accepted high-water (§S3).
    SinkAccepted {
        /// The stream call.
        call_id: CallId,
        /// The exclusive offset the sink has now accepted through.
        through: u64,
    },
    /// The receiver's media sink failed to accept bytes (§S3).
    SinkFailed {
        /// The stream call.
        call_id: CallId,
    },
}

/// One action the driver performs after a [`Core::step`]. The core decides;
/// the driver does the I/O.
pub(crate) enum Action {
    /// Push one encoded frame onto the transport.
    Send(WireFrame),
    /// Arm a driver timer to fire at `at`.
    ArmTimer {
        /// The timer's identity.
        id: TimerId,
        /// When it should fire (logical time).
        at: Tick,
    },
    /// Cancel a previously-armed timer.
    CancelTimer(TimerId),
    /// Begin one dial attempt, identified by `token`. Every outcome of the
    /// attempt (`Connected`, `DialFailed`, `GateRefused`) echoes the token,
    /// and the core accepts only the outstanding dial's.
    Dial {
        /// The attempt's identity.
        token: u64,
    },
    /// Cancel any in-progress dial/backoff.
    CancelDial,
    /// Settle one call's reply future, exactly once.
    Settle(CallId, Result<RawJson, CallError>),
    /// Drop one call's reply sender **without** settling it — the caller has
    /// abandoned the future (a queued-cancel or a tombstone creation), so the
    /// driver prunes the sender to keep its per-call map bounded (§K12). The
    /// ledger id may live on as a tombstone, but no sender lingers behind it.
    DropSender(CallId),
    /// Broadcast one event on the fan-out bus.
    Emit(ClientEvent),
    /// Close the event bus (every subscription ends after draining).
    CloseBus,
    /// Send one client-authored byte-stream control record (§S3). The driver
    /// frames it via `jeliya-codec` at the boundary; the core never touches
    /// bytes.
    SendRecord(StreamRecordIntent),
    /// Grant the producer's media source to send DATA covering up to `up_to`
    /// more bytes (§S3/§S5). Already credit- and window-bounded by the core; the
    /// driver reads ≤ `up_to` bytes, frames ≤64 KiB DATA records, sends them, and
    /// reports [`Input::Produced`].
    ProduceData {
        /// The stream call.
        call_id: CallId,
        /// The maximum additional bytes the driver may send now.
        up_to: u64,
    },
    /// Deliver an accepted inbound DATA range to the receiver's media sink
    /// (§S3): the driver writes and reports [`Input::SinkAccepted`].
    WriteSink {
        /// The stream call.
        call_id: CallId,
        /// The range's start offset.
        offset: u64,
        /// The range length in bytes.
        len: u64,
    },
}

/// The sans-IO kernel core.
pub(crate) struct Core {
    limits: KernelLimits,
    state: State,
    /// Set the instant `stop` is invoked, so a dispatch racing the stop future
    /// is refused eagerly (§K11 step 2).
    stopping: bool,
    generation: GenerationCounter,
    ids: IdAllocator,
    ledger: Ledger,
    admission: Admission,
    backoff: Backoff,
    /// FIFO of admitted-but-unsent call ids (bounded by `queue_depth`).
    queue: VecDeque<CallId>,
    /// Sent replayable calls held across a reconnect for re-send (bounded by
    /// `in_flight`).
    replay_hold: VecDeque<CallId>,
    /// FIFO of live tombstone ids (sent entries whose caller cancelled or whose
    /// deadline fired), bounded by `in_flight`: creating a tombstone past the
    /// budget evicts the oldest, so cancel/timeout churn cannot grow the ledger
    /// past its documented bound (§K12, AC-7).
    tombstones: VecDeque<CallId>,
    /// Count of sent, non-cancelled calls — asserted `<= in_flight`.
    in_flight_count: u32,
    /// The single armed reconnect-backoff timer, if any.
    backoff_timer: Option<TimerId>,
    /// Consecutive failed reconnect attempts (§K10).
    attempt: u32,
    /// Monotonic local call-id source.
    next_call_id: u64,
    /// Monotonic timer-id source.
    next_timer_id: u64,
    /// Whether the driver certifies a stable session principal across
    /// reconnects — the precondition for any auto-replay (§K5).
    stable_principal: bool,
    /// The daemon incarnation the last accepted connection reported (`None`
    /// before the first connect). The runtime half of dedup-scope continuity
    /// (§K5, D4): on reconnect a changed incarnation is a restarted daemon
    /// whose in-memory dedup ledger is empty, so any replay-held call is
    /// dropped rather than re-sent against it.
    last_incarnation: Option<Incarnation>,
    /// The outstanding dial attempt's token, if a dial is in progress. Every
    /// dial outcome is fenced against it; `None` while backing off, Ready,
    /// or terminal, so stragglers and duplicates drop.
    pending_dial: Option<u64>,
    /// Monotonic dial-token source.
    next_dial_token: u64,
    /// The byte-stream control plane (§S1): the companion sub-core keyed by the
    /// same [`CallId`], driving `OPEN/DATA/CREDIT/END/ABORT`, credit, and the
    /// per-stream deadline/stall timers over the request/reply core.
    streams: StreamTable,
    /// The served transfer bounds the streams are driven under (§6).
    stream_limits: StreamLimits,
}

impl Core {
    /// Build a core with the given limits and deterministic jitter seed, using
    /// the default stream bounds. Kept 3-arg for the request/reply unit tests,
    /// which never drive streams.
    pub(crate) fn new(limits: KernelLimits, jitter_seed: u64, stable_principal: bool) -> Self {
        Self::with_stream_limits(
            limits,
            jitter_seed,
            stable_principal,
            StreamLimits::default(),
        )
    }

    /// Build a core with explicit stream bounds (§6), the production path.
    pub(crate) fn with_stream_limits(
        limits: KernelLimits,
        jitter_seed: u64,
        stable_principal: bool,
        stream_limits: StreamLimits,
    ) -> Self {
        let admission = Admission::new(limits.queue_depth, limits.outbound_bytes);
        let backoff = Backoff::new(limits.backoff_base, limits.backoff_cap, jitter_seed);
        let streams = StreamTable::new(stream_limits.max_concurrent_streams);
        Self {
            limits,
            state: State::Idle,
            stopping: false,
            generation: GenerationCounter::new(),
            ids: IdAllocator::new(),
            ledger: Ledger::new(),
            admission,
            backoff,
            queue: VecDeque::new(),
            replay_hold: VecDeque::new(),
            tombstones: VecDeque::new(),
            in_flight_count: 0,
            backoff_timer: None,
            attempt: 0,
            next_call_id: 0,
            next_timer_id: 0,
            stable_principal,
            last_incarnation: None,
            pending_dial: None,
            next_dial_token: 0,
            streams,
            stream_limits,
        }
    }

    /// The current lifecycle state.
    pub(crate) fn state(&self) -> State {
        self.state
    }

    /// The current connection generation (0 before the first connect).
    pub(crate) fn generation(&self) -> u64 {
        self.generation.current()
    }

    /// Whether the reconnect-backoff timer is armed (the retry gap between
    /// dial attempts) — the controller folds this into `is_dialing`.
    pub(crate) fn backoff_armed(&self) -> bool {
        self.backoff_timer.is_some()
    }

    /// The outstanding dial attempt's token, if a dial is in progress — the
    /// driver echoes it on every outcome of the attempt.
    pub(crate) fn pending_dial(&self) -> Option<u64> {
        self.pending_dial
    }

    /// Allocate the next local call id (the driver calls this before
    /// dispatching so it can key the reply sender).
    pub(crate) fn alloc_call_id(&mut self) -> CallId {
        let id = CallId(self.next_call_id);
        self.next_call_id = self.next_call_id.wrapping_add(1);
        id
    }

    /// The single entry point: consume one input at logical time `now` and
    /// return the actions the driver must perform.
    pub(crate) fn step(&mut self, input: Input, now: Tick) -> Vec<Action> {
        let mut actions = Vec::new();
        match input {
            Input::Dispatch { call_id, call } => self.on_dispatch(call_id, call, now, &mut actions),
            Input::Start => self.on_start(&mut actions),
            Input::Stop => self.on_stop(now, &mut actions),
            Input::Connected { token, incarnation } => {
                self.on_connected(token, incarnation, now, &mut actions)
            }
            Input::Interrupted { generation } => self.on_interrupted(generation, now, &mut actions),
            Input::DialFailed { token } => self.on_dial_failed(token, now, &mut actions),
            Input::GateRefused { token } => self.on_gate_refused(token, now, &mut actions),
            Input::Inbound(frame) => self.on_inbound(frame, now, &mut actions),
            Input::TimerFired(id) => self.on_timer_fired(id, now, &mut actions),
            Input::Cancel(call_id) => self.on_cancel(call_id, now, &mut actions),
            Input::Produced {
                call_id,
                sent_through,
            } => {
                let outcome = self.streams.on_produced(
                    call_id,
                    sent_through,
                    &self.stream_limits,
                    &mut actions,
                );
                self.apply_stream_outcome(call_id, outcome, now, &mut actions);
            }
            Input::SourceEnd { call_id, total } => {
                let outcome = self.streams.on_source_end(call_id, total, &mut actions);
                self.apply_stream_outcome(call_id, outcome, now, &mut actions);
            }
            Input::SourceFailed { call_id } => {
                let outcome = self.streams.on_source_failed(call_id, &mut actions);
                self.apply_stream_outcome(call_id, outcome, now, &mut actions);
            }
            Input::SinkAccepted { call_id, through } => {
                let outcome = self.streams.on_sink_accepted(
                    call_id,
                    through,
                    now,
                    &self.stream_limits,
                    &mut actions,
                );
                self.apply_stream_outcome(call_id, outcome, now, &mut actions);
            }
            Input::SinkFailed { call_id } => {
                let outcome = self.streams.on_sink_failed(call_id, &mut actions);
                self.apply_stream_outcome(call_id, outcome, now, &mut actions);
            }
        }
        actions
    }

    // -- lifecycle -----------------------------------------------------------

    fn transition(&mut self, to: State, actions: &mut Vec<Action>) {
        let from = self.state;
        if from == to {
            return;
        }
        self.state = to;
        actions.push(Action::Emit(ClientEvent::StateChanged {
            from,
            to,
            // A freshly emitted transition is honest end-to-end; only the fan-out
            // bus sets this when it coalesces a problem-bearing window (§K12).
            coalesced_through_problem: false,
        }));
    }

    /// Mint a fresh dial attempt: a new token becomes the one outstanding
    /// dial every outcome is fenced against.
    fn mint_dial(&mut self, actions: &mut Vec<Action>) {
        self.next_dial_token = self.next_dial_token.wrapping_add(1);
        let token = self.next_dial_token;
        self.pending_dial = Some(token);
        actions.push(Action::Dial { token });
    }

    fn on_start(&mut self, actions: &mut Vec<Action>) {
        if self.state == State::Idle && !self.stopping {
            self.transition(State::Connecting, actions);
            self.mint_dial(actions);
        }
    }

    fn on_connected(
        &mut self,
        token: u64,
        incarnation: Incarnation,
        now: Tick,
        actions: &mut Vec<Action>,
    ) {
        // Only the outstanding dial may complete a connection: the token
        // fence drops duplicates, stragglers from retired attempts, and
        // completions after stop/failure cleared the pending dial — while
        // Ready, Idle, backing off, or terminal, no dial is pending, so
        // every completion drops. Accepting a stray one would bump the
        // generation and clear the wire index without reclassifying sent
        // calls, or flush replay-held calls onto an ungated connection.
        if self.pending_dial != Some(token) {
            return;
        }
        self.pending_dial = None;
        self.generation.bump();
        self.attempt = 0;
        if let Some(timer) = self.backoff_timer.take() {
            actions.push(Action::CancelTimer(timer));
        }
        // A fresh connection starts a fresh correlation-id space (§K3).
        self.ids.reset();
        self.ledger.clear_wire_index();
        // Dedup-scope continuity fence (§K5, D4): the incarnation is the
        // dynamic half `stable_principal` cannot assert statically. A held
        // call was SENT on the prior incarnation and may have executed there;
        // the dedup ledger is in-memory, so a CHANGED incarnation is a
        // restarted daemon with an empty ledger. Detect the change BEFORE the
        // held work is requeued: on change, drop it (settling
        // `Disconnected { Unknown }`, D6) rather than re-send it against the
        // fresh ledger where it would re-execute; unchanged (or first connect),
        // requeue exactly as before.
        let incarnation_changed =
            matches!(&self.last_incarnation, Some(prev) if *prev != incarnation);
        self.last_incarnation = Some(incarnation);
        if incarnation_changed {
            self.drop_replay_hold_on_incarnation_change(actions);
        } else {
            // Replayable calls held across the reconnect flush first, in order.
            let held: Vec<CallId> = self.replay_hold.drain(..).collect();
            for call_id in held.into_iter().rev() {
                self.queue.push_front(call_id);
            }
        }
        self.transition(State::Ready, actions);
        self.flush(now, actions);
    }

    /// Drop every replay-held call because the daemon incarnation changed
    /// across the reconnect (§K5, D6). Each held call was sent on the prior
    /// incarnation and may have executed there, but the new incarnation's
    /// in-memory dedup ledger is empty, so a replay would re-execute it —
    /// `Disconnected { Unknown }` is the honest classification (the weakest
    /// honest claim, never a `DefinitelyNot` that would invite an unguarded
    /// caller retry). Cleanup mirrors the queued-cancel accounting: a held
    /// call is in `Phase::Queued` with its full charge restored at interrupt
    /// (`charge_count_unchecked` re-added the count slot while `holds_charge`
    /// kept the bytes), so `release` frees BOTH. Drain-and-sort by `CallId`
    /// so the action stream is deterministic (§K13). Never-sent queued calls
    /// are untouched — they were never on any incarnation's wire, so they
    /// flush normally on the new connection.
    fn drop_replay_hold_on_incarnation_change(&mut self, actions: &mut Vec<Action>) {
        let mut held: Vec<CallId> = self.replay_hold.drain(..).collect();
        held.sort_unstable();
        for call_id in held {
            if let Some(entry) = self.ledger.take(call_id) {
                if entry.holds_charge {
                    self.admission.release(entry.payload_bytes);
                }
                actions.push(Action::CancelTimer(entry.deadline_timer));
                actions.push(Action::Settle(
                    call_id,
                    Err(CallError::Disconnected {
                        execution: Execution::Unknown,
                    }),
                ));
            }
        }
    }

    fn on_interrupted(&mut self, generation: u64, now: Tick, actions: &mut Vec<Action>) {
        // A connection loss is meaningful only while a connection is live:
        // duplicates after the first loss (state already Interrupted), and
        // anything during Connecting/backoff, drop here — a dial that never
        // connected reports DialFailed, token-fenced separately. The
        // generation fence then drops a delayed close from a REPLACED
        // connection racing its successor (§K7): both fences together make
        // every loss input attributable to exactly the live connection.
        if self.state != State::Ready {
            return;
        }
        if generation != self.generation.current() {
            return;
        }
        // Reclassify every sent call: settle the already-expired as Timeout
        // first (the absolute deadline binds regardless of whether the close
        // or the due timer processes first, matching on_reply), then hold the
        // replayable, settle the rest, drop tombstones. Collect first to
        // avoid mutating during iteration.
        let mut expired = Vec::new();
        let mut hold = Vec::new();
        let mut disconnect = Vec::new();
        let mut drop_cancelled = Vec::new();
        for (call_id, entry) in self.ledger.iter() {
            if entry.is_sent() {
                if entry.cancelled {
                    drop_cancelled.push(call_id);
                } else if entry.deadline_at <= now {
                    expired.push(call_id);
                } else if entry.replay.is_replayable() {
                    hold.push(call_id);
                } else {
                    disconnect.push(call_id);
                }
            }
        }
        // The ledger iterates in hash order; sort by `CallId` (monotonic at
        // dispatch, queue is FIFO) so held calls replay in original send order
        // and the whole action stream is deterministic (§K13).
        expired.sort_unstable();
        hold.sort_unstable();
        disconnect.sort_unstable();
        drop_cancelled.sort_unstable();
        for call_id in expired {
            if let Some(entry) = self.ledger.take(call_id) {
                if entry.holds_charge {
                    self.admission.release_bytes_only(entry.payload_bytes);
                }
                actions.push(Action::CancelTimer(entry.deadline_timer));
                actions.push(Action::Settle(call_id, Err(CallError::Timeout)));
            }
        }
        for call_id in hold {
            if let Some(entry) = self.ledger.get_mut(call_id) {
                entry.phase = Phase::Queued;
                // The held payload re-enters the unsent queue: its bytes
                // never left the reservation, and its count slot is restored
                // (transient count overshoot is the held-work-cannot-be-
                // refused case; new admissions are refused meanwhile).
                debug_assert!(entry.holds_charge, "a replayable retains its charge");
            }
            self.admission.charge_count_unchecked();
            self.replay_hold.push_back(call_id);
        }
        for call_id in disconnect {
            if let Some(entry) = self.ledger.take(call_id) {
                if entry.holds_charge {
                    self.admission.release_bytes_only(entry.payload_bytes);
                }
                actions.push(Action::CancelTimer(entry.deadline_timer));
                actions.push(Action::Settle(
                    call_id,
                    Err(CallError::Disconnected {
                        execution: Execution::Unknown,
                    }),
                ));
            }
        }
        for call_id in drop_cancelled {
            if let Some(entry) = self.ledger.take(call_id) {
                actions.push(Action::CancelTimer(entry.deadline_timer));
            }
        }
        // Every tombstone was a sent entry and every sent entry was just
        // reclassified, so no tombstone survives an interrupt.
        self.tombstones.clear();
        // A stream never survives its connection (§S10): tear down every
        // `StreamEntry`, cancelling its timers. The sent stream ledger entries
        // were just settled `Disconnected { Unknown }` above (streams are
        // `ReplayPolicy::Never`, §S8, so none is held).
        self.streams.drain_all(actions);
        // No sent calls remain; the wire-id space is retired until reconnect.
        self.in_flight_count = 0;
        self.ledger.clear_wire_index();
        self.schedule_reconnect_or_fail(now, actions);
    }

    fn on_dial_failed(&mut self, token: u64, now: Tick, actions: &mut Vec<Action>) {
        // Only the outstanding dial can fail: a duplicate or retired-dial
        // failure callback must not burn a retry from the budget or arm a
        // spurious backoff that would then reject the real completion.
        if self.pending_dial != Some(token) {
            return;
        }
        self.pending_dial = None;
        self.schedule_reconnect_or_fail(now, actions);
    }

    fn schedule_reconnect_or_fail(&mut self, now: Tick, actions: &mut Vec<Action>) {
        if self.attempt >= self.limits.max_reconnect_attempts {
            // Budget exhausted: settle everything honestly, no infinite spin.
            self.fail_all(now, actions);
            return;
        }
        let delay = self.backoff.delay(self.attempt);
        let timer = self.alloc_timer();
        self.backoff_timer = Some(timer);
        actions.push(Action::ArmTimer {
            id: timer,
            at: now.saturating_add(delay),
        });
        self.attempt += 1;
        // Honest labels: `Interrupted` is reserved for "was Ready, lost
        // usability" (the public State contract). A retry of the INITIAL
        // activation stays `Connecting` — no session was ever established,
        // so nothing was interrupted.
        if self.state != State::Connecting {
            self.transition(State::Interrupted, actions);
        }
    }

    fn on_gate_refused(&mut self, token: u64, now: Tick, actions: &mut Vec<Action>) {
        // Token-fenced like every dial outcome — which also makes stop win
        // (§K11): stop cancels the dial and clears the pending token, so a
        // refusal the cancelled dial already queued cannot flip a stopped
        // client to Failed after the bus closed.
        if self.pending_dial != Some(token) {
            return;
        }
        self.pending_dial = None;
        // A never-sent call is provably unexecuted (`DefinitelyNot`), but a
        // call held for replay was sent on a *prior live generation* and may
        // have executed there — this generation's gate refusal says nothing
        // about that one. `fail_all` classifies by `ever_sent` exactly as
        // §K6 requires, so the barrier delegates to it.
        self.fail_all(now, actions);
    }

    fn fail_all(&mut self, now: Tick, actions: &mut Vec<Action>) {
        actions.push(Action::CancelDial);
        self.pending_dial = None;
        if let Some(timer) = self.backoff_timer.take() {
            actions.push(Action::CancelTimer(timer));
        }
        let mut drained = self.ledger.drain();
        drained.sort_unstable_by_key(|(call_id, _)| *call_id);
        for (call_id, entry) in drained {
            actions.push(Action::CancelTimer(entry.deadline_timer));
            if !entry.cancelled {
                // The absolute deadline binds here exactly as it does for
                // replies and interrupts: a call whose budget already elapsed
                // settles Timeout regardless of whether the terminal failure
                // or its due timer processes first.
                let settled = if entry.deadline_at <= now {
                    Err(CallError::Timeout)
                } else {
                    let execution = if entry.ever_sent {
                        Execution::Unknown
                    } else {
                        Execution::DefinitelyNot
                    };
                    Err(CallError::Disconnected { execution })
                };
                actions.push(Action::Settle(call_id, settled));
            }
        }
        self.streams.drain_all(actions);
        self.reset_work();
        self.transition(State::Failed, actions);
    }

    fn on_stop(&mut self, now: Tick, actions: &mut Vec<Action>) {
        if self.stopping || self.state == State::Stopped {
            return;
        }
        self.stopping = true;
        // 1. Cancel any in-progress dial/backoff.
        actions.push(Action::CancelDial);
        self.pending_dial = None;
        if let Some(timer) = self.backoff_timer.take() {
            actions.push(Action::CancelTimer(timer));
        }
        // 2. Stopping — new calls are refused from here (see on_dispatch).
        self.transition(State::Stopping, actions);
        // 3. Drain queue and in-flight, settling each exactly once, in
        // CallId order so the reference trace is deterministic (§K13).
        let mut drained = self.ledger.drain();
        drained.sort_unstable_by_key(|(call_id, _)| *call_id);
        for (call_id, entry) in drained {
            actions.push(Action::CancelTimer(entry.deadline_timer));
            if !entry.cancelled {
                // The absolute deadline binds at stop as on every other path:
                // an already-expired call settles Timeout, not Cancelled,
                // regardless of stop-vs-timer ordering.
                let settled = if entry.deadline_at <= now {
                    Err(CallError::Timeout)
                } else {
                    let execution = if entry.ever_sent {
                        Execution::Unknown
                    } else {
                        Execution::DefinitelyNot
                    };
                    Err(CallError::Cancelled { execution })
                };
                actions.push(Action::Settle(call_id, settled));
            }
        }
        // Total stop drains every stream alongside the ledger (§S11/§K11):
        // cancel all stream timers and empty the table and its tombstones.
        self.streams.drain_all(actions);
        self.reset_work();
        // 4. Final Stopped transition, then close every subscription.
        self.transition(State::Stopped, actions);
        actions.push(Action::CloseBus);
    }

    /// Empty every bounded work collection (used by stop and terminal failure).
    fn reset_work(&mut self) {
        self.queue.clear();
        self.replay_hold.clear();
        self.tombstones.clear();
        self.admission.clear();
        self.in_flight_count = 0;
    }

    /// Record a sent entry that became a tombstone (caller cancel or live
    /// timeout), evicting the oldest live tombstone once the budget
    /// (`in_flight`) is full so cancel/timeout churn cannot grow the ledger
    /// past its documented bound (§K12, AC-7). Early eviction is safe: wire
    /// ids are monotonic within a generation, so an evicted id is never
    /// reissued and its late reply resolves to nothing and is dropped.
    fn push_tombstone(&mut self, call_id: CallId, actions: &mut Vec<Action>) {
        if self.tombstones.len() >= self.limits.in_flight as usize {
            if let Some(oldest) = self.tombstones.pop_front() {
                if let Some(entry) = self.ledger.take(oldest) {
                    actions.push(Action::CancelTimer(entry.deadline_timer));
                }
            }
        }
        self.tombstones.push_back(call_id);
    }

    // -- dispatch, cancel, send ---------------------------------------------

    fn on_dispatch(
        &mut self,
        call_id: CallId,
        call: ErasedCall,
        now: Tick,
        actions: &mut Vec<Action>,
    ) {
        // A call begun after stop, or once terminally failed, never leaves the
        // seam — refused eagerly with DefinitelyNot.
        if self.stopping || matches!(self.state, State::Stopping | State::Stopped | State::Failed) {
            actions.push(Action::Settle(
                call_id,
                Err(CallError::Cancelled {
                    execution: Execution::DefinitelyNot,
                }),
            ));
            return;
        }
        // The charge covers everything the ledger retains per queued call that
        // scales with caller input: the serialized `in` AND the envelope
        // `op_id` — `OpId` has no length cap, so an uncharged key would let
        // tiny-input/huge-key calls slip past the byte bound (AC-1, §K12).
        let payload_bytes = call.input.as_str().len() as u64
            + call.op_id.as_ref().map_or(0, |id| id.as_str().len() as u64);
        // A byte-stream op is additionally bounded by `max_concurrent_streams`
        // (§S11): a stream past the cap is refused **before OPEN**, visibly, like
        // any other bounded resource. The count spans every live stream ledger
        // entry (queued, sent-awaiting-OPEN, active, finalizing, and tombstoned),
        // so cancel/timeout churn cannot slip a stream past the bound.
        let stream = is_stream_op(call.op);
        if stream
            && self.active_stream_count() >= self.stream_limits.max_concurrent_streams as usize
        {
            actions.push(Action::Settle(
                call_id,
                Err(CallError::QueueFull {
                    resource: "max_concurrent_streams",
                    limit: self.stream_limits.max_concurrent_streams as u64,
                }),
            ));
            return;
        }
        if let Err(queue_full) = self.admission.try_admit(payload_bytes) {
            // QueueFull is visible, never absorbed (§K2). DefinitelyNot.
            actions.push(Action::Settle(call_id, Err(queue_full)));
            return;
        }
        let deadline_timer = self.alloc_timer();
        let deadline_at = now.saturating_add(self.limits.default_call_deadline);
        actions.push(Action::ArmTimer {
            id: deadline_timer,
            at: deadline_at,
        });
        let replay = ReplayPolicy::derive(
            call.op,
            call.mutating,
            call.op_id.as_ref(),
            self.stable_principal,
        );
        self.ledger.insert(
            call_id,
            Entry {
                op: call.op,
                op_id: call.op_id,
                input: call.input,
                payload_bytes,
                replay,
                deadline_timer,
                deadline_at,
                holds_charge: true,
                phase: Phase::Queued,
                ever_sent: false,
                cancelled: false,
                stream,
            },
        );
        self.queue.push_back(call_id);
        // Only a Ready connection sends; otherwise the call waits behind the
        // protocol-validation barrier (§K7).
        if self.state == State::Ready {
            self.flush(now, actions);
        }
    }

    fn on_cancel(&mut self, call_id: CallId, now: Tick, actions: &mut Vec<Action>) {
        let Some(entry) = self.ledger.get(call_id) else {
            return;
        };
        match entry.phase {
            Phase::Queued => {
                let holds_charge = entry.holds_charge;
                let payload_bytes = entry.payload_bytes;
                let deadline_timer = entry.deadline_timer;
                self.ledger.take(call_id);
                self.queue.retain(|&queued| queued != call_id);
                self.replay_hold.retain(|&held| held != call_id);
                // A queued call (never-sent, or held for replay) still holds
                // its admission charge.
                if holds_charge {
                    self.admission.release(payload_bytes);
                }
                actions.push(Action::CancelTimer(deadline_timer));
                // No frame is sent for a queued cancel; the caller's future is
                // already gone, so there is nothing to settle. Drop its reply
                // sender so the driver's per-call map does not grow without
                // bound on the routine dropped-future path (§K12, AC-7).
                actions.push(Action::DropSender(call_id));
            }
            Phase::Sent { .. } => {
                if entry.cancelled {
                    return;
                }
                // A cancelled stream past OPEN drives a real client ABORT so the
                // daemon's transfer reservation is released (§S9), retiring its
                // `StreamEntry`. FINALIZING is immune (the terminal reply settles
                // it); a pre-OPEN stream has no `StreamEntry` and falls through to
                // the ordinary sent-tombstone path. The ABORT is meaningful only
                // on a live connection.
                if self.state == State::Ready && self.streams.contains(call_id) {
                    // Emits the ABORT and retires the StreamEntry; the ledger is
                    // tombstoned below. The caller's future is already dropped
                    // (this is the drop-driven cancel path, §K9), so no settle.
                    let _ = self.streams.on_cancel(call_id, actions);
                }
                // Tombstone: keep the correlation id reserved so a real late
                // reply is matched and discarded, but deliver nothing and never
                // send a cancel frame or claim remote cancellation (§K9). The
                // throttle slot is freed so a queued call can proceed.
                self.in_flight_count = self.in_flight_count.saturating_sub(1);
                let mut release = None;
                if let Some(entry) = self.ledger.get_mut(call_id) {
                    entry.cancelled = true;
                    // A cancelled call never replays: its byte-only reservation is
                    // released with the tombstone AND its payload-bearing fields
                    // are dropped — a tombstone retaining the payload would hold up
                    // to in_flight payloads outside the released bound under
                    // send/drop churn.
                    entry.input = RawJson::from_string(String::new());
                    entry.op_id = None;
                    if entry.holds_charge {
                        entry.holds_charge = false;
                        release = Some(entry.payload_bytes);
                    }
                }
                if let Some(bytes) = release {
                    self.admission.release_bytes_only(bytes);
                }
                self.push_tombstone(call_id, actions);
                // The future is gone; drop its reply sender now. The ledger entry
                // survives as a tombstone (reclaimed by the late reply or its
                // deadline), but the sender must not linger, or a client that
                // routinely drops call futures would grow the driver's map
                // unboundedly (§K12, AC-7).
                actions.push(Action::DropSender(call_id));
                self.flush(now, actions);
            }
        }
    }

    fn flush(&mut self, now: Tick, actions: &mut Vec<Action>) {
        if self.state != State::Ready {
            return;
        }
        while self.in_flight_count < self.limits.in_flight {
            let Some(call_id) = self.queue.pop_front() else {
                break;
            };
            // A queued call whose absolute deadline already elapsed (a
            // shared-deadline timer race, or a long backoff before a replay)
            // must not be put on the wire past its budget: settle `Timeout`
            // now, mirroring the queued-timeout branch of `on_timer_fired`.
            if let Some(entry) = self.ledger.get(call_id) {
                if entry.deadline_at <= now {
                    let holds_charge = entry.holds_charge;
                    let payload_bytes = entry.payload_bytes;
                    let deadline_timer = entry.deadline_timer;
                    let cancelled = entry.cancelled;
                    self.ledger.take(call_id);
                    self.replay_hold.retain(|&held| held != call_id);
                    if holds_charge {
                        self.admission.release(payload_bytes);
                    }
                    actions.push(Action::CancelTimer(deadline_timer));
                    if !cancelled {
                        actions.push(Action::Settle(call_id, Err(CallError::Timeout)));
                    }
                    continue;
                }
            }
            // A cancelled queued entry is removed at cancel time, so any id
            // still in the queue names a live entry; defend anyway.
            let request_id = {
                let ledger = &self.ledger;
                self.ids
                    .allocate(|rid| ledger.is_wire_id_outstanding(rid))
                    .request_id()
            };
            let generation = self.generation.current();
            let (frame, holds_charge, payload_bytes, replayable) = match self.ledger.get(call_id) {
                Some(entry) => (
                    WireFrame {
                        id: request_id,
                        op: entry.op,
                        op_id: entry.op_id.clone(),
                        input: entry.input.clone(),
                    },
                    entry.holds_charge,
                    entry.payload_bytes,
                    entry.replay.is_replayable(),
                ),
                None => continue,
            };
            self.ledger.mark_sent(call_id, request_id, generation);
            if holds_charge {
                if replayable {
                    // A replayable keeps a BYTE-ONLY reservation while sent:
                    // the payload stays resident for a hold-and-requeue, so
                    // its bytes stay reserved — but the count slot frees,
                    // because queue_depth counts admitted-but-unsent calls
                    // and in-flight work is throttled, never rejected.
                    self.admission.release_count_keep_bytes();
                } else {
                    // A non-replayable will never re-send: release the whole
                    // charge AND drop the payload-bearing fields — retaining
                    // up to in_flight payloads after their charges were
                    // released would hold ~in_flight × outbound_bytes outside
                    // the advertised bound.
                    if let Some(entry) = self.ledger.get_mut(call_id) {
                        entry.holds_charge = false;
                        entry.input = RawJson::from_string(String::new());
                        entry.op_id = None;
                    }
                    self.admission.release(payload_bytes);
                }
            }
            self.in_flight_count += 1;
            actions.push(Action::Send(frame));
        }
    }

    // -- inbound and timers --------------------------------------------------

    fn on_inbound(&mut self, frame: Inbound, now: Tick, actions: &mut Vec<Action>) {
        match frame {
            Inbound::Reply {
                generation,
                id,
                result,
            } => self.on_reply(generation, id, result, now, actions),
            Inbound::Push { generation, push } => {
                // Fence pushes twice over (§K7): the generation must match —
                // a teardown from a prior connection cannot overwrite newer
                // presence state — AND the connection must be Ready. Before
                // the first connect both sides sit at generation 0, and after
                // an interrupt the generation is unchanged until reconnect,
                // so equality alone would admit pre-gate traffic or a retired
                // transport's stragglers. The lift from the wire shape happens
                // here, inside the boundary: only protocol pushes exist past
                // the transport, so no driver can fabricate a lifecycle or
                // local-overflow event.
                if self.state == State::Ready && generation == self.generation.current() {
                    actions.push(Action::Emit(ClientEvent::from(push)));
                }
            }
            Inbound::Record { generation, record } => {
                self.on_record(generation, record, now, actions)
            }
            // A frame that parses to no envelope correlates to nothing: drop it
            // with no settle so it strands no call (§K4).
            Inbound::Malformed => {}
        }
    }

    /// Route one bound byte-stream record to the stream sub-core (§S1/§S4),
    /// generation-fenced exactly as a reply is (§S10). OPEN installs a
    /// `StreamEntry` (replacing the request/reply base deadline with the stream's
    /// two timers); every other record drives the record exchange. A record for
    /// an unknown/settled/stale-generation call resolves to nothing and is
    /// dropped, stranding no call.
    fn on_record(
        &mut self,
        generation: u64,
        record: StreamRecordMeta,
        now: Tick,
        actions: &mut Vec<Action>,
    ) {
        // Generation-fenced routing: reuse the reply resolver so a stale or
        // late/duplicate record drops (§S10). A tombstoned (cancelled) stream
        // entry still resolves, so its late record is absorbed by the retired
        // `StreamEntry` rather than mis-routed.
        let Some(call_id) = self.ledger.resolve_reply(record.id(), generation) else {
            return;
        };
        // Only a stream ledger entry participates in the record exchange.
        if !self.ledger.get(call_id).map(|e| e.stream).unwrap_or(false) {
            return;
        }
        match record {
            StreamRecordMeta::Open {
                stream_id, total, ..
            } => self.on_stream_open(call_id, stream_id, total, generation, now, actions),
            StreamRecordMeta::Credit {
                accepted_through,
                send_through,
                ..
            } => {
                if self.streams.contains(call_id) {
                    let outcome = self.streams.on_credit(
                        call_id,
                        accepted_through,
                        send_through,
                        now,
                        &self.stream_limits,
                        actions,
                    );
                    self.apply_stream_outcome(call_id, outcome, now, actions);
                }
            }
            StreamRecordMeta::Data { offset, len, .. } => {
                if self.streams.contains(call_id) {
                    let outcome = self.streams.on_data(call_id, offset, len, actions);
                    self.apply_stream_outcome(call_id, outcome, now, actions);
                }
            }
            StreamRecordMeta::End { offset, .. } => {
                if self.streams.contains(call_id) {
                    let outcome = self.streams.on_end(call_id, offset, actions);
                    self.apply_stream_outcome(call_id, outcome, now, actions);
                }
            }
            StreamRecordMeta::Abort { .. } => {
                if self.streams.contains(call_id) {
                    let outcome = self.streams.on_abort(call_id, actions);
                    self.apply_stream_outcome(call_id, outcome, now, actions);
                }
            }
            StreamRecordMeta::Ack { .. } => {
                if self.streams.contains(call_id) {
                    let outcome = self.streams.on_ack(call_id, actions);
                    self.apply_stream_outcome(call_id, outcome, now, actions);
                }
            }
        }
    }

    /// Install a stream at OPEN (§S4/§S6): cancel the request/reply base deadline
    /// (the connect allowance already covered the pre-OPEN handshake) and disarm
    /// it in the ledger, then hand the stream layer two fresh timers to arm as
    /// the absolute deadline + stall. A duplicate/late OPEN is a bound-record
    /// fault that aborts only that stream (§S10).
    fn on_stream_open(
        &mut self,
        call_id: CallId,
        stream_id: u128,
        total: u64,
        open_generation: u64,
        now: Tick,
        actions: &mut Vec<Action>,
    ) {
        let Some(role) = self
            .ledger
            .get(call_id)
            .filter(|e| !e.cancelled)
            .map(|e| stream_role(e.op))
        else {
            return;
        };
        if self.streams.contains(call_id) {
            // A second OPEN for an already-installed stream: local abort.
            let outcome = self.streams.abort_protocol(call_id, actions);
            self.apply_stream_outcome(call_id, outcome, now, actions);
            return;
        }
        // Replace the base request/reply deadline with the stream's budget:
        // cancel the base timer and record the COMPUTED stream deadline in the
        // ledger — never `u64::MAX`. The absolute deadline must bind regardless
        // of event ordering (the same guarantee `on_reply`/`on_interrupted`
        // give ordinary calls): a reply or disconnect processed at or after the
        // stream deadline but before its `TimerFired` input settles `Timeout`,
        // not a dishonest success/`Disconnected`. The stream's own deadline
        // timer is armed at the same instant below, so both paths agree.
        let stream_deadline = stream_deadline_at(now, total, &self.stream_limits);
        let wire_id = if let Some(entry) = self.ledger.get_mut(call_id) {
            actions.push(Action::CancelTimer(entry.deadline_timer));
            entry.deadline_at = stream_deadline;
            match entry.phase {
                Phase::Sent { wire_id, .. } => wire_id,
                Phase::Queued => return, // OPEN before send is impossible; defend.
            }
        } else {
            return;
        };
        let deadline_timer = self.alloc_timer();
        let stall_timer = self.alloc_timer();
        self.streams.open(
            call_id,
            role,
            wire_id,
            stream_id,
            total,
            open_generation,
            now,
            &self.stream_limits,
            deadline_timer,
            stall_timer,
            actions,
        );
    }

    /// Apply the stream sub-core's outcome to the request/reply ledger (§S1). A
    /// failure settles the call's terminal and tombstones it so a late Text reply
    /// is absorbed; progress leaves the ledger untouched (the terminal Text reply
    /// settles it later through `on_reply`).
    fn apply_stream_outcome(
        &mut self,
        call_id: CallId,
        outcome: StreamOutcome,
        now: Tick,
        actions: &mut Vec<Action>,
    ) {
        match outcome {
            StreamOutcome::Progress => {}
            StreamOutcome::Failed(error) => {
                self.settle_stream_failure(call_id, error, now, actions)
            }
        }
    }

    /// Settle a failed stream's ledger entry and turn it into a request/reply
    /// tombstone (§S10), mirroring the live-timeout path: decrement the in-flight
    /// throttle, drop the payload, arm a reclaim timer so a silent daemon cannot
    /// strand the id, and absorb a late terminal Text reply. The stream layer has
    /// already emitted any ABORT and retired its `StreamEntry`.
    fn settle_stream_failure(
        &mut self,
        call_id: CallId,
        error: CallError,
        now: Tick,
        actions: &mut Vec<Action>,
    ) {
        let Some(entry) = self.ledger.get(call_id) else {
            return;
        };
        if entry.cancelled {
            // Already a tombstone (e.g. a redundant failure): nothing to settle.
            return;
        }
        self.in_flight_count = self.in_flight_count.saturating_sub(1);
        let reclaim = self.alloc_timer();
        let mut release = None;
        if let Some(entry) = self.ledger.get_mut(call_id) {
            entry.cancelled = true;
            entry.deadline_timer = reclaim;
            entry.deadline_at = Tick(u64::MAX);
            entry.input = RawJson::from_string(String::new());
            entry.op_id = None;
            if entry.holds_charge {
                entry.holds_charge = false;
                release = Some(entry.payload_bytes);
            }
        }
        if let Some(bytes) = release {
            self.admission.release_bytes_only(bytes);
        }
        actions.push(Action::ArmTimer {
            id: reclaim,
            at: now.saturating_add(self.limits.default_call_deadline),
        });
        self.push_tombstone(call_id, actions);
        actions.push(Action::Settle(call_id, Err(error)));
        self.flush(now, actions);
    }

    /// The number of live stream ledger entries — the `max_concurrent_streams`
    /// admission count (§S11). Bounded by the ledger's own bounds, so this scan
    /// is over a bounded set.
    fn active_stream_count(&self) -> usize {
        self.ledger.iter().filter(|(_, entry)| entry.stream).count()
    }

    fn on_reply(
        &mut self,
        generation: u64,
        id: jeliya_api::RequestId,
        result: WireReply,
        now: Tick,
        actions: &mut Vec<Action>,
    ) {
        // Generation-fenced routing: a stale or late/duplicate reply resolves to
        // nothing and is dropped.
        let Some(call_id) = self.ledger.resolve_reply(id, generation) else {
            return;
        };
        // A SUCCESS reply is only valid once the stream is `Finalizing` (after
        // END — §S4). An `Ok` arriving while the stream is still `Active`
        // claims success before the byte exchange completed; settling it would
        // report a dishonest success and discard the live stream. Treat it as
        // the protocol fault it is (abort the stream, settle `Timeout`) — a
        // pre-OPEN dedup-hit success remains valid because no `StreamEntry`
        // exists yet.
        if matches!(result, WireReply::Ok(_))
            && self.streams.phase_of(call_id) == Some(StreamPhase::Active)
        {
            let outcome = self.streams.abort_protocol(call_id, actions);
            self.apply_stream_outcome(call_id, outcome, now, actions);
            return;
        }
        let entry = self.ledger.take(call_id).expect("resolved entry exists");
        // A stream call's terminal is this Text reply (§S4): retire its
        // `StreamEntry` — cancel any live stream timers and drop the entry (and
        // its tombstone slot on the reclaim path) so nothing survives the
        // settlement.
        if entry.stream {
            self.streams.retire(call_id, actions);
        }
        // A sent replayable still holds its byte-only reservation; settling
        // releases it (the count slot freed at send).
        if entry.holds_charge {
            self.admission.release_bytes_only(entry.payload_bytes);
        }
        actions.push(Action::CancelTimer(entry.deadline_timer));
        if !entry.cancelled {
            self.in_flight_count = self.in_flight_count.saturating_sub(1);
            // The absolute deadline binds regardless of event ordering: a
            // reply arriving at or past `deadline_at` settles `Timeout` with
            // the payload dropped — the same outcome as the deadline timer
            // firing first — so deadline behavior does not depend on whether
            // the driver processes the transport event or the timer first.
            let settled = if entry.deadline_at <= now {
                Err(CallError::Timeout)
            } else {
                match result {
                    WireReply::Ok(raw) => Ok(raw),
                    WireReply::Err(api) => Err(CallError::Wire(api)),
                }
            };
            actions.push(Action::Settle(call_id, settled));
        } else {
            // The late real reply reclaimed a tombstone.
            self.tombstones.retain(|&t| t != call_id);
        }
        // A freed slot may release a queued call.
        self.flush(now, actions);
    }

    fn on_timer_fired(&mut self, timer: TimerId, now: Tick, actions: &mut Vec<Action>) {
        if self.backoff_timer == Some(timer) {
            self.backoff_timer = None;
            self.mint_dial(actions);
            return;
        }
        // A per-stream deadline or stall timer (§S6/§S7): the stream layer owns
        // its own bounded set of timers, distinct from the ledger's per-call
        // deadline. An `Active` stream fails honestly (Timeout + courtesy ABORT);
        // the stall timer may instead defer itself if progress is still fresh.
        if let Some(call_id) = self.streams.find_by_timer(timer) {
            if let Some(outcome) =
                self.streams
                    .on_timer(call_id, timer, now, &self.stream_limits, actions)
            {
                self.apply_stream_outcome(call_id, outcome, now, actions);
            }
            return;
        }
        let Some(call_id) = self.ledger.find_by_deadline(timer) else {
            return; // already cancelled
        };
        let (is_sent, cancelled, holds_charge, payload_bytes) = {
            let entry = self.ledger.get(call_id).expect("found by deadline");
            (
                entry.is_sent(),
                entry.cancelled,
                entry.holds_charge,
                entry.payload_bytes,
            )
        };
        if is_sent {
            if cancelled {
                // A tombstone whose reclaim deadline elapsed: reclaim it now.
                self.ledger.take(call_id);
                self.tombstones.retain(|&t| t != call_id);
            } else {
                // A live in-flight call timed out: settle Unknown (it may still
                // land) and tombstone the id so a late reply is absorbed rather
                // than mis-routed. No remote cancellation is claimed (§K8/§K9).
                // The elapsed deadline timer was consumed, so a fresh reclaim
                // timer is armed: without one, a silent peer would leave the
                // tombstone (and its wire-index record) in the ledger forever
                // (§K12, AC-7).
                self.in_flight_count = self.in_flight_count.saturating_sub(1);
                let reclaim = self.alloc_timer();
                let mut release = None;
                if let Some(entry) = self.ledger.get_mut(call_id) {
                    entry.cancelled = true;
                    entry.deadline_timer = reclaim;
                    // A timed-out call is settled and never replays: release
                    // its byte-only reservation with the tombstone and drop
                    // its payload-bearing fields (see the cancel path).
                    entry.input = RawJson::from_string(String::new());
                    entry.op_id = None;
                    if entry.holds_charge {
                        entry.holds_charge = false;
                        release = Some(entry.payload_bytes);
                    }
                }
                if let Some(bytes) = release {
                    self.admission.release_bytes_only(bytes);
                }
                actions.push(Action::ArmTimer {
                    id: reclaim,
                    at: now.saturating_add(self.limits.default_call_deadline),
                });
                self.push_tombstone(call_id, actions);
                actions.push(Action::Settle(call_id, Err(CallError::Timeout)));
                self.flush(now, actions);
            }
        } else {
            // A queued call timed out before it was ever sent.
            self.ledger.take(call_id);
            self.queue.retain(|&queued| queued != call_id);
            self.replay_hold.retain(|&held| held != call_id);
            if holds_charge {
                self.admission.release(payload_bytes);
            }
            if !cancelled {
                actions.push(Action::Settle(call_id, Err(CallError::Timeout)));
            }
        }
    }

    fn alloc_timer(&mut self) -> TimerId {
        let id = TimerId(self.next_timer_id);
        self.next_timer_id = self.next_timer_id.wrapping_add(1);
        id
    }

    // -- introspection (bounds assertions for tests and the controller) ------

    /// The count of sent, non-cancelled calls — invariant `<= in_flight`.
    pub(crate) fn in_flight_count(&self) -> u32 {
        self.in_flight_count
    }

    /// The count of admitted-but-unsent calls.
    pub(crate) fn queued_len(&self) -> usize {
        self.queue.len()
    }

    /// The count of live ledger entries — `0` after a total stop.
    pub(crate) fn ledger_len(&self) -> usize {
        self.ledger.len()
    }

    /// The count of calls held for replay across a reconnect.
    pub(crate) fn replay_hold_len(&self) -> usize {
        self.replay_hold.len()
    }

    /// The consecutive-failed-reconnect counter.
    pub(crate) fn attempt(&self) -> u32 {
        self.attempt
    }

    /// The number of installed streams (active + finalizing + tombstoned) — the
    /// AC-7 bound the controller asserts (§S11).
    pub(crate) fn stream_count(&self) -> usize {
        self.streams.len()
    }

    /// The number of armed per-stream timers (≤ 2 per active stream, 0 once
    /// terminal) — the AC-7 bound the controller asserts (§S11).
    pub(crate) fn stream_timers(&self) -> usize {
        self.streams.armed_timers()
    }

    /// Total bytes reserved across every active stream's window — the AC-7 memory
    /// bound the controller asserts (§S11).
    pub(crate) fn stream_window_bytes_reserved(&self) -> u64 {
        self.streams.window_bytes_reserved()
    }

    /// Resolve a wire id to its (sent) call on `generation` — the reference
    /// driver uses this to key its deterministic media source by [`CallId`] when
    /// the test scripts an OPEN (§S3).
    pub(crate) fn wire_to_call(
        &self,
        wire_id: jeliya_api::RequestId,
        generation: u64,
    ) -> Option<CallId> {
        self.ledger.resolve_reply(wire_id, generation)
    }
}

/// Which half of the duplex the client drives for a stream op (§S4), by path.
/// Only `file.share`/`file.read` reach here (gated by `is_stream_op` at
/// dispatch); any other op defaults to `Receiver`, which is inert because no
/// non-stream op installs a `StreamEntry`.
fn stream_role(op: &str) -> StreamRole {
    if op == "file.share" {
        StreamRole::Producer
    } else {
        StreamRole::Receiver
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Execution;
    use crate::kernel::timing::TickDelta;
    use jeliya_api::{ApiError, OpId};

    fn limits() -> KernelLimits {
        KernelLimits::default()
    }

    /// A fixed daemon incarnation for tests that reconnect to the *same*
    /// daemon: passing it on every `Connected` keeps the incarnation fence
    /// (D4) quiescent, so pre-#270 reconnect/replay behaviour is unchanged.
    /// Tests exercising a restart pass a distinct value to fire the fence.
    fn inc(tag: &str) -> Incarnation {
        Incarnation::new(tag)
    }

    /// The default incarnation for the connect helper below and every
    /// reconnect-to-the-same-daemon test.
    fn test_incarnation() -> Incarnation {
        inc("inc-default")
    }

    fn erased(op: &'static str, mutating: bool, op_id: Option<&str>) -> ErasedCall {
        ErasedCall {
            op,
            mutating,
            op_id: op_id.map(OpId::new),
            input: RawJson::from_string(String::from("{}")),
        }
    }

    fn find_settle(actions: &[Action], call_id: CallId) -> Option<&Result<RawJson, CallError>> {
        actions.iter().find_map(|action| match action {
            Action::Settle(id, result) if *id == call_id => Some(result),
            _ => None,
        })
    }

    fn first_send_id(actions: &[Action]) -> Option<u64> {
        actions.iter().find_map(|action| match action {
            Action::Send(frame) => Some(frame.id.get()),
            _ => None,
        })
    }

    fn first_armed_timer(actions: &[Action]) -> Option<TimerId> {
        actions.iter().find_map(|action| match action {
            Action::ArmTimer { id, .. } => Some(*id),
            _ => None,
        })
    }

    fn settle_count(actions: &[Action], call_id: CallId) -> usize {
        actions
            .iter()
            .filter(|action| matches!(action, Action::Settle(id, _) if *id == call_id))
            .count()
    }

    /// AC-1: exceeding `queue_depth` surfaces `QueueFull { queue_depth }`
    /// before any byte is sent — visible, never absorbed (§K2).
    #[test]
    fn queue_depth_overflow_is_a_visible_queue_full() {
        let mut core = Core::new(
            KernelLimits {
                queue_depth: 1,
                ..limits()
            },
            1,
            true,
        );
        let a = core.alloc_call_id();
        let b = core.alloc_call_id();
        // Idle: both admit to the queue (nothing is Ready to flush them).
        let first = core.step(
            Input::Dispatch {
                call_id: a,
                call: erased("room.list", false, None),
            },
            Tick::ZERO,
        );
        assert!(
            find_settle(&first, a).is_none(),
            "first call is admitted, not settled"
        );
        let second = core.step(
            Input::Dispatch {
                call_id: b,
                call: erased("room.list", false, None),
            },
            Tick::ZERO,
        );
        assert!(
            matches!(
                find_settle(&second, b),
                Some(Err(CallError::QueueFull {
                    resource: "queue_depth",
                    limit: 1
                }))
            ),
            "expected QueueFull{{queue_depth}}, got {:?}",
            find_settle(&second, b).map(|r| r.as_ref().err())
        );
    }

    /// AC-1: exceeding `outbound_bytes` surfaces `QueueFull { outbound_bytes }`.
    #[test]
    fn outbound_bytes_overflow_is_a_visible_queue_full() {
        let mut core = Core::new(
            KernelLimits {
                queue_depth: 100,
                outbound_bytes: 2, // one `{}` payload fits, a second overflows
                ..limits()
            },
            1,
            true,
        );
        let a = core.alloc_call_id();
        let b = core.alloc_call_id();
        core.step(
            Input::Dispatch {
                call_id: a,
                call: erased("room.list", false, None),
            },
            Tick::ZERO,
        );
        let second = core.step(
            Input::Dispatch {
                call_id: b,
                call: erased("room.list", false, None),
            },
            Tick::ZERO,
        );
        assert!(
            matches!(
                find_settle(&second, b),
                Some(Err(CallError::QueueFull {
                    resource: "outbound_bytes",
                    limit: 2
                }))
            ),
            "expected QueueFull{{outbound_bytes}}"
        );
    }

    /// AC-2: an accepted call settles exactly once; a duplicate reply is
    /// dropped and never double-settles (§K4).
    #[test]
    fn a_reply_settles_once_and_a_duplicate_is_dropped() {
        let mut core = Core::new(limits(), 1, true);
        core.step(Input::Start, Tick::ZERO);
        core.step(
            Input::Connected {
                token: core.pending_dial().expect("dial pending"),
                incarnation: test_incarnation(),
            },
            Tick::ZERO,
        ); // → Ready, generation 1
        let call = core.alloc_call_id();
        let sent = core.step(
            Input::Dispatch {
                call_id: call,
                call: erased("room.list", false, None),
            },
            Tick::ZERO,
        );
        let wire = first_send_id(&sent).expect("a Ready connection sends immediately");

        let reply = core.step(
            Input::Inbound(Inbound::Reply {
                generation: core.generation(),
                id: jeliya_api::RequestId::new(wire).unwrap(),
                result: WireReply::Ok(RawJson::from_string(String::from("{\"rooms\":[]}"))),
            }),
            Tick::ZERO,
        );
        assert!(
            matches!(find_settle(&reply, call), Some(Ok(_))),
            "first reply settles Ok"
        );

        // A duplicate reply for the same wire id now resolves to nothing.
        let dup = core.step(
            Input::Inbound(Inbound::Reply {
                generation: core.generation(),
                id: jeliya_api::RequestId::new(wire).unwrap(),
                result: WireReply::Ok(RawJson::from_string(String::from("{\"rooms\":[]}"))),
            }),
            Tick::ZERO,
        );
        assert_eq!(
            settle_count(&dup, call),
            0,
            "a duplicate reply must not re-settle"
        );
        assert_eq!(core.in_flight_count(), 0);
    }

    /// AC-4: connection loss distinguishes never-sent (`DefinitelyNot`) from
    /// may-have-executed (`Unknown`) work (§K6).
    #[test]
    fn disconnect_classifies_never_sent_vs_may_have_executed() {
        let mut core = Core::new(
            KernelLimits {
                in_flight: 1,              // throttle so the second call stays queued
                max_reconnect_attempts: 0, // one loss is terminal
                ..limits()
            },
            1,
            true,
        );
        core.step(Input::Start, Tick::ZERO);
        core.step(
            Input::Connected {
                token: core.pending_dial().expect("dial pending"),
                incarnation: test_incarnation(),
            },
            Tick::ZERO,
        );
        let sent = core.alloc_call_id();
        let queued = core.alloc_call_id();
        let flush = core.step(
            Input::Dispatch {
                call_id: sent,
                call: erased("room.list", false, None),
            },
            Tick::ZERO,
        );
        assert!(first_send_id(&flush).is_some(), "first call is sent");
        core.step(
            Input::Dispatch {
                call_id: queued,
                call: erased("room.list", false, None),
            },
            Tick::ZERO,
        );

        let lost = core.step(
            Input::Interrupted {
                generation: core.generation(),
            },
            Tick::ZERO,
        );
        assert!(
            matches!(
                find_settle(&lost, sent),
                Some(Err(CallError::Disconnected {
                    execution: Execution::Unknown
                }))
            ),
            "sent call may have executed"
        );
        assert!(
            matches!(
                find_settle(&lost, queued),
                Some(Err(CallError::Disconnected {
                    execution: Execution::DefinitelyNot
                }))
            ),
            "queued call provably never executed"
        );
        assert_eq!(core.state(), State::Failed);
    }

    /// AC-6: a stale-generation reply is fenced; the call stays outstanding and
    /// only its current-generation reply settles it (§K7). A `mutating + op_id`
    /// call is held across the reconnect (§K5).
    #[test]
    fn generation_fencing_drops_a_stale_reply_and_replays_the_held_call() {
        let mut core = Core::new(
            KernelLimits {
                max_reconnect_attempts: 4,
                ..limits()
            },
            1,
            true,
        );
        core.step(Input::Start, Tick::ZERO);
        core.step(
            Input::Connected {
                token: core.pending_dial().expect("dial pending"),
                incarnation: test_incarnation(),
            },
            Tick::ZERO,
        ); // generation 1
        let call = core.alloc_call_id();
        let first_send = core.step(
            Input::Dispatch {
                call_id: call,
                call: erased("message.send", true, Some("op-1")),
            },
            Tick::ZERO,
        );
        let gen1 = core.generation();
        let wire1 = first_send_id(&first_send).expect("sent on generation 1");

        // Lose the connection: the replayable call is held, not settled.
        let lost = core.step(
            Input::Interrupted {
                generation: core.generation(),
            },
            Tick::ZERO,
        );
        assert_eq!(
            settle_count(&lost, call),
            0,
            "a replayable call is held, not settled"
        );
        assert_eq!(core.replay_hold_len(), 1);

        // The backoff fires and the retry dial completes: the held call
        // re-sends under generation 2 (a completion before the timer fires
        // would be a stale straggler and is dropped).
        let backoff = first_armed_timer(&lost).expect("backoff armed");
        core.step(Input::TimerFired(backoff), Tick(1));
        let reconnect = core.step(
            Input::Connected {
                token: core.pending_dial().expect("dial pending"),
                incarnation: test_incarnation(),
            },
            Tick(1),
        );
        let gen2 = core.generation();
        assert_eq!(gen2, gen1 + 1);
        let wire2 = first_send_id(&reconnect).expect("held call re-sends on reconnect");

        // A stale generation-1 reply is fenced and drops.
        let stale = core.step(
            Input::Inbound(Inbound::Reply {
                generation: gen1,
                id: jeliya_api::RequestId::new(wire1).unwrap(),
                result: WireReply::Ok(RawJson::from_string(String::from("{}"))),
            }),
            Tick::ZERO,
        );
        assert_eq!(
            settle_count(&stale, call),
            0,
            "a stale-generation reply must be fenced"
        );

        // The current-generation reply settles it exactly once.
        let fresh = core.step(
            Input::Inbound(Inbound::Reply {
                generation: gen2,
                id: jeliya_api::RequestId::new(wire2).unwrap(),
                result: WireReply::Err(ApiError::NotReady),
            }),
            Tick::ZERO,
        );
        assert!(matches!(
            find_settle(&fresh, call),
            Some(Err(CallError::Wire(ApiError::NotReady)))
        ));
    }

    /// #270 D6: a keyed mutation held across a reconnect is **dropped**
    /// (settling `Disconnected { Unknown }`), not re-sent, when the daemon
    /// incarnation changed — a restarted daemon's in-memory dedup ledger is
    /// empty, so a replay would re-execute. The held entry leaves the ledger
    /// and its full admission charge is released.
    #[test]
    fn a_changed_daemon_incarnation_drops_the_held_call_unknown() {
        let mut core = Core::new(
            KernelLimits {
                max_reconnect_attempts: 4,
                ..limits()
            },
            1,
            true,
        );
        core.step(Input::Start, Tick::ZERO);
        core.step(
            Input::Connected {
                token: core.pending_dial().expect("dial pending"),
                incarnation: inc("inc-a"),
            },
            Tick::ZERO,
        );
        let call = core.alloc_call_id();
        core.step(
            Input::Dispatch {
                call_id: call,
                call: erased("message.send", true, Some("op-1")),
            },
            Tick::ZERO,
        );
        let lost = core.step(
            Input::Interrupted {
                generation: core.generation(),
            },
            Tick::ZERO,
        );
        assert_eq!(core.replay_hold_len(), 1, "the keyed mutation is held");
        let backoff = first_armed_timer(&lost).expect("backoff armed");
        core.step(Input::TimerFired(backoff), Tick(1));

        // Reconnect to a DIFFERENT incarnation: the daemon restarted.
        let reconnect = core.step(
            Input::Connected {
                token: core.pending_dial().expect("dial pending"),
                incarnation: inc("inc-b"),
            },
            Tick(1),
        );
        assert!(
            matches!(
                find_settle(&reconnect, call),
                Some(Err(CallError::Disconnected {
                    execution: Execution::Unknown
                }))
            ),
            "a restarted daemon settles the held call Unknown, never replays it"
        );
        assert!(
            first_send_id(&reconnect).is_none(),
            "the dropped call is not re-sent against the empty ledger"
        );
        assert_eq!(core.replay_hold_len(), 0);
        assert_eq!(core.ledger_len(), 0, "the held entry is removed");
        assert_eq!(core.state(), State::Ready);
    }

    /// #270 D6 (the mixed case): on an incarnation change the previously-sent
    /// held call is dropped `Unknown`, while a never-sent queued call — which
    /// provably executed nowhere — flushes normally on the new connection.
    #[test]
    fn a_changed_incarnation_drops_the_held_but_flushes_never_sent_queued() {
        let mut core = Core::new(
            KernelLimits {
                in_flight: 1, // throttle so the second call stays queued behind the sent one
                max_reconnect_attempts: 4,
                ..limits()
            },
            1,
            true,
        );
        core.step(Input::Start, Tick::ZERO);
        core.step(
            Input::Connected {
                token: core.pending_dial().expect("dial pending"),
                incarnation: inc("inc-a"),
            },
            Tick::ZERO,
        );
        let held = core.alloc_call_id();
        let queued = core.alloc_call_id();
        let flush = core.step(
            Input::Dispatch {
                call_id: held,
                call: erased("message.send", true, Some("op-1")),
            },
            Tick::ZERO,
        );
        assert!(
            first_send_id(&flush).is_some(),
            "the first keyed mutation sends"
        );
        core.step(
            Input::Dispatch {
                call_id: queued,
                call: erased("message.send", true, Some("op-2")),
            },
            Tick::ZERO,
        );
        let lost = core.step(
            Input::Interrupted {
                generation: core.generation(),
            },
            Tick::ZERO,
        );
        assert_eq!(core.replay_hold_len(), 1, "only the sent call is held");
        assert_eq!(core.queued_len(), 1, "the never-sent call stays queued");
        let backoff = first_armed_timer(&lost).expect("backoff armed");
        core.step(Input::TimerFired(backoff), Tick(1));

        let reconnect = core.step(
            Input::Connected {
                token: core.pending_dial().expect("dial pending"),
                incarnation: inc("inc-b"),
            },
            Tick(1),
        );
        assert!(
            matches!(
                find_settle(&reconnect, held),
                Some(Err(CallError::Disconnected {
                    execution: Execution::Unknown
                }))
            ),
            "the previously-sent held call is dropped Unknown"
        );
        assert_eq!(
            settle_count(&reconnect, queued),
            0,
            "the never-sent queued call is not settled — it flushes"
        );
        assert!(
            first_send_id(&reconnect).is_some(),
            "the never-sent queued call sends on the new incarnation"
        );
        assert_eq!(core.replay_hold_len(), 0);
        assert_eq!(core.ledger_len(), 1, "only the flushed queued call remains");
        assert_eq!(core.state(), State::Ready);
    }

    /// A non-replayable mutation (no `op_id`) is never re-sent; it settles on
    /// disconnect (§K5, the "all others never auto-replay" rule).
    #[test]
    fn a_mutation_without_op_id_never_replays() {
        let mut core = Core::new(limits(), 1, true);
        core.step(Input::Start, Tick::ZERO);
        core.step(
            Input::Connected {
                token: core.pending_dial().expect("dial pending"),
                incarnation: test_incarnation(),
            },
            Tick::ZERO,
        );
        let call = core.alloc_call_id();
        core.step(
            Input::Dispatch {
                call_id: call,
                call: erased("message.send", true, None),
            },
            Tick::ZERO,
        );
        let lost = core.step(
            Input::Interrupted {
                generation: core.generation(),
            },
            Tick::ZERO,
        );
        // Sent, non-replayable → settled Unknown, not held.
        assert!(
            matches!(
                find_settle(&lost, call),
                Some(Err(CallError::Disconnected {
                    execution: Execution::Unknown
                }))
            ),
            "a no-op_id mutation settles rather than replays"
        );
        assert_eq!(core.replay_hold_len(), 0);
    }

    /// AC-5/AC-7: total stop settles queued and in-flight calls and leaves no
    /// unbounded map behind; a second stop is idempotent (§K11).
    #[test]
    fn stop_settles_everything_and_empties_every_collection() {
        let mut core = Core::new(
            KernelLimits {
                in_flight: 1,
                ..limits()
            },
            1,
            true,
        );
        core.step(Input::Start, Tick::ZERO);
        core.step(
            Input::Connected {
                token: core.pending_dial().expect("dial pending"),
                incarnation: test_incarnation(),
            },
            Tick::ZERO,
        );
        let sent = core.alloc_call_id();
        let queued = core.alloc_call_id();
        core.step(
            Input::Dispatch {
                call_id: sent,
                call: erased("room.list", false, None),
            },
            Tick::ZERO,
        );
        core.step(
            Input::Dispatch {
                call_id: queued,
                call: erased("room.list", false, None),
            },
            Tick::ZERO,
        );

        let stopped = core.step(Input::Stop, Tick::ZERO);
        assert!(
            matches!(
                find_settle(&stopped, sent),
                Some(Err(CallError::Cancelled {
                    execution: Execution::Unknown
                }))
            ),
            "a sent call stops as Cancelled{{Unknown}}"
        );
        assert!(
            matches!(
                find_settle(&stopped, queued),
                Some(Err(CallError::Cancelled {
                    execution: Execution::DefinitelyNot
                }))
            ),
            "a queued call stops as Cancelled{{DefinitelyNot}}"
        );
        assert!(
            stopped.iter().any(|a| matches!(a, Action::CloseBus)),
            "the bus closes"
        );
        assert_eq!(core.state(), State::Stopped);
        // No unbounded task or map survives.
        assert_eq!(core.ledger_len(), 0);
        assert_eq!(core.queued_len(), 0);
        assert_eq!(core.in_flight_count(), 0);
        assert_eq!(core.replay_hold_len(), 0);

        // A second stop is a no-op.
        let again = core.step(Input::Stop, Tick::ZERO);
        assert!(
            again.is_empty(),
            "a second stop produces no further actions"
        );

        // A call after stop is refused DefinitelyNot.
        let late = core.alloc_call_id();
        let refused = core.step(
            Input::Dispatch {
                call_id: late,
                call: erased("room.list", false, None),
            },
            Tick::ZERO,
        );
        assert!(matches!(
            find_settle(&refused, late),
            Some(Err(CallError::Cancelled {
                execution: Execution::DefinitelyNot
            }))
        ));
    }

    /// Verification (timeout): a sent call whose deadline fires settles
    /// `Timeout` and tombstones its id, so a late reply is absorbed rather than
    /// mis-routed (§K8).
    #[test]
    fn a_sent_call_times_out_and_absorbs_its_late_reply() {
        let mut core = Core::new(
            KernelLimits {
                default_call_deadline: TickDelta(10),
                ..limits()
            },
            1,
            true,
        );
        core.step(Input::Start, Tick::ZERO);
        core.step(
            Input::Connected {
                token: core.pending_dial().expect("dial pending"),
                incarnation: test_incarnation(),
            },
            Tick::ZERO,
        );
        let call = core.alloc_call_id();
        let sent = core.step(
            Input::Dispatch {
                call_id: call,
                call: erased("room.list", false, None),
            },
            Tick::ZERO,
        );
        let wire = first_send_id(&sent).expect("sent");
        let deadline = first_armed_timer(&sent).expect("a deadline timer is armed");

        let fired = core.step(Input::TimerFired(deadline), Tick(10));
        assert!(matches!(
            find_settle(&fired, call),
            Some(Err(CallError::Timeout))
        ));
        assert_eq!(core.in_flight_count(), 0, "the in-flight slot is freed");

        // The tombstone absorbs a late real reply without re-settling.
        let late = core.step(
            Input::Inbound(Inbound::Reply {
                generation: core.generation(),
                id: jeliya_api::RequestId::new(wire).unwrap(),
                result: WireReply::Ok(RawJson::from_string(String::from("{\"rooms\":[]}"))),
            }),
            Tick(11),
        );
        assert_eq!(
            settle_count(&late, call),
            0,
            "a late reply after timeout strands nothing"
        );
        assert_eq!(
            core.ledger_len(),
            0,
            "the tombstone is reclaimed by its late reply"
        );
    }

    /// Verification (cancellation, queued): a dropped future for a queued call
    /// removes it with no frame sent (§K9).
    #[test]
    fn cancelling_a_queued_call_sends_no_frame() {
        let mut core = Core::new(limits(), 1, true);
        // Idle: the call is admitted but not sent.
        let call = core.alloc_call_id();
        core.step(
            Input::Dispatch {
                call_id: call,
                call: erased("room.list", false, None),
            },
            Tick::ZERO,
        );
        let cancelled = core.step(Input::Cancel(call), Tick::ZERO);
        assert!(
            first_send_id(&cancelled).is_none(),
            "a queued cancel sends nothing"
        );
        assert_eq!(core.ledger_len(), 0);
        assert_eq!(core.queued_len(), 0);
    }

    /// Verification (reconnect exhaustion): after `max_reconnect_attempts`
    /// losses the kernel fails honestly rather than spinning (§K10).
    #[test]
    fn reconnect_exhaustion_fails_rather_than_spinning() {
        let mut core = Core::new(
            KernelLimits {
                max_reconnect_attempts: 2,
                ..limits()
            },
            1,
            true,
        );
        core.step(Input::Start, Tick::ZERO); // Connecting + Dial (attempt 1)
                                             // Each dial failure schedules a backoff whose timer fires a real
                                             // retry dial before the next failure (every outcome is token-fenced
                                             // to its own attempt). Two scheduled retries fit the budget; the
                                             // third failure exhausts it.
        let failed = core.step(
            Input::DialFailed {
                token: core.pending_dial().expect("dial pending"),
            },
            Tick::ZERO,
        );
        assert_eq!(
            core.state(),
            State::Connecting,
            "an initial-activation retry stays Connecting"
        );
        let t1 = first_armed_timer(&failed).expect("first retry scheduled");
        let dial = core.step(Input::TimerFired(t1), Tick(1_000));
        assert!(dial.iter().any(|a| matches!(a, Action::Dial { .. })));
        let failed = core.step(
            Input::DialFailed {
                token: core.pending_dial().expect("dial pending"),
            },
            Tick(1_001),
        );
        assert_eq!(core.state(), State::Connecting);
        let t2 = first_armed_timer(&failed).expect("second retry scheduled");
        let dial = core.step(Input::TimerFired(t2), Tick(2_000));
        assert!(dial.iter().any(|a| matches!(a, Action::Dial { .. })));
        let exhausted = core.step(
            Input::DialFailed {
                token: core.pending_dial().expect("dial pending"),
            },
            Tick(2_001),
        );
        assert!(
            exhausted.iter().any(|a| matches!(a, Action::CancelDial)),
            "exhaustion cancels the dial"
        );
        assert_eq!(core.state(), State::Failed);
    }

    /// A malformed inbound frame correlates to nothing and strands no call
    /// (§K4).
    #[test]
    fn a_malformed_frame_strands_nothing() {
        let mut core = Core::new(limits(), 1, true);
        core.step(Input::Start, Tick::ZERO);
        core.step(
            Input::Connected {
                token: core.pending_dial().expect("dial pending"),
                incarnation: test_incarnation(),
            },
            Tick::ZERO,
        );
        let call = core.alloc_call_id();
        core.step(
            Input::Dispatch {
                call_id: call,
                call: erased("room.list", false, None),
            },
            Tick::ZERO,
        );
        let malformed = core.step(Input::Inbound(Inbound::Malformed), Tick::ZERO);
        assert!(malformed.is_empty(), "a malformed frame produces no action");
        assert_eq!(core.ledger_len(), 1, "the outstanding call is untouched");
    }

    /// Verification (timeout, queued): a call admitted to the bounded queue
    /// whose deadline fires before it is ever sent settles `Timeout` and
    /// releases its admission charge, so a subsequent call can be admitted
    /// again (§K8). The code path through `on_timer_fired` that handles
    /// never-sent calls is distinct from the in-flight timeout path.
    #[test]
    fn timeout_while_queued_settles_timeout_and_releases_admission() {
        let mut core = Core::new(
            KernelLimits {
                queue_depth: 1, // tight: confirms the charge is released
                default_call_deadline: TickDelta(10),
                ..limits()
            },
            1,
            true,
        );
        // Idle: the call is admitted to the queue but not sent.
        let call = core.alloc_call_id();
        let admitted = core.step(
            Input::Dispatch {
                call_id: call,
                call: erased("room.list", false, None),
            },
            Tick::ZERO,
        );
        let deadline = first_armed_timer(&admitted).expect("deadline armed at admission");
        assert!(
            first_send_id(&admitted).is_none(),
            "a queued (Idle) call never sends immediately"
        );
        assert_eq!(core.ledger_len(), 1);

        // The deadline fires before the call reached the wire.
        let fired = core.step(Input::TimerFired(deadline), Tick(10));
        assert!(
            matches!(find_settle(&fired, call), Some(Err(CallError::Timeout))),
            "a queued-timeout settles Timeout, got {:?}",
            find_settle(&fired, call).map(|r| r.as_ref().err())
        );
        assert_eq!(core.ledger_len(), 0, "call removed from the ledger");
        assert_eq!(core.queued_len(), 0, "queue empty after queued-timeout");

        // The admission charge is released: the depth-1 queue accepts a new call.
        let call2 = core.alloc_call_id();
        let re_admitted = core.step(
            Input::Dispatch {
                call_id: call2,
                call: erased("room.list", false, None),
            },
            Tick(10),
        );
        assert!(
            find_settle(&re_admitted, call2).is_none(),
            "admission charge was released; next call is admitted without QueueFull"
        );
    }

    /// §K7: a terminal generation-gate refusal settles all outstanding calls as
    /// `Disconnected { DefinitelyNot }` (nothing passed the barrier) and
    /// transitions to `Failed` with no auto-retry — `Start` in `Failed` state
    /// is a no-op (§K7, gate-refused branch of AC-5).
    #[test]
    fn gate_refused_settles_all_queued_as_definitely_not_with_no_retry() {
        let mut core = Core::new(limits(), 1, true);
        core.step(Input::Start, Tick::ZERO); // → Connecting, Dial

        // Two calls queue behind the protocol-validation barrier while Connecting.
        let a = core.alloc_call_id();
        let b = core.alloc_call_id();
        core.step(
            Input::Dispatch {
                call_id: a,
                call: erased("room.list", false, None),
            },
            Tick::ZERO,
        );
        core.step(
            Input::Dispatch {
                call_id: b,
                call: erased("message.send", true, Some("op-key")),
            },
            Tick::ZERO,
        );
        assert_eq!(core.ledger_len(), 2, "both calls queued behind the gate");

        // Terminal gate refusal — no auto-retry, every call is DefinitelyNot.
        let refused = core.step(
            Input::GateRefused {
                token: core.pending_dial().expect("dial pending"),
            },
            Tick::ZERO,
        );
        assert!(
            matches!(
                find_settle(&refused, a),
                Some(Err(CallError::Disconnected {
                    execution: Execution::DefinitelyNot
                }))
            ),
            "read call is DefinitelyNot on gate refusal"
        );
        assert!(
            matches!(
                find_settle(&refused, b),
                Some(Err(CallError::Disconnected {
                    execution: Execution::DefinitelyNot
                }))
            ),
            "replayable mutation is still DefinitelyNot (nothing past the barrier)"
        );
        assert_eq!(core.state(), State::Failed);
        assert_eq!(core.ledger_len(), 0);

        // Start after Failed must not produce a Dial — no auto-retry.
        let retry = core.step(Input::Start, Tick::ZERO);
        assert!(
            !retry.iter().any(|a| matches!(a, Action::Dial { .. })),
            "Start after Failed must not initiate a new dial"
        );
        assert_eq!(core.state(), State::Failed, "state remains Failed");
    }

    /// A stale-generation push is fenced and never reaches the bus (§K7).
    #[test]
    fn a_stale_generation_push_is_fenced() {
        let mut core = Core::new(limits(), 1, true);
        core.step(Input::Start, Tick::ZERO);
        core.step(
            Input::Connected {
                token: core.pending_dial().expect("dial pending"),
                incarnation: test_incarnation(),
            },
            Tick::ZERO,
        ); // generation 1
        let push = || jeliya_api::Push::Transfer {
            transfer_op_id: jeliya_api::OpId::new("t"),
            transferred_bytes: 1,
            total: jeliya_api::ByteTotal::Unknown,
        };
        let stale = core.step(
            Input::Inbound(Inbound::Push {
                generation: 0, // older than the live generation
                push: push(),
            }),
            Tick::ZERO,
        );
        assert!(
            !stale.iter().any(|a| matches!(a, Action::Emit(_))),
            "a stale push is dropped"
        );
        let live = core.step(
            Input::Inbound(Inbound::Push {
                generation: 1,
                push: push(),
            }),
            Tick::ZERO,
        );
        assert!(
            live.iter().any(|a| matches!(a, Action::Emit(_))),
            "a live-generation push is emitted"
        );
    }

    /// §K6/§K7: a gate refusal after a replayable mutation was sent on a prior
    /// live generation settles it `Disconnected { Unknown }` — it may have
    /// executed there — while a never-sent call stays `DefinitelyNot`.
    #[test]
    fn gate_refusal_classifies_held_sent_calls_unknown() {
        let mut core = Core::new(limits(), 1, true);
        core.step(Input::Start, Tick::ZERO);
        core.step(
            Input::Connected {
                token: core.pending_dial().expect("dial pending"),
                incarnation: test_incarnation(),
            },
            Tick::ZERO,
        );
        let sent = core.alloc_call_id();
        let actions = core.step(
            Input::Dispatch {
                call_id: sent,
                call: erased("message.send", true, Some("op-1")),
            },
            Tick::ZERO,
        );
        assert!(
            actions.iter().any(|a| matches!(a, Action::Send(_))),
            "sent on generation 1"
        );
        let lost = core.step(
            Input::Interrupted {
                generation: core.generation(),
            },
            Tick::ZERO,
        );
        // Dispatched while Interrupted: admitted, never sent.
        let queued = core.alloc_call_id();
        core.step(
            Input::Dispatch {
                call_id: queued,
                call: erased("message.send", true, Some("op-2")),
            },
            Tick::ZERO,
        );
        // The retry dial reaches the gate and is terminally refused.
        let backoff = first_armed_timer(&lost).expect("backoff armed");
        core.step(Input::TimerFired(backoff), Tick(1));
        let refused = core.step(
            Input::GateRefused {
                token: core.pending_dial().expect("dial pending"),
            },
            Tick(1),
        );
        assert!(
            matches!(
                find_settle(&refused, sent),
                Some(Err(CallError::Disconnected {
                    execution: Execution::Unknown
                }))
            ),
            "a call sent on a prior live generation may have executed there"
        );
        assert!(
            matches!(
                find_settle(&refused, queued),
                Some(Err(CallError::Disconnected {
                    execution: Execution::DefinitelyNot
                }))
            ),
            "a never-sent call is provably unexecuted"
        );
        assert_eq!(core.state(), State::Failed);
        assert_eq!(core.ledger_len(), 0);
    }

    /// §K12/AC-7: a live timeout's tombstone is reclaimed by its re-armed
    /// deadline even when the peer never sends the late reply.
    #[test]
    fn timeout_tombstone_is_reclaimed_without_a_late_reply() {
        let mut core = Core::new(limits(), 1, true);
        core.step(Input::Start, Tick::ZERO);
        core.step(
            Input::Connected {
                token: core.pending_dial().expect("dial pending"),
                incarnation: test_incarnation(),
            },
            Tick::ZERO,
        );
        let call = core.alloc_call_id();
        let dispatched = core.step(
            Input::Dispatch {
                call_id: call,
                call: erased("room.list", false, None),
            },
            Tick::ZERO,
        );
        let deadline = first_armed_timer(&dispatched).expect("deadline armed");
        let deadline_at = Tick::ZERO.saturating_add(limits().default_call_deadline);
        let fired = core.step(Input::TimerFired(deadline), deadline_at);
        assert!(matches!(
            find_settle(&fired, call),
            Some(Err(CallError::Timeout))
        ));
        let reclaim =
            first_armed_timer(&fired).expect("a reclaim deadline is re-armed for the tombstone");
        assert_eq!(
            core.ledger_len(),
            1,
            "the tombstone absorbs a late reply until reclaim"
        );
        let reclaimed = core.step(
            Input::TimerFired(reclaim),
            deadline_at.saturating_add(limits().default_call_deadline),
        );
        assert_eq!(
            core.ledger_len(),
            0,
            "no late reply ever arrives: the reclaim deadline empties the ledger"
        );
        assert!(
            reclaimed.iter().all(|a| !matches!(a, Action::Settle(..))),
            "reclaim settles nothing"
        );
    }

    /// §K12/AC-7: cancel churn cannot grow the ledger past its bound — each
    /// tombstone past the `in_flight` budget evicts the oldest.
    #[test]
    fn cancel_churn_stays_within_the_tombstone_budget() {
        let mut core = Core::new(
            KernelLimits {
                in_flight: 1,
                ..limits()
            },
            1,
            true,
        );
        core.step(Input::Start, Tick::ZERO);
        core.step(
            Input::Connected {
                token: core.pending_dial().expect("dial pending"),
                incarnation: test_incarnation(),
            },
            Tick::ZERO,
        );
        for _ in 0..10 {
            let call = core.alloc_call_id();
            let actions = core.step(
                Input::Dispatch {
                    call_id: call,
                    call: erased("room.list", false, None),
                },
                Tick::ZERO,
            );
            assert!(
                actions.iter().any(|a| matches!(a, Action::Send(_))),
                "the freed slot sends immediately"
            );
            core.step(Input::Cancel(call), Tick::ZERO);
        }
        assert!(
            core.ledger_len() <= 1,
            "cancel churn keeps at most the tombstone budget (in_flight = 1), got {}",
            core.ledger_len()
        );
    }

    /// A duplicate `Connected` while already `Ready` is dropped: the generation
    /// does not bump and no sent call is stranded behind a cleared wire index.
    #[test]
    fn duplicate_connected_while_ready_is_ignored() {
        let mut core = Core::new(limits(), 1, true);
        core.step(Input::Start, Tick::ZERO);
        core.step(
            Input::Connected {
                token: core.pending_dial().expect("dial pending"),
                incarnation: test_incarnation(),
            },
            Tick::ZERO,
        );
        let call = core.alloc_call_id();
        let sent = core.step(
            Input::Dispatch {
                call_id: call,
                call: erased("room.list", false, None),
            },
            Tick::ZERO,
        );
        let wire_id = first_send_id(&sent).expect("sent while Ready");
        let generation = core.generation();
        // The original dial's token was consumed by the first completion;
        // replaying it (or any stale token) is dropped whole.
        let duplicate = core.step(
            Input::Connected {
                token: 1,
                incarnation: test_incarnation(),
            },
            Tick::ZERO,
        );
        assert!(
            duplicate.is_empty(),
            "a duplicate Connected is dropped whole"
        );
        assert_eq!(
            core.generation(),
            generation,
            "the generation does not bump"
        );
        // The original call still resolves under its issuing generation.
        let reply = core.step(
            Input::Inbound(Inbound::Reply {
                generation,
                id: jeliya_api::RequestId::new(wire_id).unwrap(),
                result: WireReply::Ok(RawJson::from_string(String::from("{}"))),
            }),
            Tick::ZERO,
        );
        assert!(
            matches!(find_settle(&reply, call), Some(Ok(_))),
            "the reply still settles the call"
        );
    }

    /// A queued call whose absolute deadline elapsed is settled `Timeout` by
    /// `flush`, never sent past its budget — the shared-deadline timer race.
    #[test]
    fn a_queued_call_past_its_deadline_is_not_sent() {
        let mut core = Core::new(
            KernelLimits {
                in_flight: 1,
                ..limits()
            },
            1,
            true,
        );
        core.step(Input::Start, Tick::ZERO);
        core.step(
            Input::Connected {
                token: core.pending_dial().expect("dial pending"),
                incarnation: test_incarnation(),
            },
            Tick::ZERO,
        );
        let sent = core.alloc_call_id();
        let queued = core.alloc_call_id();
        let first = core.step(
            Input::Dispatch {
                call_id: sent,
                call: erased("room.list", false, None),
            },
            Tick::ZERO,
        );
        let deadline = first_armed_timer(&first).expect("deadline armed");
        core.step(
            Input::Dispatch {
                call_id: queued,
                call: erased("message.send", true, None),
            },
            Tick::ZERO,
        );
        // Both deadlines are due at the same tick; the in-flight call's timer
        // processes first and frees the slot.
        let at = Tick::ZERO.saturating_add(limits().default_call_deadline);
        let fired = core.step(Input::TimerFired(deadline), at);
        assert!(matches!(
            find_settle(&fired, sent),
            Some(Err(CallError::Timeout))
        ));
        assert!(
            matches!(find_settle(&fired, queued), Some(Err(CallError::Timeout))),
            "the queued call's budget elapsed too: it settles, it is not sent"
        );
        assert!(
            !fired.iter().any(|a| matches!(a, Action::Send(_))),
            "no call goes onto the wire past its absolute budget"
        );
    }

    /// The absolute deadline binds regardless of event ordering: a reply
    /// delivered at the deadline tick settles `Timeout` with the payload
    /// dropped — the same outcome as the timer firing first — while a reply
    /// one tick earlier settles normally.
    #[test]
    fn a_reply_at_the_deadline_settles_timeout_not_the_payload() {
        let mut core = Core::new(limits(), 1, true);
        core.step(Input::Start, Tick::ZERO);
        core.step(
            Input::Connected {
                token: core.pending_dial().expect("dial pending"),
                incarnation: test_incarnation(),
            },
            Tick::ZERO,
        );
        let call = core.alloc_call_id();
        let sent = core.step(
            Input::Dispatch {
                call_id: call,
                call: erased("room.list", false, None),
            },
            Tick::ZERO,
        );
        let wire_id = first_send_id(&sent).expect("sent while Ready");
        let generation = core.generation();
        let at_deadline = Tick::ZERO.saturating_add(limits().default_call_deadline);
        let reply = core.step(
            Input::Inbound(Inbound::Reply {
                generation,
                id: jeliya_api::RequestId::new(wire_id).unwrap(),
                result: WireReply::Ok(RawJson::from_string(String::from("{}"))),
            }),
            at_deadline,
        );
        assert!(
            matches!(find_settle(&reply, call), Some(Err(CallError::Timeout))),
            "a reply at the deadline tick is late: Timeout, payload dropped"
        );
        assert_eq!(
            core.ledger_len(),
            0,
            "the late reply still reclaims the entry"
        );

        // One tick earlier the same reply settles normally.
        let mut core = Core::new(limits(), 1, true);
        core.step(Input::Start, Tick::ZERO);
        core.step(
            Input::Connected {
                token: core.pending_dial().expect("dial pending"),
                incarnation: test_incarnation(),
            },
            Tick::ZERO,
        );
        let call = core.alloc_call_id();
        let sent = core.step(
            Input::Dispatch {
                call_id: call,
                call: erased("room.list", false, None),
            },
            Tick::ZERO,
        );
        let wire_id = first_send_id(&sent).expect("sent while Ready");
        let generation = core.generation();
        let just_in_time = Tick(at_deadline.0 - 1);
        let reply = core.step(
            Input::Inbound(Inbound::Reply {
                generation,
                id: jeliya_api::RequestId::new(wire_id).unwrap(),
                result: WireReply::Ok(RawJson::from_string(String::from("{}"))),
            }),
            just_in_time,
        );
        assert!(
            matches!(find_settle(&reply, call), Some(Ok(_))),
            "one tick before the deadline the payload is delivered"
        );
    }

    /// §K10: duplicate loss signals while the backoff is armed coalesce —
    /// they must not burn reconnect attempts with no dial between them or
    /// orphan armed timers.
    #[test]
    fn duplicate_interruptions_coalesce_while_backoff_is_armed() {
        let mut core = Core::new(limits(), 1, true);
        core.step(Input::Start, Tick::ZERO);
        core.step(
            Input::Connected {
                token: core.pending_dial().expect("dial pending"),
                incarnation: test_incarnation(),
            },
            Tick::ZERO,
        );
        let first = core.step(
            Input::Interrupted {
                generation: core.generation(),
            },
            Tick::ZERO,
        );
        let backoff = first_armed_timer(&first).expect("the first loss arms the backoff");
        // Several sends observed the same closed transport: duplicates arrive.
        let dup = core.step(
            Input::Interrupted {
                generation: core.generation(),
            },
            Tick::ZERO,
        );
        assert!(
            dup.is_empty(),
            "a duplicate loss while the backoff is armed is coalesced whole"
        );
        // The armed timer still fires and dials — the retry was not consumed.
        let fired = core.step(Input::TimerFired(backoff), Tick(50));
        assert!(
            fired.iter().any(|a| matches!(a, Action::Dial { .. })),
            "the scheduled dial proceeds"
        );
        // A failure of the dial that began is a real new attempt — reported
        // as a token-fenced dial failure, not a connection loss.
        let redial_failed = core.step(
            Input::DialFailed {
                token: core.pending_dial().expect("dial pending"),
            },
            Tick(51),
        );
        assert!(
            first_armed_timer(&redial_failed).is_some(),
            "a post-dial failure schedules the next backoff"
        );
    }

    /// AC-1/§K12: the outbound byte bound covers the envelope `op_id` too —
    /// `OpId` has no length cap, so an uncharged key would evade the bound.
    #[test]
    fn oversized_dedup_keys_are_charged_against_outbound_bytes() {
        let mut core = Core::new(
            KernelLimits {
                outbound_bytes: 16,
                ..limits()
            },
            1,
            true,
        );
        let call = core.alloc_call_id();
        let huge_key = "k".repeat(64);
        let actions = core.step(
            Input::Dispatch {
                call_id: call,
                call: erased("message.send", true, Some(huge_key.as_str())),
            },
            Tick::ZERO,
        );
        assert!(
            matches!(
                find_settle(&actions, call),
                Some(Err(CallError::QueueFull {
                    resource: "outbound_bytes",
                    ..
                }))
            ),
            "a tiny input with a huge op_id must not slip past the byte bound"
        );
    }

    /// The absolute deadline binds at a gate refusal exactly as it does for
    /// replies and interrupts: an expired call settles `Timeout`, never
    /// `Disconnected`, regardless of refusal-vs-timer processing order.
    #[test]
    fn a_gate_refusal_at_the_deadline_settles_timeout() {
        let mut core = Core::new(limits(), 1, true);
        core.step(Input::Start, Tick::ZERO);
        // Queued while Connecting; the gate resolves only at the deadline.
        let call = core.alloc_call_id();
        core.step(
            Input::Dispatch {
                call_id: call,
                call: erased("room.list", false, None),
            },
            Tick::ZERO,
        );
        let at = Tick::ZERO.saturating_add(limits().default_call_deadline);
        let refused = core.step(
            Input::GateRefused {
                token: core.pending_dial().expect("dial pending"),
            },
            at,
        );
        assert!(
            matches!(find_settle(&refused, call), Some(Err(CallError::Timeout))),
            "an expired call settles Timeout even through the terminal gate path"
        );
        assert_eq!(core.state(), State::Failed);
    }

    /// The absolute deadline binds at an interrupt exactly as it does for a
    /// reply: a close processed at or past `deadline_at` settles the expired
    /// call `Timeout` — never `Disconnected`, never held for a replay past
    /// its budget — regardless of close-vs-timer processing order.
    #[test]
    fn an_interrupt_at_the_deadline_settles_timeout_not_disconnected() {
        let mut core = Core::new(limits(), 1, true);
        core.step(Input::Start, Tick::ZERO);
        core.step(
            Input::Connected {
                token: core.pending_dial().expect("dial pending"),
                incarnation: test_incarnation(),
            },
            Tick::ZERO,
        );
        // One replayable and one plain call, both sent at t0.
        let replayable = core.alloc_call_id();
        core.step(
            Input::Dispatch {
                call_id: replayable,
                call: erased("message.send", true, Some("op-exp")),
            },
            Tick::ZERO,
        );
        let plain = core.alloc_call_id();
        core.step(
            Input::Dispatch {
                call_id: plain,
                call: erased("room.list", false, None),
            },
            Tick::ZERO,
        );
        // The close arrives exactly at the shared deadline, before the timers.
        let at = Tick::ZERO.saturating_add(limits().default_call_deadline);
        let interrupted = core.step(
            Input::Interrupted {
                generation: core.generation(),
            },
            at,
        );
        assert!(
            matches!(
                find_settle(&interrupted, replayable),
                Some(Err(CallError::Timeout))
            ),
            "an expired replayable settles Timeout, it is not held past its budget"
        );
        assert!(
            matches!(
                find_settle(&interrupted, plain),
                Some(Err(CallError::Timeout))
            ),
            "an expired plain call settles Timeout, not Disconnected"
        );
        assert_eq!(core.replay_hold_len(), 0);
    }

    /// §K7: a push is emitted only on a Ready connection with a matching
    /// generation — before the first connect both sides sit at generation 0,
    /// and after an interrupt the generation is unchanged until reconnect,
    /// so equality alone would admit pre-gate or retired-transport traffic.
    #[test]
    fn pushes_outside_ready_are_fenced() {
        let mut core = Core::new(limits(), 1, true);
        let push = |generation| {
            Input::Inbound(Inbound::Push {
                generation,
                push: jeliya_api::Push::Transfer {
                    transfer_op_id: jeliya_api::OpId::new("t"),
                    transferred_bytes: 1,
                    total: jeliya_api::ByteTotal::Unknown,
                },
            })
        };
        // Idle, generation 0: a push with the matching generation is fenced.
        let idle = core.step(push(0), Tick::ZERO);
        assert!(
            idle.iter().all(|a| !matches!(a, Action::Emit(_))),
            "pre-gate traffic must not reach subscribers"
        );
        core.step(Input::Start, Tick::ZERO);
        core.step(
            Input::Connected {
                token: core.pending_dial().expect("dial pending"),
                incarnation: test_incarnation(),
            },
            Tick::ZERO,
        );
        let ready = core.step(push(core.generation()), Tick::ZERO);
        assert!(
            ready.iter().any(|a| matches!(a, Action::Emit(_))),
            "a Ready, matching-generation push is emitted"
        );
        core.step(
            Input::Interrupted {
                generation: core.generation(),
            },
            Tick::ZERO,
        );
        let retired = core.step(push(core.generation()), Tick::ZERO);
        assert!(
            retired.iter().all(|a| !matches!(a, Action::Emit(_))),
            "a retired transport's straggler must not reach subscribers"
        );
    }

    /// §K7/§K10: a completion arriving while the backoff timer is still
    /// armed is a straggler from the retired dial — no retry dial exists yet
    /// — and must not cancel the backoff, bump the generation, reset the
    /// budget, or flush held calls.
    #[test]
    fn a_completion_during_armed_backoff_is_dropped() {
        let mut core = Core::new(limits(), 1, true);
        core.step(Input::Start, Tick::ZERO);
        core.step(
            Input::Connected {
                token: core.pending_dial().expect("dial pending"),
                incarnation: test_incarnation(),
            },
            Tick::ZERO,
        );
        let generation = core.generation();
        let lost = core.step(
            Input::Interrupted {
                generation: core.generation(),
            },
            Tick::ZERO,
        );
        let backoff = first_armed_timer(&lost).expect("backoff armed");
        // The retired dial's straggler arrives before the timer fires: no
        // dial is pending, and its old token (1, from Start) is consumed.
        let straggler = core.step(
            Input::Connected {
                token: 1,
                incarnation: test_incarnation(),
            },
            Tick::ZERO,
        );
        assert!(
            straggler.is_empty(),
            "a straggler completion is dropped whole"
        );
        assert_eq!(core.generation(), generation, "the generation holds");
        // The scheduled retry still proceeds normally.
        let fired = core.step(Input::TimerFired(backoff), Tick(1));
        assert!(fired.iter().any(|a| matches!(a, Action::Dial { .. })));
        core.step(
            Input::Connected {
                token: core.pending_dial().expect("dial pending"),
                incarnation: test_incarnation(),
            },
            Tick(1),
        );
        assert_eq!(core.generation(), generation + 1);
    }

    /// The round-12 race: a delayed SECOND close from the dead connection
    /// arriving while the retry dial is active must not burn an attempt or
    /// arm a backoff that would reject the real completion — losses are
    /// Ready-only, dial outcomes are token-fenced, so the retry completes.
    #[test]
    fn a_duplicate_close_during_the_retry_dial_is_dropped() {
        let mut core = Core::new(limits(), 1, true);
        core.step(Input::Start, Tick::ZERO);
        core.step(
            Input::Connected {
                token: core.pending_dial().expect("dial pending"),
                incarnation: test_incarnation(),
            },
            Tick::ZERO,
        );
        let generation = core.generation();
        let lost = core.step(Input::Interrupted { generation }, Tick::ZERO);
        let backoff = first_armed_timer(&lost).expect("backoff armed");
        // The backoff fires: the retry dial is active, generation unchanged.
        core.step(Input::TimerFired(backoff), Tick(1));
        // The dead connection's SECOND close callback finally arrives — same
        // generation as current, but the state is not Ready: dropped whole.
        let dup = core.step(Input::Interrupted { generation }, Tick(2));
        assert!(dup.is_empty(), "the duplicate close is dropped");
        // The real retry completion is accepted — nothing rejected it.
        let reconnect = core.step(
            Input::Connected {
                token: core.pending_dial().expect("retry dial pending"),
                incarnation: test_incarnation(),
            },
            Tick(3),
        );
        assert!(
            reconnect.iter().any(|a| matches!(
                a,
                Action::Emit(ClientEvent::StateChanged {
                    to: State::Ready,
                    ..
                })
            )),
            "the retry completes normally"
        );
        assert_eq!(core.generation(), generation + 1);
    }

    /// §K2: a sent replayable reserves BYTES only — queue_depth counts
    /// admitted-but-unsent calls, and in-flight work throttles, never
    /// rejects.
    #[test]
    fn a_sent_replayable_does_not_consume_queue_depth() {
        let mut core = Core::new(
            KernelLimits {
                queue_depth: 1,
                in_flight: 2,
                ..limits()
            },
            1,
            true,
        );
        core.step(Input::Start, Tick::ZERO);
        core.step(
            Input::Connected {
                token: core.pending_dial().expect("dial pending"),
                incarnation: test_incarnation(),
            },
            Tick::ZERO,
        );
        let sent = core.alloc_call_id();
        let actions = core.step(
            Input::Dispatch {
                call_id: sent,
                call: erased("message.send", true, Some("op-qd")),
            },
            Tick::ZERO,
        );
        assert!(actions.iter().any(|a| matches!(a, Action::Send(_))));
        // The count slot freed at send: a second admission fits queue_depth=1.
        let second = core.alloc_call_id();
        let admitted = core.step(
            Input::Dispatch {
                call_id: second,
                call: erased("room.list", false, None),
            },
            Tick::ZERO,
        );
        assert!(
            !matches!(
                find_settle(&admitted, second),
                Some(Err(CallError::QueueFull { .. }))
            ),
            "a sent replayable must not consume the queue-depth slot"
        );
    }

    /// §K2: a non-replayable payload is dropped at send — it can never
    /// re-send, so retaining it would hold bytes outside the released bound.
    #[test]
    fn a_non_replayable_payload_is_dropped_at_send() {
        let mut core = Core::new(limits(), 1, true);
        core.step(Input::Start, Tick::ZERO);
        core.step(
            Input::Connected {
                token: core.pending_dial().expect("dial pending"),
                incarnation: test_incarnation(),
            },
            Tick::ZERO,
        );
        let call = core.alloc_call_id();
        core.step(
            Input::Dispatch {
                call_id: call,
                call: erased("room.list", false, None),
            },
            Tick::ZERO,
        );
        let entry = core.ledger.get(call).expect("sent entry");
        assert!(
            entry.input.as_str().is_empty(),
            "the sent non-replayable's payload is dropped"
        );
        assert!(entry.op_id.is_none(), "and its op_id is dropped");
    }

    /// §K5/§K13: held replayable calls re-send in their original send order
    /// after a reconnect — the ledger iterates in hash order, so the core
    /// sorts by `CallId` (monotonic at dispatch, FIFO queue). Note `file.share`
    /// is deliberately **absent**: it is a byte-stream op, forced to
    /// `ReplayPolicy::Never` (§S8), so it is settled on disconnect rather than
    /// held — `file.fetch` (a non-stream dedup-set mutation) stands in its place.
    #[test]
    fn replay_hold_preserves_original_send_order() {
        let mut core = Core::new(limits(), 1, true);
        core.step(Input::Start, Tick::ZERO);
        core.step(
            Input::Connected {
                token: core.pending_dial().expect("dial pending"),
                incarnation: test_incarnation(),
            },
            Tick::ZERO,
        );
        let ops: [&'static str; 8] = [
            "room.create",
            "room.leave",
            "member.remove",
            "invite.mint",
            "invite.revoke",
            "message.send",
            "status.post",
            "file.fetch",
        ];
        for op in ops {
            let call = core.alloc_call_id();
            let actions = core.step(
                Input::Dispatch {
                    call_id: call,
                    call: erased(op, true, Some(op)),
                },
                Tick::ZERO,
            );
            assert!(actions.iter().any(|a| matches!(a, Action::Send(_))));
        }
        let lost = core.step(
            Input::Interrupted {
                generation: core.generation(),
            },
            Tick::ZERO,
        );
        assert_eq!(core.replay_hold_len(), 8);
        // Fire the backoff so the retry dial is live before the completion.
        let backoff = first_armed_timer(&lost).expect("backoff armed");
        core.step(Input::TimerFired(backoff), Tick(1));
        let reconnect = core.step(
            Input::Connected {
                token: core.pending_dial().expect("dial pending"),
                incarnation: test_incarnation(),
            },
            Tick(1),
        );
        let resent: Vec<&'static str> = reconnect
            .iter()
            .filter_map(|a| match a {
                Action::Send(frame) => Some(frame.op),
                _ => None,
            })
            .collect();
        assert_eq!(
            resent,
            ops.to_vec(),
            "held mutations replay in original send order"
        );
    }
}
