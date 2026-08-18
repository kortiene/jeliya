//! The companion stream sub-core (§S1, §S4–S7, §S10, §S11): the client's
//! `OPEN/DATA/CREDIT/END/ABORT` state, credit ledger, per-stream deadline and
//! stall timers, and honest teardown — a **thin layer over the same sans-IO
//! core** that drives request/reply.
//!
//! **Byte-free by construction (§S2).** Nothing here holds a payload: a
//! producer's outbound DATA is an offset+length *grant* the driver fulfils; an
//! inbound DATA is offset+length *metadata* the driver has already routed to its
//! sink. The framing (`JBS2` header, per-kind field rules, offset arithmetic)
//! stays owned by `jeliya-codec` (#233/#242/#243) and is executed at the driver
//! boundary, never re-implemented here. A reviewer can check the rule
//! mechanically: no `Vec<u8>`/payload field and no framing constant appears in
//! this file.

use std::collections::{HashMap, VecDeque};

use crate::error::{CallError, Execution};
use crate::kernel::core::Action;
use crate::kernel::inflight::CallId;
use crate::kernel::timing::{Tick, TickDelta, TimerId};
use crate::kernel::transport::{StreamAbortReason, StreamRecordIntent};
use crate::kernel::StreamLimits;

use jeliya_api::RequestId;

/// Which half of the duplex the *client* drives, derived from the op path
/// (§S4): `file.share` uploads (the client produces bytes), `file.read`
/// downloads (the client receives bytes).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum StreamRole {
    /// `file.share`: the client sends DATA, bounded by daemon CREDIT.
    Producer,
    /// `file.read`: the client grants CREDIT and receives DATA.
    Receiver,
}

/// The per-stream lifecycle phase (§S4). `Requesting` never appears in the
/// table — a stream enters at OPEN as `Active` — but is kept as documentation of
/// the pre-OPEN ledger state the request/reply core owns.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum StreamPhase {
    /// Bytes are moving under credit; both timers may fire (§S6/§S7).
    Active,
    /// END emitted (producer) or accepted (receiver): the client awaits only the
    /// terminal Text reply. **Uncancellable** — neither timer aborts it (§S7).
    Finalizing,
    /// A settled tombstone kept so a late daemon record strands nothing (§S10);
    /// timers cancelled, reclaimed by the terminal reply or FIFO eviction.
    Retired,
}

/// One client-driven stream's byte-free control state (§S1). No file name,
/// digest, declared content type, or payload byte ever enters it (§S12).
pub(crate) struct StreamEntry {
    /// Which half the client drives.
    pub(crate) role: StreamRole,
    /// The lifecycle phase.
    pub(crate) phase: StreamPhase,
    /// The reply-correlation id (needed to author outbound records).
    wire_id: RequestId,
    /// The connection-local stream id adopted from OPEN.
    stream_id: u128,
    /// OPEN's authoritative total byte count.
    total: u64,
    /// The generation the stream opened on — its issuing generation, fenced
    /// against on every inbound record (§S10).
    generation: u64,
    /// Producer: exclusive end the daemon has granted us to send through.
    send_through: u64,
    /// Cumulative accepted high-water: daemon-accepted (producer) or
    /// sink-accepted (receiver). The stall timer re-arms only when this rises.
    accepted_through: u64,
    /// Producer: exclusive end we have already handed the driver to send.
    sent_offset: u64,
    /// Receiver: exclusive end through which we have granted the daemon credit.
    granted_through: u64,
    /// Producer: whether the byte source has reported EOF.
    source_ended: bool,
    /// The absolute-deadline timer, armed while `Active` (§S6).
    deadline_timer: Option<TimerId>,
    /// The stall timer, armed while `Active`. Rather than cancel/re-arm on every
    /// progress event, it fires on schedule and defers itself while progress is
    /// still fresh (§S7): see [`StreamTable::on_timer`].
    stall_timer: Option<TimerId>,
    /// The last accepted-progress instant — an increase in `accepted_through`
    /// (from inbound CREDIT for a producer, from `SinkAccepted` for a receiver).
    /// The stall timer measures silence against it (§S7).
    last_progress_at: Tick,
}

impl StreamEntry {
    /// The producer's send grant this credit state permits (§S5): send up to the
    /// credit ceiling, capped by the ACKNOWLEDGED-progress window and the
    /// declared total plus the daemon-authorized probe byte. The window anchors
    /// at `accepted_through`, never `sent_offset` — otherwise a CREDIT larger
    /// than the window would let the producer outrun every acknowledgement and
    /// `window_reserved()` would exceed its advertised bound (§S11). The
    /// `total + 1` headroom lets the source consume the daemon's declared+1
    /// probe credit so a true EOF is distinguished from an over-long source
    /// (which the daemon then refuses `declared_size_mismatch`). `0` means
    /// "paused for credit" and emits no `ProduceData`.
    fn producer_grant(&self, window: u64) -> u64 {
        let ceiling = self
            .send_through
            .min(self.accepted_through.saturating_add(window))
            .min(self.total.saturating_add(1));
        ceiling.saturating_sub(self.sent_offset)
    }

    /// The receiver's next credit grant (§S5): advance `send_through` to the
    /// quarantine ceiling `min(total, accepted_through + window)`. Returns the
    /// new `send_through` when it actually advances (so a redundant grant emits
    /// no record), else `None`.
    fn receiver_grant(&self, window: u64) -> Option<u64> {
        let ceiling = self.accepted_through.saturating_add(window).min(self.total);
        (ceiling > self.granted_through).then_some(ceiling)
    }

    /// The bytes currently reserved in the window (unacked in flight for a
    /// producer, outstanding credit for a receiver) — always `<= window` by the
    /// grant arithmetic, summed for the AC-7 bound assertion (§S11).
    fn window_reserved(&self) -> u64 {
        match self.role {
            StreamRole::Producer => self.sent_offset.saturating_sub(self.accepted_through),
            StreamRole::Receiver => self.granted_through.saturating_sub(self.accepted_through),
        }
    }

    /// The high-water mark to carry on an outbound ABORT (§S3): the ACCEPTED
    /// high-water for either role — a receiver's local `accepted_through`, a
    /// producer's greatest `accepted_through` observed in CREDIT (protocol
    /// §ABORT: "a receiver accepts a producer ABORT offset when it equals any
    /// `accepted_through` value that receiver previously issued"). A producer
    /// offset beyond that (`sent_offset`) is rejectable as malformed and can
    /// strand the transfer reservation.
    fn high_water(&self) -> u64 {
        self.accepted_through
    }
}

/// The absolute per-stream deadline (§S6): the fixed connect allowance plus a
/// size-aware floor-throughput term, armed once at OPEN and never re-armed. All
/// arithmetic is checked; a zero floor or zero ticks-per-second (an invalid
/// served configuration, rejected at construction — §6) degrades to the connect
/// allowance alone, where the stall timer remains the effective bound, rather
/// than dividing by zero. A `total` large enough to overflow saturates to the
/// far future, again leaving the stall timer as the bound.
pub(crate) fn stream_deadline_at(open_at: Tick, total: u64, limits: &StreamLimits) -> Tick {
    let floor = limits.transfer_floor_bits_per_second;
    let tps = limits.budget_ticks_per_second;
    let floor_term = if floor == 0 || tps == 0 {
        0
    } else {
        // ceil(total * 8 * ticks_per_second / floor_bits_per_second), in u128 so
        // the multiply cannot wrap, saturating back into a u64 tick delta.
        let numerator = (total as u128)
            .saturating_mul(8)
            .saturating_mul(tps as u128);
        u64::try_from(numerator.div_ceil(floor as u128)).unwrap_or(u64::MAX)
    };
    let budget = limits
        .transfer_connect_allowance
        .ticks()
        .saturating_add(floor_term);
    open_at.saturating_add(TickDelta::from_ticks(budget))
}

/// What the request/reply core must do with the ledger after a stream input
/// (§S1): the stream layer owns the record exchange and its timers, but the
/// terminal settlement runs through the same per-call ledger the core already
/// owns.
pub(crate) enum StreamOutcome {
    /// The record exchange advanced; the ledger entry is untouched.
    Progress,
    /// The stream failed. The core settles the ledger terminal with this error
    /// and tombstones the call so a late Text reply is absorbed (§S10). The
    /// stream layer has already emitted any ABORT and torn down its timers.
    Failed(CallError),
}

/// The bounded companion map `CallId -> StreamEntry` plus its FIFO tombstone
/// budget (§S11). Every collection is bounded by `max_concurrent_streams`, and
/// every terminal releases the entry and cancels its timers in one place.
pub(crate) struct StreamTable {
    entries: HashMap<CallId, StreamEntry>,
    /// Retired-stream ids, oldest first, evicted past the concurrency bound so a
    /// late-record absorber cannot grow without bound (§S11).
    tombstones: VecDeque<CallId>,
    max_concurrent: u32,
}

impl StreamTable {
    /// A fresh table bounded by `max_concurrent_streams`.
    pub(crate) fn new(max_concurrent: u32) -> Self {
        Self {
            entries: HashMap::new(),
            tombstones: VecDeque::new(),
            max_concurrent: max_concurrent.max(1),
        }
    }

    /// Whether a stream is already installed for `call_id`.
    pub(crate) fn contains(&self, call_id: CallId) -> bool {
        self.entries.contains_key(&call_id)
    }

    /// The role of an installed stream, if any.
    pub(crate) fn role(&self, call_id: CallId) -> Option<StreamRole> {
        self.entries.get(&call_id).map(|entry| entry.role)
    }

    /// The phase of an installed stream, if any — the core gates terminal-reply
    /// settlement on it (a success reply is only valid from `Finalizing`).
    pub(crate) fn phase_of(&self, call_id: CallId) -> Option<StreamPhase> {
        self.entries.get(&call_id).map(|entry| entry.phase)
    }

    /// The number of installed streams (active + finalizing + retired) — the
    /// AC-7 bound the controller asserts.
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// The number of armed per-stream timers (≤ 2 per `Active` stream, 0 once
    /// terminal) — the AC-7 bound the controller asserts.
    pub(crate) fn armed_timers(&self) -> usize {
        self.entries
            .values()
            .map(|entry| {
                usize::from(entry.deadline_timer.is_some())
                    + usize::from(entry.stall_timer.is_some())
            })
            .sum()
    }

    /// Total bytes reserved across every active stream's window — bounded by
    /// `len() * stream_window_bytes`, the AC-7 memory bound the controller
    /// asserts (§S11).
    pub(crate) fn window_bytes_reserved(&self) -> u64 {
        self.entries
            .values()
            .map(StreamEntry::window_reserved)
            .fold(0u64, |acc, reserved| acc.saturating_add(reserved))
    }

    /// Install a stream at OPEN (§S4): arm the two timers, and for a receiver
    /// grant the initial credit window. `deadline_timer`/`stall_timer` are
    /// freshly-allocated by the core so timer ownership stays with the core's
    /// single allocator. Returns `false` when a second OPEN arrives for an
    /// already-installed or retired stream — a bound-record fault the caller
    /// turns into a local abort (§S10).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn open(
        &mut self,
        call_id: CallId,
        role: StreamRole,
        wire_id: RequestId,
        stream_id: u128,
        total: u64,
        generation: u64,
        now: Tick,
        limits: &StreamLimits,
        deadline_timer: TimerId,
        stall_timer: TimerId,
        actions: &mut Vec<Action>,
    ) -> bool {
        if self.entries.contains_key(&call_id) {
            return false;
        }
        let mut entry = StreamEntry {
            role,
            phase: StreamPhase::Active,
            wire_id,
            stream_id,
            total,
            generation,
            send_through: 0,
            accepted_through: 0,
            sent_offset: 0,
            granted_through: 0,
            source_ended: false,
            deadline_timer: Some(deadline_timer),
            stall_timer: Some(stall_timer),
            last_progress_at: now,
        };
        actions.push(Action::ArmTimer {
            id: deadline_timer,
            at: stream_deadline_at(now, total, limits),
        });
        actions.push(Action::ArmTimer {
            id: stall_timer,
            at: now.saturating_add(limits.transfer_stall),
        });
        if role == StreamRole::Receiver {
            if total == 0 {
                // The zero-byte handshake is OPEN, CREDIT(0,0), END(0) — the
                // CREDIT is MANDATORY even though no window is needed: the
                // daemon's producer waits for it before it may send END, so
                // skipping it stalls an empty download until timeout.
                actions.push(Action::SendRecord(StreamRecordIntent::Credit {
                    id: wire_id,
                    stream_id,
                    accepted_through: 0,
                    send_through: 0,
                }));
            } else if let Some(send_through) = entry.receiver_grant(limits.stream_window_bytes) {
                // Grant the opening credit window immediately (§S4/§S5).
                entry.granted_through = send_through;
                actions.push(Action::SendRecord(StreamRecordIntent::Credit {
                    id: wire_id,
                    stream_id,
                    accepted_through: entry.accepted_through,
                    send_through,
                }));
            }
        }
        self.entries.insert(call_id, entry);
        true
    }

    /// The stream a fired timer belongs to, if any (deadline or stall).
    pub(crate) fn find_by_timer(&self, timer: TimerId) -> Option<CallId> {
        self.entries.iter().find_map(|(call_id, entry)| {
            (entry.deadline_timer == Some(timer) || entry.stall_timer == Some(timer))
                .then_some(*call_id)
        })
    }

    /// Handle a fired stream timer (§S6/§S7). The fired `timer` id is already
    /// consumed by the driver, so this method reuses it when it defers the stall
    /// timer. A `Finalizing`/`Retired` stream is immune (only the sequenced
    /// terminal settles it). For an `Active` stream: the absolute deadline always
    /// fails honestly; the stall timer fails **only if** no accepted progress has
    /// landed within `transfer_stall` of now, otherwise it re-arms itself for the
    /// remaining window (this is the "no progress since it was armed" rule
    /// expressed as a fire-time check, avoiding cancel/re-arm churn).
    pub(crate) fn on_timer(
        &mut self,
        call_id: CallId,
        timer: TimerId,
        now: Tick,
        limits: &StreamLimits,
        actions: &mut Vec<Action>,
    ) -> Option<StreamOutcome> {
        let entry = self.entries.get_mut(&call_id)?;
        let is_deadline = entry.deadline_timer == Some(timer);
        let is_stall = entry.stall_timer == Some(timer);
        if !is_deadline && !is_stall {
            // Not this stream's live timer (a stale/forgotten id).
            return None;
        }
        match entry.phase {
            StreamPhase::Finalizing | StreamPhase::Retired => {
                // Immune: the fired timer is spent; forget it and keep awaiting
                // the terminal reply.
                if is_deadline {
                    entry.deadline_timer = None;
                }
                if is_stall {
                    entry.stall_timer = None;
                }
                Some(StreamOutcome::Progress)
            }
            StreamPhase::Active if is_stall => {
                let silence = now.ticks().saturating_sub(entry.last_progress_at.ticks());
                if silence >= limits.transfer_stall.ticks() {
                    // Genuinely stalled: settle honestly with a courtesy ABORT.
                    entry.stall_timer = None;
                    Some(self.fail(
                        call_id,
                        StreamAbortReason::Cancelled,
                        CallError::Timeout,
                        actions,
                    ))
                } else {
                    // Progress landed since the timer was armed: defer it for the
                    // remaining window, reusing the now-free timer id.
                    let at = entry.last_progress_at.saturating_add(limits.transfer_stall);
                    entry.stall_timer = Some(timer);
                    actions.push(Action::ArmTimer { id: timer, at });
                    Some(StreamOutcome::Progress)
                }
            }
            StreamPhase::Active => {
                // The absolute deadline elapsed (§S6): Timeout (Unknown — the
                // transfer may have partially landed) with a courtesy ABORT.
                entry.deadline_timer = None;
                Some(self.fail(
                    call_id,
                    StreamAbortReason::Cancelled,
                    CallError::Timeout,
                    actions,
                ))
            }
        }
    }

    /// Inbound CREDIT (producer direction, §S5). Validate the cumulative
    /// invariants; on violation abort the stream locally. On valid progress,
    /// re-arm the stall timer and emit a fresh `ProduceData` grant.
    pub(crate) fn on_credit(
        &mut self,
        call_id: CallId,
        accepted_through: u64,
        send_through: u64,
        now: Tick,
        limits: &StreamLimits,
        actions: &mut Vec<Action>,
    ) -> StreamOutcome {
        let Some(entry) = self.entries.get(&call_id) else {
            return StreamOutcome::Progress;
        };
        if !matches!(entry.phase, StreamPhase::Active) {
            return StreamOutcome::Progress;
        }
        // Producer-direction only, monotonic non-decreasing, no ack beyond sent
        // (protocol §Credit). A violation is a bound-record fault → local abort.
        let valid = entry.role == StreamRole::Producer
            && accepted_through <= send_through
            && send_through >= entry.send_through
            && accepted_through >= entry.accepted_through
            && accepted_through <= entry.sent_offset;
        if !valid {
            return self.fail(
                call_id,
                StreamAbortReason::ProtocolError,
                CallError::Timeout,
                actions,
            );
        }
        let progressed = accepted_through > entry.accepted_through;
        let entry = self.entries.get_mut(&call_id).expect("checked above");
        entry.send_through = send_through;
        entry.accepted_through = accepted_through;
        if progressed {
            // Accepted progress re-bases the stall window (§S7).
            entry.last_progress_at = now;
        }
        self.pump_producer(call_id, limits, actions);
        self.maybe_finish(call_id, actions)
    }

    /// The driver produced bytes for a `ProduceData` grant (§S5): advance
    /// `sent_offset`, then (source permitting) emit END or the next grant.
    pub(crate) fn on_produced(
        &mut self,
        call_id: CallId,
        sent_through: u64,
        limits: &StreamLimits,
        actions: &mut Vec<Action>,
    ) -> StreamOutcome {
        if let Some(entry) = self.entries.get_mut(&call_id) {
            if matches!(entry.phase, StreamPhase::Active) {
                entry.sent_offset = entry.sent_offset.max(sent_through);
            }
        }
        self.pump_producer(call_id, limits, actions);
        self.maybe_finish(call_id, actions)
    }

    /// The producer's byte source reached EOF (§S4): record it and try to
    /// finish. END is emitted only once every sent byte is acknowledged.
    pub(crate) fn on_source_end(
        &mut self,
        call_id: CallId,
        total: u64,
        actions: &mut Vec<Action>,
    ) -> StreamOutcome {
        if let Some(entry) = self.entries.get_mut(&call_id) {
            entry.source_ended = true;
            // The source's own count is authoritative for the END offset.
            entry.sent_offset = entry.sent_offset.max(total);
        }
        self.maybe_finish(call_id, actions)
    }

    /// The producer's byte source failed (§S3): abort the stream honestly.
    pub(crate) fn on_source_failed(
        &mut self,
        call_id: CallId,
        actions: &mut Vec<Action>,
    ) -> StreamOutcome {
        self.fail(
            call_id,
            StreamAbortReason::SourceFailed,
            CallError::Timeout,
            actions,
        )
    }

    /// Inbound DATA (receiver direction, §S4/§S5). Validate contiguity and the
    /// granted-credit bound; on violation abort locally. Otherwise hand the
    /// range to the driver's sink.
    pub(crate) fn on_data(
        &mut self,
        call_id: CallId,
        offset: u64,
        len: u64,
        actions: &mut Vec<Action>,
    ) -> StreamOutcome {
        let Some(entry) = self.entries.get(&call_id) else {
            return StreamOutcome::Progress;
        };
        if !matches!(entry.phase, StreamPhase::Active) {
            return StreamOutcome::Progress;
        }
        let end = offset.saturating_add(len);
        // Contiguous from the accepted high-water, within granted credit, and
        // within the declared total (protocol §DATA). A discontinuity or an
        // over-credit / oversized record is a bound-record fault → local abort.
        let valid = entry.role == StreamRole::Receiver
            && len > 0
            && offset == entry.accepted_through
            && end <= entry.granted_through
            && end <= entry.total;
        if !valid {
            return self.fail(
                call_id,
                StreamAbortReason::ProtocolError,
                CallError::Timeout,
                actions,
            );
        }
        actions.push(Action::WriteSink {
            id: entry.wire_id,
            call_id,
            offset,
            len,
        });
        StreamOutcome::Progress
    }

    /// The driver's sink accepted a DATA range (§S5): advance the accepted
    /// high-water, re-arm the stall timer, and grant more credit.
    pub(crate) fn on_sink_accepted(
        &mut self,
        call_id: CallId,
        through: u64,
        now: Tick,
        limits: &StreamLimits,
        actions: &mut Vec<Action>,
    ) -> StreamOutcome {
        let progressed = {
            let Some(entry) = self.entries.get_mut(&call_id) else {
                return StreamOutcome::Progress;
            };
            if !matches!(entry.phase, StreamPhase::Active) {
                return StreamOutcome::Progress;
            }
            let progressed = through > entry.accepted_through;
            entry.accepted_through = entry.accepted_through.max(through);
            if progressed {
                // Accepted progress re-bases the stall window (§S7).
                entry.last_progress_at = now;
            }
            progressed
        };
        if progressed {
            // Extend credit as the quarantine drains — and REPORT the
            // acceptance. The terminal CREDIT is not optional: a live
            // producer may END "only after every DATA byte it sent has been
            // acknowledged by CREDIT" (protocol §END), and the receiver's
            // ceiling is capped at `total`, so the final acceptance often
            // cannot extend the window. Sending CREDIT on every accepted
            // advance (cumulative, monotonic — an identical repeat is
            // idempotent per §Credit) is what unblocks the producer's END;
            // without it a window-covered stream accepts every byte and
            // still stalls to timeout (found live against jeliyad).
            if let Some(entry) = self.entries.get_mut(&call_id) {
                let ceiling = entry
                    .receiver_grant(limits.stream_window_bytes)
                    .unwrap_or(entry.granted_through.max(entry.accepted_through));
                entry.granted_through = entry.granted_through.max(ceiling);
                let (wire_id, stream_id, accepted_through, send_through) = (
                    entry.wire_id,
                    entry.stream_id,
                    entry.accepted_through,
                    entry.granted_through,
                );
                actions.push(Action::SendRecord(StreamRecordIntent::Credit {
                    id: wire_id,
                    stream_id,
                    accepted_through,
                    send_through,
                }));
            }
        }
        StreamOutcome::Progress
    }

    /// The driver's sink failed (§S3): abort the stream honestly.
    pub(crate) fn on_sink_failed(
        &mut self,
        call_id: CallId,
        actions: &mut Vec<Action>,
    ) -> StreamOutcome {
        self.fail(
            call_id,
            StreamAbortReason::SinkFailed,
            CallError::Timeout,
            actions,
        )
    }

    /// The driver decoded a structurally malformed nonterminal DATA/CREDIT on
    /// this active binding (protocol §Malformed stream records): abort only
    /// this stream with the closed `protocol_error` tag — the stream-local
    /// half of the codec's severity rule. A late fault for a stream already
    /// terminal is absorbed by the retired entry.
    pub(crate) fn on_protocol_fault(
        &mut self,
        call_id: CallId,
        actions: &mut Vec<Action>,
    ) -> StreamOutcome {
        self.fail(
            call_id,
            StreamAbortReason::ProtocolError,
            CallError::Timeout,
            actions,
        )
    }

    /// Inbound END (receiver direction, §S4): accept only once the full byte
    /// sequence is sink-accepted and the count agrees, then enter FINALIZING
    /// (timers cancelled) to await the terminal Text reply.
    pub(crate) fn on_end(
        &mut self,
        call_id: CallId,
        offset: u64,
        actions: &mut Vec<Action>,
    ) -> StreamOutcome {
        let Some(entry) = self.entries.get(&call_id) else {
            return StreamOutcome::Progress;
        };
        if !matches!(entry.phase, StreamPhase::Active) {
            return StreamOutcome::Progress;
        }
        // END is a terminal commitment: the receiver keeps bytes quarantined
        // until END and the accepted count agree (§S4).
        let valid = entry.role == StreamRole::Receiver
            && offset == entry.total
            && entry.accepted_through == entry.total;
        if !valid {
            return self.fail(
                call_id,
                StreamAbortReason::ProtocolError,
                CallError::Timeout,
                actions,
            );
        }
        self.finalize(call_id, actions);
        StreamOutcome::Progress
    }

    /// Inbound ABORT from the daemon (§S4): ACK it, then await the daemon's
    /// authoritative Text reply — it carries the exact terminal error
    /// (`file_too_large`, `declared_size_mismatch`, …) and is the ONLY honest
    /// settlement for a daemon-originated abort. Settling `Cancelled` here
    /// would discard that reply into the ledger tombstone and lose the real
    /// error. A daemon ABORT that crosses our own outbound ABORT (we are
    /// already `Retired`) is still ACKed: the protocol requires BOTH sides to
    /// complete the ACK exchange in that race, or the daemon waits out
    /// `transfer_stall_ms` and closes the connection (`4007`), disrupting
    /// unrelated requests.
    pub(crate) fn on_abort(&mut self, call_id: CallId, actions: &mut Vec<Action>) -> StreamOutcome {
        let Some(entry) = self.entries.get(&call_id) else {
            return StreamOutcome::Progress;
        };
        let (wire_id, stream_id, high_water) = (entry.wire_id, entry.stream_id, entry.high_water());
        match entry.phase {
            StreamPhase::Retired => {
                // The crossing-ABORT race: our ABORT is already out and the
                // ledger already settled; complete the ACK exchange only.
                actions.push(Action::SendRecord(StreamRecordIntent::Ack {
                    id: wire_id,
                    stream_id,
                    high_water,
                }));
                StreamOutcome::Progress
            }
            StreamPhase::Finalizing => {
                // FINALIZING is uncancellable (§S7) — including from the daemon's
                // side: only the terminal reply settles it. A late ABORT here is a
                // protocol anomaly, dropped like a record to a Retired tombstone;
                // settling it would double-settle against the in-flight reply.
                StreamOutcome::Progress
            }
            StreamPhase::Active => {
                // ACK first (the daemon sends its exact error reply only after
                // the ACK), then await that reply with FINALIZING immunity:
                // timers cancelled, only the reply settles the ledger.
                actions.push(Action::SendRecord(StreamRecordIntent::Ack {
                    id: wire_id,
                    stream_id,
                    high_water,
                }));
                self.finalize(call_id, actions);
                StreamOutcome::Progress
            }
        }
    }

    /// Inbound ACK (§S4): valid ONLY in response to a client ABORT — i.e. for a
    /// `Retired` stream whose ABORT is outstanding — where it completes the
    /// exchange. An ACK addressed to an `Active` stream is a protocol fault:
    /// silently retiring the stream would orphan the ledger entry (whose base
    /// deadline no longer governs it) and hang the call forever, so the stream
    /// is aborted `protocol_error` and settled honestly instead.
    pub(crate) fn on_ack(&mut self, call_id: CallId, actions: &mut Vec<Action>) -> StreamOutcome {
        match self.entries.get(&call_id).map(|entry| entry.phase) {
            Some(StreamPhase::Retired) => {
                self.retire_to_tombstone(call_id, actions);
                StreamOutcome::Progress
            }
            Some(StreamPhase::Active) => self.abort_protocol(call_id, actions),
            // No such stream, or FINALIZING (we sent no ABORT — a spurious
            // record; dropping it cannot strand anything: the terminal reply
            // still settles the call).
            _ => StreamOutcome::Progress,
        }
    }

    /// A caller cancelled an `Active` stream (§S9): emit a courtesy ABORT and
    /// settle `Cancelled` with the kernel-derived execution (`Unknown` past
    /// OPEN). Returns `None` if there is no installed stream (the pre-OPEN case
    /// the request/reply core handles).
    pub(crate) fn on_cancel(
        &mut self,
        call_id: CallId,
        actions: &mut Vec<Action>,
    ) -> Option<StreamOutcome> {
        let entry = self.entries.get(&call_id)?;
        if matches!(entry.phase, StreamPhase::Retired) {
            return None;
        }
        if matches!(entry.phase, StreamPhase::Finalizing) {
            // FINALIZING is uncancellable (§S7); only the terminal reply settles.
            return Some(StreamOutcome::Progress);
        }
        let outcome = self.fail(
            call_id,
            StreamAbortReason::Cancelled,
            CallError::Cancelled {
                execution: Execution::Unknown,
            },
            actions,
        );
        Some(outcome)
    }

    /// Abort a stream on a bound-record protocol fault (§S10) — a duplicate
    /// OPEN, a credit regression, an oversized/discontinuous DATA. Emits a
    /// `protocol_error` ABORT and fails the call; a concurrent stream and
    /// ordinary requests are untouched.
    pub(crate) fn abort_protocol(
        &mut self,
        call_id: CallId,
        actions: &mut Vec<Action>,
    ) -> StreamOutcome {
        self.fail(
            call_id,
            StreamAbortReason::ProtocolError,
            CallError::Timeout,
            actions,
        )
    }

    /// Cleanly remove a stream after its terminal Text reply settled it, or when
    /// its ledger tombstone is reclaimed (§S10): cancel any live timers and drop
    /// the entry and its tombstone slot.
    pub(crate) fn retire(&mut self, call_id: CallId, actions: &mut Vec<Action>) {
        if let Some(entry) = self.entries.remove(&call_id) {
            cancel_timers(&entry, actions);
        }
        self.tombstones.retain(|&t| t != call_id);
    }

    /// Tear down every stream on connection loss or total stop (§S10/§S11): a
    /// stream never survives its connection. Cancels all timers and empties the
    /// table and tombstone budget.
    pub(crate) fn drain_all(&mut self, actions: &mut Vec<Action>) {
        for entry in self.entries.values() {
            cancel_timers(entry, actions);
        }
        self.entries.clear();
        self.tombstones.clear();
    }

    // -- internals ----------------------------------------------------------

    /// Emit the next producer grant if the credit/window arithmetic permits.
    fn pump_producer(&mut self, call_id: CallId, limits: &StreamLimits, actions: &mut Vec<Action>) {
        let Some(entry) = self.entries.get(&call_id) else {
            return;
        };
        if entry.role != StreamRole::Producer || !matches!(entry.phase, StreamPhase::Active) {
            return;
        }
        let up_to = entry.producer_grant(limits.stream_window_bytes);
        if up_to > 0 || entry.total == 0 {
            // A zero-total source is granted a zero-length read: the driver's
            // `read_at(0, &mut [])` observes EOF and reports `SourceEnd{0}`,
            // which is the only path to the mandatory END(0) — without this
            // grant an empty `file.share` stalls until timeout (the driver
            // never touches an ungranted source).
            actions.push(Action::ProduceData {
                id: entry.wire_id,
                call_id,
                up_to,
            });
        }
    }

    /// A producer emits END once the source has ended and every sent byte is
    /// acknowledged (§S4); it then enters FINALIZING.
    fn maybe_finish(&mut self, call_id: CallId, actions: &mut Vec<Action>) -> StreamOutcome {
        let Some(entry) = self.entries.get(&call_id) else {
            return StreamOutcome::Progress;
        };
        if entry.role != StreamRole::Producer || !matches!(entry.phase, StreamPhase::Active) {
            return StreamOutcome::Progress;
        }
        if entry.source_ended && entry.accepted_through == entry.sent_offset {
            let (wire_id, stream_id, offset) = (entry.wire_id, entry.stream_id, entry.sent_offset);
            actions.push(Action::SendRecord(StreamRecordIntent::End {
                id: wire_id,
                stream_id,
                offset,
            }));
            self.finalize(call_id, actions);
        }
        StreamOutcome::Progress
    }

    /// Enter FINALIZING: cancel both timers (§S7 immunity) and await the reply.
    fn finalize(&mut self, call_id: CallId, actions: &mut Vec<Action>) {
        if let Some(entry) = self.entries.get_mut(&call_id) {
            cancel_timers(entry, actions);
            entry.deadline_timer = None;
            entry.stall_timer = None;
            entry.phase = StreamPhase::Finalizing;
        }
    }

    /// Fail an active stream: emit a courtesy ABORT, tear down its timers, and
    /// become a tombstone. Returns the outcome the core settles the ledger with.
    fn fail(
        &mut self,
        call_id: CallId,
        reason: StreamAbortReason,
        error: CallError,
        actions: &mut Vec<Action>,
    ) -> StreamOutcome {
        if let Some(entry) = self.entries.get(&call_id) {
            let (wire_id, stream_id, high_water) =
                (entry.wire_id, entry.stream_id, entry.high_water());
            actions.push(Action::SendRecord(StreamRecordIntent::Abort {
                id: wire_id,
                stream_id,
                high_water,
                reason,
            }));
        }
        self.retire_to_tombstone(call_id, actions);
        StreamOutcome::Failed(error)
    }

    /// Move a stream to the `Retired` tombstone state: cancel its timers, mark
    /// it, and enforce the FIFO tombstone budget (§S11).
    fn retire_to_tombstone(&mut self, call_id: CallId, actions: &mut Vec<Action>) {
        if let Some(entry) = self.entries.get_mut(&call_id) {
            cancel_timers(entry, actions);
            entry.deadline_timer = None;
            entry.stall_timer = None;
            entry.phase = StreamPhase::Retired;
        } else {
            return;
        }
        self.tombstones.retain(|&t| t != call_id);
        if self.tombstones.len() >= self.max_concurrent as usize {
            if let Some(oldest) = self.tombstones.pop_front() {
                self.entries.remove(&oldest);
            }
        }
        self.tombstones.push_back(call_id);
    }
}

/// Cancel both of a stream's timers if armed.
fn cancel_timers(entry: &StreamEntry, actions: &mut Vec<Action>) {
    if let Some(timer) = entry.deadline_timer {
        actions.push(Action::CancelTimer(timer));
    }
    if let Some(timer) = entry.stall_timer {
        actions.push(Action::CancelTimer(timer));
    }
}
