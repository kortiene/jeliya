//! Connection-local protocol-v2 `file.read` producer runtime.

use std::collections::{HashMap, HashSet, VecDeque};
use std::num::NonZeroUsize;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering},
    Arc, Mutex,
};

use jeliya_api::{ApiError, ByteTotal, FileRead, FileReadOut, StreamAbortReason};
use jeliya_codec::{
    decode_stream_identity, decode_stream_kind, decode_stream_record, encode_stream_record,
    BinaryAbortReason, CodecBounds, StreamCodecError, StreamIdentity, StreamRecord,
    StreamRecordBody, StreamRecordKind, STREAM_HEADER_BYTES,
};
use jeliya_core::engine::Engine;
use tokio::sync::watch;
use tokio::sync::Notify;
use tokio::time::{Duration, Instant};

use crate::outbound::{Outbound, PendingWrite, WriteReceipt};
use crate::transfer::{RuntimeLimits, StreamIdGenerator, TransferPool, TransferReservation};

const CONTROL_ATTEMPT_QUEUED: u8 = 0;
const CONTROL_ATTEMPT_STARTED: u8 = 1;
const CONTROL_ATTEMPT_CANCELLED: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClientAbortAckCancel {
    Committed,
    ProtocolFault,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClientAbortAckOutcome {
    Sent,
    ProtocolFaultBeforeSent,
    ProtocolFaultAfterSent,
    Failed,
}

/// Admission result for one connection-local request identifier.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RequestAdmissionError {
    /// The client reused an identifier whose terminal Text reply is pending.
    Duplicate,
    /// The served request-capacity limit is already consumed.
    Exhausted(ApiError),
}

/// Connection-local outstanding request identifiers.
#[derive(Clone)]
pub(crate) struct RequestTracker {
    inner: Arc<RequestTrackerInner>,
}

struct RequestTrackerInner {
    limit: u64,
    ids: Mutex<HashSet<u64>>,
}

/// RAII ownership of one outstanding request identifier.
pub(crate) struct RequestPermit {
    tracker: Arc<RequestTrackerInner>,
    id: u64,
}

#[derive(Clone)]
struct RequestLease {
    permit: Arc<Mutex<Option<RequestPermit>>>,
}

impl RequestLease {
    fn new(permit: RequestPermit) -> Self {
        Self {
            permit: Arc::new(Mutex::new(Some(permit))),
        }
    }

    fn release(&self) {
        drop(self.permit.lock().expect("request lease poisoned").take());
    }
}

impl RequestTracker {
    pub(crate) fn new(limit: u64) -> Self {
        Self {
            inner: Arc::new(RequestTrackerInner {
                limit,
                ids: Mutex::new(HashSet::new()),
            }),
        }
    }

    pub(crate) fn acquire(&self, id: u64) -> Result<RequestPermit, RequestAdmissionError> {
        let mut ids = self.inner.ids.lock().expect("request tracker poisoned");
        if ids.contains(&id) {
            return Err(RequestAdmissionError::Duplicate);
        }
        let used = u64::try_from(ids.len()).unwrap_or(u64::MAX);
        if used >= self.inner.limit {
            return Err(RequestAdmissionError::Exhausted(
                ApiError::ResourceExhausted {
                    resource: "max_inflight_requests".into(),
                    limit: self.inner.limit,
                },
            ));
        }
        ids.insert(id);
        drop(ids);
        Ok(RequestPermit {
            tracker: self.inner.clone(),
            id,
        })
    }

    pub(crate) fn is_outstanding(&self, id: u64) -> bool {
        self.inner
            .ids
            .lock()
            .expect("request tracker poisoned")
            .contains(&id)
    }
}

impl Drop for RequestPermit {
    fn drop(&mut self) {
        self.tracker
            .ids
            .lock()
            .expect("request tracker poisoned")
            .remove(&self.id);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum BindingPhase {
    Opening = 0,
    Active = 1,
    DaemonAbortQueued = 2,
    DaemonAbortWaitAck = 3,
    AckPending = 4,
    ClientAbortPending = 5,
    ClientAbortAckCommitted = 6,
    ClientAbortAckSent = 7,
    Finalizing = 8,
    Retired = 9,
}

impl BindingPhase {
    fn load(value: &AtomicU8) -> Self {
        match value.load(Ordering::Acquire) {
            0 => Self::Opening,
            1 => Self::Active,
            2 => Self::DaemonAbortQueued,
            3 => Self::DaemonAbortWaitAck,
            4 => Self::AckPending,
            5 => Self::ClientAbortPending,
            6 => Self::ClientAbortAckCommitted,
            7 => Self::ClientAbortAckSent,
            8 => Self::Finalizing,
            _ => Self::Retired,
        }
    }
}

#[derive(Debug)]
enum MailboxEventBody {
    Record(StreamRecordBody),
    Malformed,
}

#[derive(Debug)]
struct MailboxEvent {
    received_at: Instant,
    body: MailboxEventBody,
}

type PendingCredit = (u128, Instant, u64, u64);

#[derive(Default)]
struct MailboxState {
    // At most one DATA record can be in flight. Consequently a valid pending
    // CREDIT batch has at most two accepted-through values: the prior
    // high-water mark and that DATA record's end. Same-accepted send-window
    // advances coalesce into their group without replacing the timestamp of
    // the accepted change. A third accepted change is necessarily malformed.
    credits: VecDeque<PendingCredit>,
    last_credit: Option<(u64, u64)>,
    terminal_events: VecDeque<(u128, MailboxEvent)>,
    abort_seen: bool,
    ack_seen: bool,
    fault_seen: bool,
    fault: Option<(u128, Instant)>,
    fault_abort: Option<(Instant, u64, BinaryAbortReason)>,
    first_terminal_sequence: Option<u128>,
    closed: bool,
    next_sequence: u128,
}

struct InboundMailbox {
    state: Mutex<MailboxState>,
    ready: Notify,
}

impl InboundMailbox {
    fn new() -> Self {
        Self {
            state: Mutex::new(MailboxState::default()),
            ready: Notify::new(),
        }
    }

    fn malformed(&self) {
        let received_at = Instant::now();
        let mut state = self.state.lock().expect("stream mailbox poisoned");
        let changed = Self::record_fault(&mut state, received_at);
        drop(state);
        if changed {
            self.ready.notify_one();
        }
    }

    fn push(&self, body: StreamRecordBody, allow_crossed_abort: bool) {
        let received_at = Instant::now();
        let mut state = self.state.lock().expect("stream mailbox poisoned");
        let Some(sequence) = Self::next_sequence(&mut state) else {
            drop(state);
            self.ready.notify_one();
            return;
        };
        let changed = match body {
            StreamRecordBody::Credit { .. } if state.abort_seen || state.ack_seen => {
                Self::record_fault_at(&mut state, sequence, received_at)
            }
            StreamRecordBody::Credit { .. } if state.fault_seen => false,
            StreamRecordBody::Credit {
                accepted_through,
                send_through,
            } => {
                let previous = state.last_credit;
                if accepted_through > send_through
                    || previous.is_some_and(|(old_accepted, old_send)| {
                        accepted_through < old_accepted || send_through < old_send
                    })
                {
                    Self::record_fault_at(&mut state, sequence, received_at)
                } else if previous == Some((accepted_through, send_through)) {
                    false
                } else {
                    state.last_credit = Some((accepted_through, send_through));
                    if let Some((_, _, pending_accepted, pending_send)) = state.credits.back_mut() {
                        if *pending_accepted == accepted_through {
                            // Only the cumulative send window changed. Keep
                            // the accepted group's original sequence/time so
                            // a later non-progress update cannot move actual
                            // progress across a stall boundary.
                            *pending_send = send_through;
                            false
                        } else if state.credits.len() < 2 {
                            state.credits.push_back((
                                sequence,
                                received_at,
                                accepted_through,
                                send_through,
                            ));
                            true
                        } else {
                            Self::record_fault_at(&mut state, sequence, received_at)
                        }
                    } else {
                        state.credits.push_back((
                            sequence,
                            received_at,
                            accepted_through,
                            send_through,
                        ));
                        true
                    }
                }
            }
            terminal @ (StreamRecordBody::Abort { .. } | StreamRecordBody::Ack { .. }) => {
                let is_abort = matches!(&terminal, StreamRecordBody::Abort { .. });
                let is_ack = matches!(&terminal, StreamRecordBody::Ack { .. });
                let allowed = (is_abort && !state.abort_seen && !state.ack_seen)
                    || (is_ack && !state.ack_seen && (!state.abort_seen || allow_crossed_abort));
                if allowed {
                    state.abort_seen |= is_abort;
                    state.ack_seen |= is_ack;
                    state.first_terminal_sequence.get_or_insert(sequence);
                    state.terminal_events.push_back((
                        sequence,
                        MailboxEvent {
                            received_at,
                            body: MailboxEventBody::Record(terminal),
                        },
                    ));
                    true
                } else {
                    // Terminal latches survive dequeue. Repeating a terminal
                    // is therefore rejected even if the actor already
                    // observed the first one. The only two-record exception
                    // is crossed client ABORT then daemon-ABORT ACK.
                    Self::record_fault_at(&mut state, sequence, received_at)
                }
            }
            _ if state.fault_seen => false,
            _ => Self::record_fault_at(&mut state, sequence, received_at),
        };
        drop(state);
        if changed {
            self.ready.notify_one();
        }
    }

    fn record_fault(state: &mut MailboxState, received_at: Instant) -> bool {
        let Some(sequence) = Self::next_sequence(state) else {
            return true;
        };
        Self::record_fault_at(state, sequence, received_at)
    }

    fn record_fault_at(state: &mut MailboxState, sequence: u128, received_at: Instant) -> bool {
        if state.fault_seen {
            return false;
        }
        state.fault_seen = true;
        // A duplicate/invalid record buffered behind an unconsumed terminal
        // invalidates that terminal batch, but never erases earlier CREDIT.
        // Replacing the batch at its first sequence preserves wire order.
        if state.fault_abort.is_none() {
            state.fault_abort =
                state
                    .terminal_events
                    .iter()
                    .find_map(|(_, event)| match &event.body {
                        MailboxEventBody::Record(StreamRecordBody::Abort {
                            accepted_through,
                            reason,
                        }) => Some((event.received_at, *accepted_through, *reason)),
                        _ => None,
                    });
        }
        let fault_sequence = state.first_terminal_sequence.unwrap_or(sequence);
        state.terminal_events.clear();
        state.fault = Some((fault_sequence, received_at));
        true
    }

    fn next_sequence(state: &mut MailboxState) -> Option<u128> {
        let sequence = state.next_sequence;
        let Some(next) = sequence.checked_add(1) else {
            // Although unreachable at any physical message rate, keep even
            // internal ordering arithmetic checked. A priority protocol fault
            // is the only safe behavior if the counter cannot advance.
            state.fault_seen = true;
            state.fault = Some((sequence, Instant::now()));
            return None;
        };
        state.next_sequence = next;
        Some(sequence)
    }

    fn take_locked(state: &mut MailboxState) -> Option<MailboxEvent> {
        let credit_sequence = state.credits.front().map(|(sequence, _, _, _)| *sequence);
        let terminal_sequence = state.terminal_events.front().map(|(sequence, _)| *sequence);
        let fault_sequence = state.fault.map(|(sequence, _)| sequence);
        let next = [credit_sequence, terminal_sequence, fault_sequence]
            .into_iter()
            .flatten()
            .min()?;
        if credit_sequence == Some(next) {
            return state.credits.pop_front().map(
                |(_, received_at, accepted_through, send_through)| MailboxEvent {
                    received_at,
                    body: MailboxEventBody::Record(StreamRecordBody::Credit {
                        accepted_through,
                        send_through,
                    }),
                },
            );
        }
        if terminal_sequence == Some(next) {
            return state.terminal_events.pop_front().map(|(_, event)| event);
        }
        state.fault.take().map(|(_, received_at)| {
            // The fault has now made its terminal decision. Subsequent input
            // belongs to the daemon-ABORT handshake and must be validated in
            // that state rather than silently discarded behind a stale latch.
            state.fault_seen = false;
            if state.terminal_events.is_empty() {
                state.first_terminal_sequence = None;
            }
            MailboxEvent {
                received_at,
                body: MailboxEventBody::Malformed,
            }
        })
    }

    fn try_recv(&self) -> Option<MailboxEvent> {
        Self::take_locked(&mut self.state.lock().expect("stream mailbox poisoned"))
    }

    fn has_pending(&self) -> bool {
        let state = self.state.lock().expect("stream mailbox poisoned");
        state.closed
            || state.fault.is_some()
            || !state.credits.is_empty()
            || !state.terminal_events.is_empty()
    }

    fn is_closed(&self) -> bool {
        self.state.lock().expect("stream mailbox poisoned").closed
    }

    fn take_fault_abort(&self) -> Option<(Instant, u64, BinaryAbortReason)> {
        self.state
            .lock()
            .expect("stream mailbox poisoned")
            .fault_abort
            .take()
    }

    async fn wait_ready(&self) {
        self.ready.notified().await;
    }

    fn close(&self) {
        self.state.lock().expect("stream mailbox poisoned").closed = true;
        self.ready.notify_waiters();
        self.ready.notify_one();
    }
}

struct StreamIngress {
    mailbox: InboundMailbox,
    sequencing: Mutex<()>,
    phase: AtomicU8,
    data_live: Arc<AtomicBool>,
    opening_fatal: AtomicBool,
    final_through: AtomicU64,
}

impl StreamIngress {
    fn new() -> Self {
        Self {
            mailbox: InboundMailbox::new(),
            sequencing: Mutex::new(()),
            phase: AtomicU8::new(BindingPhase::Opening as u8),
            data_live: Arc::new(AtomicBool::new(true)),
            opening_fatal: AtomicBool::new(false),
            final_through: AtomicU64::new(0),
        }
    }

    fn set_phase(&self, phase: BindingPhase) {
        let _sequencing = self.sequencing.lock().expect("stream sequencing poisoned");
        self.set_phase_locked(phase);
    }

    fn set_phase_locked(&self, phase: BindingPhase) {
        self.phase.store(phase as u8, Ordering::Release);
        if !matches!(phase, BindingPhase::Opening | BindingPhase::Active) {
            self.data_live.store(false, Ordering::Release);
        }
    }

    fn try_sequence_open(&self, attempt_live: &AtomicBool, absolute_deadline: Instant) -> bool {
        let _sequencing = self.sequencing.lock().expect("stream sequencing poisoned");
        if !attempt_live.load(Ordering::Acquire)
            || BindingPhase::load(&self.phase) != BindingPhase::Opening
            || !self.data_live.load(Ordering::Acquire)
            || Instant::now() >= absolute_deadline
        {
            return false;
        }
        self.set_phase_locked(BindingPhase::Active);
        true
    }

    fn cancel_open_attempt(&self, attempt_live: &AtomicBool) -> bool {
        let _sequencing = self.sequencing.lock().expect("stream sequencing poisoned");
        attempt_live.store(false, Ordering::Release);
        let committed = BindingPhase::load(&self.phase) == BindingPhase::Active;
        self.data_live.store(false, Ordering::Release);
        committed
    }

    fn try_sequence_end(
        &self,
        attempt_live: &AtomicBool,
        absolute_deadline: Instant,
        stall_deadline: Instant,
        final_through: u64,
    ) -> bool {
        let _sequencing = self.sequencing.lock().expect("stream sequencing poisoned");
        let now = Instant::now();
        if !attempt_live.load(Ordering::Acquire)
            || BindingPhase::load(&self.phase) != BindingPhase::Active
            || !self.data_live.load(Ordering::Acquire)
            || self.mailbox.has_pending()
            || now >= absolute_deadline
            || now >= stall_deadline
        {
            return false;
        }
        self.final_through.store(final_through, Ordering::Release);
        self.set_phase_locked(BindingPhase::Finalizing);
        true
    }

    fn try_sequence_client_abort_ack(&self, attempt_live: &AtomicBool, deadline: Instant) -> bool {
        let _sequencing = self.sequencing.lock().expect("stream sequencing poisoned");
        if !attempt_live.load(Ordering::Acquire)
            || BindingPhase::load(&self.phase) != BindingPhase::ClientAbortPending
            || self.mailbox.has_pending()
            || Instant::now() >= deadline
        {
            return false;
        }
        self.set_phase_locked(BindingPhase::ClientAbortAckCommitted);
        true
    }

    fn cancel_client_abort_ack_attempt(&self, attempt_live: &AtomicBool) -> ClientAbortAckCancel {
        let _sequencing = self.sequencing.lock().expect("stream sequencing poisoned");
        attempt_live.store(false, Ordering::Release);
        match BindingPhase::load(&self.phase) {
            BindingPhase::ClientAbortAckCommitted => ClientAbortAckCancel::Committed,
            BindingPhase::ClientAbortPending if self.mailbox.has_pending() => {
                ClientAbortAckCancel::ProtocolFault
            }
            _ => ClientAbortAckCancel::Cancelled,
        }
    }

    fn classify_client_abort_ack_discard(&self) -> ClientAbortAckOutcome {
        let _sequencing = self.sequencing.lock().expect("stream sequencing poisoned");
        if BindingPhase::load(&self.phase) == BindingPhase::ClientAbortPending
            && self.mailbox.has_pending()
        {
            ClientAbortAckOutcome::ProtocolFaultBeforeSent
        } else {
            ClientAbortAckOutcome::Failed
        }
    }

    fn classify_client_abort_ack_sent(&self) -> ClientAbortAckOutcome {
        let _sequencing = self.sequencing.lock().expect("stream sequencing poisoned");
        if BindingPhase::load(&self.phase) == BindingPhase::ClientAbortAckSent {
            ClientAbortAckOutcome::ProtocolFaultAfterSent
        } else {
            ClientAbortAckOutcome::Sent
        }
    }

    fn try_sequence_daemon_abort(&self, attempt_live: &AtomicBool, deadline: Instant) -> bool {
        let _sequencing = self.sequencing.lock().expect("stream sequencing poisoned");
        if !attempt_live.load(Ordering::Acquire)
            || BindingPhase::load(&self.phase) != BindingPhase::DaemonAbortQueued
            || Instant::now() >= deadline
        {
            return false;
        }
        self.set_phase_locked(BindingPhase::DaemonAbortWaitAck);
        true
    }

    fn cancel_daemon_abort_attempt(&self, attempt_live: &AtomicBool) -> bool {
        let _sequencing = self.sequencing.lock().expect("stream sequencing poisoned");
        attempt_live.store(false, Ordering::Release);
        BindingPhase::load(&self.phase) == BindingPhase::DaemonAbortWaitAck
    }

    fn cancel_end_attempt(&self, attempt_live: &AtomicBool) -> bool {
        let _sequencing = self.sequencing.lock().expect("stream sequencing poisoned");
        attempt_live.store(false, Ordering::Release);
        BindingPhase::load(&self.phase) == BindingPhase::Finalizing
    }

    fn stop_active_data(&self) -> bool {
        let _sequencing = self.sequencing.lock().expect("stream sequencing poisoned");
        if BindingPhase::load(&self.phase) != BindingPhase::Active {
            return false;
        }
        self.data_live.store(false, Ordering::Release);
        true
    }

    fn try_start_data(&self, absolute_deadline: Instant, stall_deadline: Instant) -> bool {
        let _sequencing = self.sequencing.lock().expect("stream sequencing poisoned");
        let now = Instant::now();
        BindingPhase::load(&self.phase) == BindingPhase::Active
            && self.data_live.load(Ordering::Acquire)
            && !self.mailbox.has_pending()
            && now < absolute_deadline
            && now < stall_deadline
    }

    fn protocol_fault(&self) {
        self.data_live.store(false, Ordering::Release);
        self.mailbox.malformed();
    }
}

/// Result of routing one complete inbound Binary message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BinaryRoute {
    /// The record was delivered or coalesced for its exact active pair.
    Delivered,
    /// Complete-message size takes the unparsed `4005` path.
    CloseTooLarge,
    /// No trustworthy exact binding exists; close `4007`.
    CloseMalformed,
}

/// Exact-pair bindings for one WebSocket connection.
#[derive(Clone)]
pub(crate) struct StreamRegistry {
    inner: Arc<Mutex<StreamRegistryState>>,
    invalidated: Arc<Notify>,
}

struct StreamRegistryState {
    accepting: bool,
    bindings: HashMap<StreamIdentity, StreamEndpoint>,
}

#[derive(Clone)]
enum StreamEndpoint {
    Download(Arc<StreamIngress>),
    Upload(Arc<crate::file_share::UploadIngress>),
}

#[cfg(test)]
impl StreamEndpoint {
    fn download(self) -> Arc<StreamIngress> {
        match self {
            Self::Download(ingress) => ingress,
            Self::Upload(_) => panic!("test expected a download binding"),
        }
    }
}

/// Single owner for a connection-fatal WebSocket Close.
///
/// Claiming a Close is synchronous: it first invalidates every stream and
/// queued non-Close write, then wakes the connection reader. The sole Close
/// write runs in an independent task so cancelling a request actor cannot
/// strand it, and a completion watch lets connection teardown preserve its
/// bounded flush opportunity before stopping the writer.
#[derive(Clone)]
pub(crate) struct ConnectionCloser {
    inner: Arc<ConnectionCloserInner>,
}

struct ConnectionCloserInner {
    claimed: AtomicBool,
    registry: StreamRegistry,
    outbound: Outbound,
    requested: watch::Sender<bool>,
    completed: watch::Sender<bool>,
}

impl ConnectionCloser {
    pub(crate) fn new(
        registry: StreamRegistry,
        outbound: Outbound,
    ) -> (Self, watch::Receiver<bool>, watch::Receiver<bool>) {
        let (requested, requested_rx) = watch::channel(false);
        let (completed, completed_rx) = watch::channel(false);
        (
            Self {
                inner: Arc::new(ConnectionCloserInner {
                    claimed: AtomicBool::new(false),
                    registry,
                    outbound,
                    requested,
                    completed,
                }),
            },
            requested_rx,
            completed_rx,
        )
    }

    /// Claims the connection's one fatal Close. The first caller owns its
    /// code/reason; later callers only observe the already-invalidated state.
    pub(crate) fn request(
        &self,
        frame: tokio_tungstenite::tungstenite::protocol::CloseFrame,
    ) -> bool {
        if self
            .inner
            .claimed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        self.inner.registry.invalidate_connection();
        self.inner.outbound.invalidate_connection();

        let outbound = self.inner.outbound.clone();
        let completed = self.inner.completed.clone();
        tokio::spawn(async move {
            let _ = outbound.close(frame).await;
            let _ = completed.send(true);
        });
        let _ = self.inner.requested.send(true);
        true
    }

    pub(crate) fn malformed(&self) -> bool {
        self.request(tokio_tungstenite::tungstenite::protocol::CloseFrame {
            code: 4007.into(),
            reason: "malformed_frame".into(),
        })
    }

    pub(crate) fn is_requested(&self) -> bool {
        self.inner.claimed.load(Ordering::Acquire)
    }
}

pub(crate) struct StreamBinding {
    registry: StreamRegistry,
    identity: StreamIdentity,
    ingress: Arc<StreamIngress>,
}

/// RAII ownership of one consumer-direction upload binding in the unified
/// connection-local registry.
pub(crate) struct UploadStreamBinding {
    registry: StreamRegistry,
    identity: StreamIdentity,
    ingress: Arc<crate::file_share::UploadIngress>,
}

#[derive(Clone)]
pub(crate) struct UploadStreamRetirement {
    registry: StreamRegistry,
    identity: StreamIdentity,
    ingress: Arc<crate::file_share::UploadIngress>,
}

#[derive(Clone)]
struct StreamRetirement {
    registry: StreamRegistry,
    identity: StreamIdentity,
    ingress: Arc<StreamIngress>,
}

impl StreamRegistry {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(StreamRegistryState {
                accepting: true,
                bindings: HashMap::new(),
            })),
            invalidated: Arc::new(Notify::new()),
        }
    }

    fn bind(&self, identity: StreamIdentity) -> Option<StreamBinding> {
        let ingress = Arc::new(StreamIngress::new());
        let mut registry = self.inner.lock().expect("stream registry poisoned");
        if !registry.accepting {
            return None;
        }
        match registry.bindings.entry(identity) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(StreamEndpoint::Download(ingress.clone()));
            }
            std::collections::hash_map::Entry::Occupied(_) => return None,
        }
        Some(StreamBinding {
            registry: self.clone(),
            identity,
            ingress,
        })
    }

    pub(crate) fn is_active(&self) -> bool {
        !self
            .inner
            .lock()
            .expect("stream registry poisoned")
            .bindings
            .is_empty()
    }

    pub(crate) fn is_accepting(&self) -> bool {
        self.inner
            .lock()
            .expect("stream registry poisoned")
            .accepting
    }

    pub(crate) async fn wait_invalidated(&self) {
        while self.is_accepting() {
            tokio::select! {
                () = self.invalidated.notified() => {}
                () = tokio::time::sleep(Duration::from_millis(10)) => {}
            }
        }
    }

    /// Atomically makes every current or future stream on this connection
    /// unstartable before a connection-fatal Close is queued. The registry
    /// lock is acquired before each stream sequencing lock, matching
    /// retirement, so an OPEN writer start cannot slip between invalidation
    /// and the Close merely because the FIFO control queue is congested.
    pub(crate) fn invalidate_connection(&self) {
        let mut registry = self.inner.lock().expect("stream registry poisoned");
        registry.accepting = false;
        for ingress in registry.bindings.values() {
            match ingress {
                StreamEndpoint::Download(ingress) => {
                    let _sequencing = ingress
                        .sequencing
                        .lock()
                        .expect("stream sequencing poisoned");
                    ingress.opening_fatal.store(true, Ordering::Release);
                    ingress.set_phase_locked(BindingPhase::Retired);
                    ingress.mailbox.close();
                    ingress.data_live.store(false, Ordering::Release);
                }
                StreamEndpoint::Upload(ingress) => ingress.invalidate_connection_locked(),
            }
        }
        registry.bindings.clear();
        drop(registry);
        self.invalidated.notify_waiters();
        self.invalidated.notify_one();
    }

    #[cfg(test)]
    pub(crate) fn route_binary(&self, bytes: &[u8], bounds: &CodecBounds) -> BinaryRoute {
        if bytes.len() > bounds.max_frame_bytes {
            return BinaryRoute::CloseTooLarge;
        }
        let identity = match decode_stream_identity(bytes, bounds) {
            Ok(identity) => identity,
            Err(StreamCodecError::FrameTooLarge { .. }) => return BinaryRoute::CloseTooLarge,
            Err(_) => return BinaryRoute::CloseMalformed,
        };
        let endpoint = {
            self.inner
                .lock()
                .expect("stream registry poisoned")
                .bindings
                .get(&identity)
                .cloned()
        };
        let Some(endpoint) = endpoint else {
            return BinaryRoute::CloseMalformed;
        };

        match endpoint {
            StreamEndpoint::Download(ingress) => Self::route_bound(&ingress, bytes, bounds),
            // The production WebSocket path uses `route_binary_message`,
            // which can await bounded upload ingress capacity. This legacy
            // borrowed entry point remains for the existing download unit
            // harness and must never turn legal scheduling pressure into a
            // protocol error.
            StreamEndpoint::Upload(_) => BinaryRoute::CloseMalformed,
        }
    }

    /// Route one owned complete Binary message through the unified exact-pair
    /// registry. Upload DATA retains the transport-owned `Bytes` while it
    /// waits for bounded staging capacity, so no payload copy is needed.
    pub(crate) async fn route_binary_message(
        &self,
        bytes: tokio_tungstenite::tungstenite::Bytes,
        bounds: &CodecBounds,
    ) -> BinaryRoute {
        if bytes.len() > bounds.max_frame_bytes {
            return BinaryRoute::CloseTooLarge;
        }
        let identity = match decode_stream_identity(&bytes, bounds) {
            Ok(identity) => identity,
            Err(StreamCodecError::FrameTooLarge { .. }) => return BinaryRoute::CloseTooLarge,
            Err(_) => return BinaryRoute::CloseMalformed,
        };
        let endpoint = {
            self.inner
                .lock()
                .expect("stream registry poisoned")
                .bindings
                .get(&identity)
                .cloned()
        };
        let Some(endpoint) = endpoint else {
            return BinaryRoute::CloseMalformed;
        };
        match endpoint {
            StreamEndpoint::Download(ingress) => Self::route_bound(&ingress, &bytes, bounds),
            StreamEndpoint::Upload(ingress) => ingress.route_bound(bytes, bounds).await,
        }
    }

    /// Install an upload binding in the same map downloads use. A duplicate
    /// full pair cannot replace or cross either direction.
    pub(crate) fn bind_upload(
        &self,
        identity: StreamIdentity,
        ingress: Arc<crate::file_share::UploadIngress>,
    ) -> Option<UploadStreamBinding> {
        let mut registry = self.inner.lock().expect("stream registry poisoned");
        if !registry.accepting {
            return None;
        }
        match registry.bindings.entry(identity) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(StreamEndpoint::Upload(ingress.clone()));
            }
            std::collections::hash_map::Entry::Occupied(_) => return None,
        }
        Some(UploadStreamBinding {
            registry: self.clone(),
            identity,
            ingress,
        })
    }

    fn route_bound(
        ingress: &Arc<StreamIngress>,
        bytes: &[u8],
        bounds: &CodecBounds,
    ) -> BinaryRoute {
        let _sequencing = ingress
            .sequencing
            .lock()
            .expect("stream sequencing poisoned");
        let phase = BindingPhase::load(&ingress.phase);
        if phase == BindingPhase::Opening {
            // Until the sole writer commits OPEN, the pair is private runtime
            // state rather than a trustworthy peer-visible binding. Cancel a
            // queued OPEN under the same sequencing lock before asking the
            // connection owner to close 4007.
            ingress.opening_fatal.store(true, Ordering::Release);
            ingress.data_live.store(false, Ordering::Release);
            return BinaryRoute::CloseMalformed;
        }
        if matches!(phase, BindingPhase::AckPending | BindingPhase::Retired) {
            return BinaryRoute::CloseMalformed;
        }
        // The exact active pair is trustworthy before kind/direction. This
        // staged helper performs no DATA allocation.
        let kind = match decode_stream_kind(bytes, bounds) {
            Ok(kind) => kind,
            Err(_) => {
                ingress.protocol_fault();
                return BinaryRoute::Delivered;
            }
        };
        let direction_ok = match phase {
            BindingPhase::Opening => unreachable!("opening returned above"),
            BindingPhase::Active => {
                matches!(kind, StreamRecordKind::Credit | StreamRecordKind::Abort)
            }
            BindingPhase::DaemonAbortQueued => {
                matches!(kind, StreamRecordKind::Credit | StreamRecordKind::Abort)
            }
            BindingPhase::DaemonAbortWaitAck => matches!(
                kind,
                StreamRecordKind::Credit | StreamRecordKind::Abort | StreamRecordKind::Ack
            ),
            BindingPhase::ClientAbortPending => {
                matches!(kind, StreamRecordKind::Credit | StreamRecordKind::Abort)
            }
            BindingPhase::ClientAbortAckCommitted | BindingPhase::ClientAbortAckSent => {
                matches!(kind, StreamRecordKind::Credit | StreamRecordKind::Abort)
            }
            BindingPhase::Finalizing => {
                matches!(kind, StreamRecordKind::Credit | StreamRecordKind::Abort)
            }
            BindingPhase::AckPending | BindingPhase::Retired => {
                unreachable!("terminal phases returned above")
            }
        };
        if !direction_ok {
            if phase == BindingPhase::Finalizing {
                return BinaryRoute::CloseMalformed;
            }
            ingress.protocol_fault();
            return BinaryRoute::Delivered;
        }

        let record = match decode_stream_record(bytes, bounds) {
            Ok(record) => record,
            Err(_) => {
                if phase == BindingPhase::Finalizing {
                    return BinaryRoute::CloseMalformed;
                }
                ingress.protocol_fault();
                return BinaryRoute::Delivered;
            }
        };
        if phase == BindingPhase::Finalizing {
            let final_through = ingress.final_through.load(Ordering::Acquire);
            let valid_late_control = match record.body {
                StreamRecordBody::Credit {
                    accepted_through,
                    send_through,
                } => accepted_through == final_through && send_through == final_through,
                StreamRecordBody::Abort {
                    accepted_through,
                    reason,
                } => {
                    accepted_through == final_through
                        && matches!(
                            reason,
                            BinaryAbortReason::Cancelled
                                | BinaryAbortReason::SinkFailed
                                | BinaryAbortReason::ProtocolError
                        )
                }
                _ => false,
            };
            // END already committed the authoritative result. A valid late
            // receiver control cannot alter it; an invalid one cannot be
            // answered with a second correlated terminal and is therefore a
            // fatal malformed terminal/binding failure.
            return if valid_late_control {
                BinaryRoute::Delivered
            } else {
                BinaryRoute::CloseMalformed
            };
        }
        if kind == StreamRecordKind::Abort {
            // Once the peer has sequenced a terminal control, queued source
            // DATA must not pass it. A write already in progress remains the
            // older in-flight message and naturally precedes the ACK.
            ingress.data_live.store(false, Ordering::Release);
        }
        if phase == BindingPhase::DaemonAbortWaitAck && kind == StreamRecordKind::Ack {
            // Latch the first structurally valid ACK before exposing it to the
            // actor. Until semantic validation and retirement finish, any
            // later record for this pair is an unambiguous 4007 rather than a
            // duplicate terminal racing the mailbox consumer.
            ingress.set_phase_locked(BindingPhase::AckPending);
        }
        ingress
            .mailbox
            .push(record.body, phase == BindingPhase::DaemonAbortWaitAck);
        BinaryRoute::Delivered
    }
}

impl StreamBinding {
    fn identity(&self) -> StreamIdentity {
        self.identity
    }

    fn set_phase(&self, phase: BindingPhase) {
        self.ingress.set_phase(phase);
    }

    fn phase(&self) -> BindingPhase {
        BindingPhase::load(&self.ingress.phase)
    }

    fn retirement(&self) -> StreamRetirement {
        StreamRetirement {
            registry: self.registry.clone(),
            identity: self.identity,
            ingress: self.ingress.clone(),
        }
    }

    fn retire(&self) {
        self.retirement().retire();
    }
}

impl StreamRetirement {
    /// Completes a client-ABORT ACK at the socket acknowledgement boundary.
    /// Routing uses the same sequencing lock: if a second exact-pair record
    /// won before the ACK flush, keep the binding request-local for the actor
    /// to promote into a daemon protocol ABORT. Otherwise retire it before a
    /// later record can still name the pair.
    fn retire_client_abort_ack(&self) {
        let mut registry = self
            .registry
            .inner
            .lock()
            .expect("stream registry poisoned");
        let _sequencing = self
            .ingress
            .sequencing
            .lock()
            .expect("stream sequencing poisoned");
        if BindingPhase::load(&self.ingress.phase) == BindingPhase::ClientAbortAckCommitted
            && self.ingress.mailbox.has_pending()
        {
            self.ingress
                .set_phase_locked(BindingPhase::ClientAbortAckSent);
            return;
        }
        self.ingress.set_phase_locked(BindingPhase::Retired);
        self.ingress.mailbox.close();
        self.ingress.data_live.store(false, Ordering::Release);
        if registry
            .bindings
            .get(&self.identity)
            .is_some_and(|current| {
                matches!(current, StreamEndpoint::Download(current) if Arc::ptr_eq(current, &self.ingress))
            })
        {
            registry.bindings.remove(&self.identity);
        }
    }

    fn retire(&self) {
        // Keep lock order identical to connection invalidation. No code may
        // hold a stream sequencing lock while acquiring the registry lock.
        let mut registry = self
            .registry
            .inner
            .lock()
            .expect("stream registry poisoned");
        let _sequencing = self
            .ingress
            .sequencing
            .lock()
            .expect("stream sequencing poisoned");
        self.ingress.set_phase_locked(BindingPhase::Retired);
        self.ingress.mailbox.close();
        self.ingress.data_live.store(false, Ordering::Release);
        if registry
            .bindings
            .get(&self.identity)
            .is_some_and(|current| {
                matches!(current, StreamEndpoint::Download(current) if Arc::ptr_eq(current, &self.ingress))
            })
        {
            registry.bindings.remove(&self.identity);
        }
    }
}

impl Drop for StreamBinding {
    fn drop(&mut self) {
        self.retire();
    }
}

impl UploadStreamBinding {
    pub(crate) fn retire(&self) {
        let mut registry = self
            .registry
            .inner
            .lock()
            .expect("stream registry poisoned");
        self.ingress.retire_locked();
        if registry
            .bindings
            .get(&self.identity)
            .is_some_and(|current| {
                matches!(current, StreamEndpoint::Upload(current) if Arc::ptr_eq(current, &self.ingress))
            })
        {
            registry.bindings.remove(&self.identity);
        }
    }

    pub(crate) fn retirement(&self) -> UploadStreamRetirement {
        UploadStreamRetirement {
            registry: self.registry.clone(),
            identity: self.identity,
            ingress: self.ingress.clone(),
        }
    }

    /// Retire the first semantically valid daemon-ABORT ACK only if no later
    /// exact-pair record was already queued behind it.
    pub(crate) fn retire_daemon_ack(&self) -> bool {
        let mut registry = self
            .registry
            .inner
            .lock()
            .expect("stream registry poisoned");
        if !self.ingress.daemon_ack_retire_locked() {
            return false;
        }
        if registry
            .bindings
            .get(&self.identity)
            .is_some_and(|current| {
                matches!(current, StreamEndpoint::Upload(current) if Arc::ptr_eq(current, &self.ingress))
            })
        {
            registry.bindings.remove(&self.identity);
        }
        true
    }
}

impl UploadStreamRetirement {
    pub(crate) fn retire(&self) {
        let mut registry = self
            .registry
            .inner
            .lock()
            .expect("stream registry poisoned");
        self.ingress.retire_locked();
        if registry
            .bindings
            .get(&self.identity)
            .is_some_and(|current| {
                matches!(current, StreamEndpoint::Upload(current) if Arc::ptr_eq(current, &self.ingress))
            })
        {
            registry.bindings.remove(&self.identity);
        }
    }

    /// Retire at the successful ACK writer boundary unless a later exact-pair
    /// record won the sequencing lock first. In that case the actor retains
    /// the binding long enough to promote the duplicate into protocol_error.
    pub(crate) fn retire_client_abort_ack(&self) {
        let mut registry = self
            .registry
            .inner
            .lock()
            .expect("stream registry poisoned");
        if !self.ingress.client_abort_ack_sent_locked() {
            return;
        }
        if registry
            .bindings
            .get(&self.identity)
            .is_some_and(|current| {
                matches!(current, StreamEndpoint::Upload(current) if Arc::ptr_eq(current, &self.ingress))
            })
        {
            registry.bindings.remove(&self.identity);
        }
    }
}

impl Drop for UploadStreamBinding {
    fn drop(&mut self) {
        self.retire();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CreditEffect {
    Repeated,
    Advanced { accepted: bool },
}

#[derive(Debug, Clone)]
struct ProducerState {
    total: u64,
    accepted: u64,
    send_through: u64,
    sent: u64,
    credit_seen: bool,
}

impl ProducerState {
    fn new(total: u64) -> Self {
        Self {
            total,
            accepted: 0,
            send_through: 0,
            sent: 0,
            credit_seen: false,
        }
    }

    fn credit(&mut self, accepted: u64, send: u64) -> Result<CreditEffect, ()> {
        if accepted > send
            || accepted < self.accepted
            || send < self.send_through
            || send > self.total
            || accepted > self.sent
            || (accepted != self.accepted && accepted != self.sent)
        {
            return Err(());
        }
        let repeated = self.credit_seen && accepted == self.accepted && send == self.send_through;
        let accepted_advanced = accepted > self.accepted;
        self.credit_seen = true;
        self.accepted = accepted;
        self.send_through = send;
        if repeated {
            Ok(CreditEffect::Repeated)
        } else {
            Ok(CreditEffect::Advanced {
                accepted: accepted_advanced,
            })
        }
    }

    fn next_payload(&self, max_payload: usize) -> Option<usize> {
        if !self.credit_seen
            || self.accepted != self.sent
            || self.sent >= self.send_through
            || self.sent >= self.total
        {
            return None;
        }
        let permitted = self
            .send_through
            .checked_sub(self.sent)?
            .min(self.total.checked_sub(self.sent)?);
        usize::try_from(permitted)
            .ok()
            .map(|bytes| bytes.min(max_payload))
            .filter(|bytes| *bytes > 0)
    }

    fn data_sent(&mut self, payload_bytes: usize) -> Result<(), ()> {
        let payload = u64::try_from(payload_bytes).map_err(|_| ())?;
        let end = self.sent.checked_add(payload).ok_or(())?;
        if payload == 0 || self.accepted != self.sent || end > self.send_through || end > self.total
        {
            return Err(());
        }
        self.sent = end;
        Ok(())
    }

    fn valid_receiver_terminal(&self, accepted: u64) -> bool {
        accepted >= self.accepted
            && accepted <= self.sent
            && (accepted == self.accepted || accepted == self.sent)
    }

    fn ready_for_end(&self) -> bool {
        self.credit_seen && self.sent == self.total && self.accepted == self.total
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DaemonFailure {
    Source,
    Protocol,
    Deadline { budget_ms: u64 },
    Stall,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActiveStop {
    ClientAbort {
        accepted: u64,
        reason: StreamAbortReason,
    },
    Daemon(DaemonFailure),
    TransportLost,
}

fn known_total(total: u64) -> ByteTotal {
    ByteTotal::Known { bytes: total }
}

fn apply_active_event(
    state: &mut ProducerState,
    event: MailboxEvent,
    stall_deadline: &mut Instant,
    limits: RuntimeLimits,
) -> Result<(), ActiveStop> {
    let MailboxEvent { received_at, body } = event;
    match body {
        MailboxEventBody::Malformed => Err(ActiveStop::Daemon(DaemonFailure::Protocol)),
        MailboxEventBody::Record(StreamRecordBody::Credit {
            accepted_through,
            send_through,
        }) => match state.credit(accepted_through, send_through) {
            Ok(CreditEffect::Advanced { accepted: true }) => {
                *stall_deadline = limits
                    .stall_deadline(received_at)
                    .map_err(|_| ActiveStop::Daemon(DaemonFailure::Protocol))?;
                Ok(())
            }
            Ok(CreditEffect::Advanced { accepted: false } | CreditEffect::Repeated) => Ok(()),
            Err(()) => Err(ActiveStop::Daemon(DaemonFailure::Protocol)),
        },
        MailboxEventBody::Record(StreamRecordBody::Abort {
            accepted_through,
            reason,
        }) => {
            let reason = match reason {
                BinaryAbortReason::Cancelled => StreamAbortReason::Cancelled,
                BinaryAbortReason::SinkFailed => StreamAbortReason::SinkFailed,
                BinaryAbortReason::ProtocolError => StreamAbortReason::ProtocolError,
                // The client is the receiver. `source_failed` describes the
                // producer's local failure, and operation_error is daemon-only.
                BinaryAbortReason::SourceFailed | BinaryAbortReason::OperationError => {
                    return Err(ActiveStop::Daemon(DaemonFailure::Protocol));
                }
            };
            if !state.valid_receiver_terminal(accepted_through) {
                return Err(ActiveStop::Daemon(DaemonFailure::Protocol));
            }
            Err(ActiveStop::ClientAbort {
                accepted: accepted_through,
                reason,
            })
        }
        MailboxEventBody::Record(_) => Err(ActiveStop::Daemon(DaemonFailure::Protocol)),
    }
}

fn timer_stop_at_event(
    timing: ActiveTiming,
    stall_deadline: Instant,
    received_at: Instant,
    event_wins_tie: bool,
) -> Option<ActiveStop> {
    let deadline_expired = if event_wins_tie {
        received_at > timing.absolute_deadline
    } else {
        received_at >= timing.absolute_deadline
    };
    if deadline_expired {
        return Some(ActiveStop::Daemon(DaemonFailure::Deadline {
            budget_ms: timing.budget_ms,
        }));
    }
    let stall_expired = if event_wins_tie {
        received_at > stall_deadline
    } else {
        received_at >= stall_deadline
    };
    stall_expired.then_some(ActiveStop::Daemon(DaemonFailure::Stall))
}

fn timer_stop(timing: ActiveTiming, stall_deadline: Instant, now: Instant) -> Option<ActiveStop> {
    if now >= timing.absolute_deadline {
        Some(ActiveStop::Daemon(DaemonFailure::Deadline {
            budget_ms: timing.budget_ms,
        }))
    } else if now >= stall_deadline {
        Some(ActiveStop::Daemon(DaemonFailure::Stall))
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActivePoll {
    Event,
    Idle,
}

fn sequence_active_stop_locked(ingress: &StreamIngress, stop: ActiveStop) {
    match stop {
        ActiveStop::ClientAbort { .. } => {
            ingress.set_phase_locked(BindingPhase::ClientAbortPending)
        }
        ActiveStop::Daemon(_) => ingress.set_phase_locked(BindingPhase::DaemonAbortQueued),
        ActiveStop::TransportLost => ingress.data_live.store(false, Ordering::Release),
    }
}

/// Atomically observes the next routed record or claims an expired timer.
/// Routing, OPEN/DATA/END writer starts, and this decision all use the same
/// sequencing lock, so a progress CREDIT or valid client ABORT cannot be
/// hidden merely because the actor was descheduled at the boundary.
fn poll_active(
    binding: &StreamBinding,
    state: &mut ProducerState,
    stall_deadline: &mut Instant,
    timing: ActiveTiming,
) -> Result<ActivePoll, ActiveStop> {
    let _sequencing = binding
        .ingress
        .sequencing
        .lock()
        .expect("stream sequencing poisoned");
    if let Some(event) = binding.ingress.mailbox.try_recv() {
        match apply_active_event_with_timers(state, event, stall_deadline, timing) {
            Ok(()) => return Ok(ActivePoll::Event),
            Err(stop) => {
                sequence_active_stop_locked(&binding.ingress, stop);
                return Err(stop);
            }
        }
    }
    if binding.ingress.mailbox.is_closed() {
        let stop = ActiveStop::TransportLost;
        sequence_active_stop_locked(&binding.ingress, stop);
        return Err(stop);
    }
    if let Some(stop) = timer_stop(timing, *stall_deadline, Instant::now()) {
        sequence_active_stop_locked(&binding.ingress, stop);
        return Err(stop);
    }
    Ok(ActivePoll::Idle)
}

/// Sequences a non-timer local failure after giving every already-routed
/// record its deterministic earlier opportunity. Source/codec failures do not
/// become a scheduling shortcut around a client ABORT that already arrived.
fn claim_local_stop(
    binding: &StreamBinding,
    state: &mut ProducerState,
    stall_deadline: &mut Instant,
    timing: ActiveTiming,
    candidate: ActiveStop,
) -> ActiveStop {
    let _sequencing = binding
        .ingress
        .sequencing
        .lock()
        .expect("stream sequencing poisoned");
    if BindingPhase::load(&binding.ingress.phase) != BindingPhase::Active {
        return candidate;
    }
    while let Some(event) = binding.ingress.mailbox.try_recv() {
        if let Err(stop) = apply_active_event_with_timers(state, event, stall_deadline, timing) {
            sequence_active_stop_locked(&binding.ingress, stop);
            return stop;
        }
    }
    sequence_active_stop_locked(&binding.ingress, candidate);
    candidate
}

fn apply_active_event_with_timers(
    state: &mut ProducerState,
    event: MailboxEvent,
    stall_deadline: &mut Instant,
    timing: ActiveTiming,
) -> Result<(), ActiveStop> {
    // Only a semantically valid peer ABORT wins a timer tie. Merely carrying
    // the ABORT kind is insufficient: an invalid reason or high-water mark is
    // malformed input, so deadline then stall retains priority over it.
    let received_at = event.received_at;
    if matches!(
        &event.body,
        MailboxEventBody::Record(StreamRecordBody::Abort { .. })
    ) {
        let result = apply_active_event(state, event, stall_deadline, timing.limits);
        let valid_client_abort = matches!(result, Err(ActiveStop::ClientAbort { .. }));
        if let Some(stop) =
            timer_stop_at_event(timing, *stall_deadline, received_at, valid_client_abort)
        {
            return Err(stop);
        }
        return result;
    }
    if let Some(stop) = timer_stop_at_event(timing, *stall_deadline, received_at, false) {
        return Err(stop);
    }
    apply_active_event(state, event, stall_deadline, timing.limits)
}

async fn await_active<F, T>(
    operation: F,
    binding: &StreamBinding,
    state: &mut ProducerState,
    stall_deadline: &mut Instant,
    timing: ActiveTiming,
) -> Result<T, ActiveStop>
where
    F: std::future::Future<Output = T>,
{
    tokio::pin!(operation);
    loop {
        match poll_active(binding, state, stall_deadline, timing)? {
            ActivePoll::Event => continue,
            ActivePoll::Idle => {}
        }
        tokio::select! {
            biased;
            () = binding.ingress.mailbox.wait_ready() => {}
            () = tokio::time::sleep_until(timing.absolute_deadline) => {}
            () = tokio::time::sleep_until(*stall_deadline) => {}
            output = &mut operation => return Ok(output),
        }
    }
}

async fn await_active_stop(
    binding: &StreamBinding,
    state: &mut ProducerState,
    stall_deadline: &mut Instant,
    timing: ActiveTiming,
) -> ActiveStop {
    match await_active(
        std::future::pending::<()>(),
        binding,
        state,
        stall_deadline,
        timing,
    )
    .await
    {
        Err(stop) => stop,
        Ok(()) => unreachable!("a pending operation cannot complete"),
    }
}

#[derive(Clone, Copy)]
struct ActiveTiming {
    absolute_deadline: Instant,
    budget_ms: u64,
    limits: RuntimeLimits,
}

async fn await_data_write(
    receipt: PendingWrite,
    binding: &StreamBinding,
    state: &mut ProducerState,
    payload_bytes: usize,
    stall_deadline: &mut Instant,
    timing: ActiveTiming,
) -> Result<WriteReceipt, ActiveStop> {
    let cancellation = receipt.cancellation();
    let receipt = receipt.wait();
    tokio::pin!(receipt);
    // Do not dequeue CREDIT/ABORT until the older DATA write is reconciled.
    // A peer can observe and acknowledge DATA while the local sink flush is
    // still pending; validating that acknowledgement against the old `sent`
    // high-water mark would be a false protocol fault. ABORT/fault routing
    // still lowers `data_live` immediately, so DATA not yet started is
    // discarded. The writer owns an independent finite send watchdog, making
    // this receipt wait bounded even if the socket sink stops draining.
    loop {
        tokio::select! {
            biased;
            result = &mut receipt => {
                if result == WriteReceipt::Sent {
                    state
                        .data_sent(payload_bytes)
                        .map_err(|()| ActiveStop::Daemon(DaemonFailure::Protocol))?;
                }
                return Ok(result);
            }
            () = binding.ingress.mailbox.wait_ready() => {
                if !binding.ingress.data_live.load(Ordering::Acquire) {
                    if cancellation.cancel_before_start() {
                        return Ok(WriteReceipt::Discarded);
                    }
                    // Writer start won. Its independently bounded receipt is
                    // the only proof whether this older DATA became visible;
                    // reconcile it before applying the terminal record.
                    let result = (&mut receipt).await;
                    if result == WriteReceipt::Sent {
                        state
                            .data_sent(payload_bytes)
                            .map_err(|()| ActiveStop::Daemon(DaemonFailure::Protocol))?;
                    }
                    return Ok(result);
                }
            }
            () = tokio::time::sleep_until(timing.absolute_deadline) => {
                binding.ingress.stop_active_data();
                if cancellation.cancel_before_start() {
                    return Ok(WriteReceipt::Discarded);
                }
                // A DATA whose writer start preceded the deadline is older
                // than the terminal decision and must drain. The main actor
                // then applies timestamped ABORT/CREDIT before choosing the
                // deadline result.
                let result = (&mut receipt).await;
                if result == WriteReceipt::Sent {
                    state
                        .data_sent(payload_bytes)
                        .map_err(|()| ActiveStop::Daemon(DaemonFailure::Protocol))?;
                }
                return Ok(result);
            }
            () = tokio::time::sleep_until(*stall_deadline) => {
                if cancellation.cancel_before_start() {
                    return Ok(WriteReceipt::Discarded);
                }
                // If writer start already won, a client may have observed and
                // acknowledged this DATA just before the stall boundary.
                // Reconcile the write, then let ingress timestamps decide.
                let result = (&mut receipt).await;
                if result == WriteReceipt::Sent {
                    state
                        .data_sent(payload_bytes)
                        .map_err(|()| ActiveStop::Daemon(DaemonFailure::Protocol))?;
                }
                return Ok(result);
            }
        }
    }
}

struct StreamTerminalContext<'a> {
    outbound: &'a Outbound,
    binding: &'a StreamBinding,
    bounds: &'a CodecBounds,
    closer: &'a ConnectionCloser,
}

async fn send_stream_control_until(
    context: &StreamTerminalContext<'_>,
    bytes: Vec<u8>,
    deadline: Instant,
) -> WriteReceipt {
    let write_live = Arc::new(AtomicBool::new(true));
    let attempt = Arc::new(AtomicU8::new(CONTROL_ATTEMPT_QUEUED));
    let start_attempt = attempt.clone();
    let write = context
        .outbound
        .binary_control_with_start(bytes, write_live.clone(), move || {
            start_attempt
                .compare_exchange(
                    CONTROL_ATTEMPT_QUEUED,
                    CONTROL_ATTEMPT_STARTED,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
        });
    tokio::pin!(write);
    tokio::select! {
        biased;
        receipt = &mut write => receipt,
        () = tokio::time::sleep_until(deadline) => {
            if attempt.compare_exchange(
                CONTROL_ATTEMPT_QUEUED,
                CONTROL_ATTEMPT_CANCELLED,
                Ordering::AcqRel,
                Ordering::Acquire,
            ).is_ok() {
                write_live.store(false, Ordering::Release);
                WriteReceipt::Closed
            } else {
                // Writer start won, so this control may be peer-visible. Its
                // independent writer watchdog bounds the reconciliation.
                write.await
            }
        }
    }
}

async fn send_client_abort_ack_until(
    context: &StreamTerminalContext<'_>,
    bytes: Vec<u8>,
    deadline: Instant,
    retirement: StreamRetirement,
) -> ClientAbortAckOutcome {
    let attempt_live = Arc::new(AtomicBool::new(true));
    let commit_live = attempt_live.clone();
    let ingress = context.binding.ingress.clone();
    let cancel_ingress = context.binding.ingress.clone();
    let cancel_live = attempt_live.clone();
    let write = context.outbound.binary_control_with_hooks(
        bytes,
        attempt_live,
        move || ingress.try_sequence_client_abort_ack(&commit_live, deadline),
        move || retirement.retire_client_abort_ack(),
    );
    tokio::pin!(write);
    tokio::select! {
        biased;
        receipt = &mut write => match receipt {
            WriteReceipt::Sent => cancel_ingress.classify_client_abort_ack_sent(),
            WriteReceipt::Discarded => cancel_ingress.classify_client_abort_ack_discard(),
            WriteReceipt::Closed => ClientAbortAckOutcome::Failed,
        },
        () = tokio::time::sleep_until(deadline) => {
            match cancel_ingress.cancel_client_abort_ack_attempt(&cancel_live) {
                ClientAbortAckCancel::Committed => {
                    if write.await == WriteReceipt::Sent {
                        cancel_ingress.classify_client_abort_ack_sent()
                    } else {
                        ClientAbortAckOutcome::Failed
                    }
                }
                ClientAbortAckCancel::ProtocolFault => {
                    ClientAbortAckOutcome::ProtocolFaultBeforeSent
                }
                ClientAbortAckCancel::Cancelled => ClientAbortAckOutcome::Failed,
            }
        }
    }
}

async fn send_phase_control_until(
    context: &StreamTerminalContext<'_>,
    bytes: Vec<u8>,
    deadline: Instant,
) -> WriteReceipt {
    let attempt_live = Arc::new(AtomicBool::new(true));
    let commit_live = attempt_live.clone();
    let ingress = context.binding.ingress.clone();
    let cancel_ingress = context.binding.ingress.clone();
    let cancel_live = attempt_live.clone();
    let write = context
        .outbound
        .binary_control_with_start(bytes, attempt_live, move || {
            ingress.try_sequence_open(&commit_live, deadline)
        });
    tokio::pin!(write);
    tokio::select! {
        biased;
        receipt = &mut write => receipt,
        () = tokio::time::sleep_until(deadline) => {
            if cancel_ingress.cancel_open_attempt(&cancel_live) {
                // OPEN won the atomic writer-start race. Reconcile its bounded
                // socket receipt; the caller then runs the admitted stream's
                // deadline ABORT path rather than a pre-OPEN refusal.
                write.await
            } else {
                WriteReceipt::Discarded
            }
        }
    }
}

async fn send_daemon_abort_until(
    context: &StreamTerminalContext<'_>,
    bytes: Vec<u8>,
    deadline: Instant,
) -> WriteReceipt {
    let attempt_live = Arc::new(AtomicBool::new(true));
    let commit_live = attempt_live.clone();
    let ingress = context.binding.ingress.clone();
    let cancel_ingress = context.binding.ingress.clone();
    let cancel_live = attempt_live.clone();
    let write = context
        .outbound
        .binary_control_with_start(bytes, attempt_live, move || {
            ingress.try_sequence_daemon_abort(&commit_live, deadline)
        });
    tokio::pin!(write);
    tokio::select! {
        biased;
        receipt = &mut write => receipt,
        () = tokio::time::sleep_until(deadline) => {
            if cancel_ingress.cancel_daemon_abort_attempt(&cancel_live) {
                // Writer start won atomically. Reconcile the bounded socket
                // receipt; ACK is trustworthy only after this transition.
                write.await
            } else {
                WriteReceipt::Discarded
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn send_end_while_active(
    context: &StreamTerminalContext<'_>,
    bytes: Vec<u8>,
    state: &mut ProducerState,
    reservation: Arc<Mutex<Option<TransferReservation>>>,
    stall_deadline: &mut Instant,
    timing: ActiveTiming,
) -> Result<WriteReceipt, ActiveStop> {
    'attempt: loop {
        match poll_active(context.binding, state, stall_deadline, timing)? {
            ActivePoll::Event => continue,
            ActivePoll::Idle => {}
        }

        let attempt_live = Arc::new(AtomicBool::new(true));
        let commit_live = attempt_live.clone();
        let ingress = context.binding.ingress.clone();
        let release_on_commit = reservation.clone();
        let end_stall_deadline = *stall_deadline;
        let absolute_deadline = timing.absolute_deadline;
        let final_through = state.accepted;
        let write = context.outbound.binary_control_with_start(
            bytes.clone(),
            attempt_live.clone(),
            move || {
                if !ingress.try_sequence_end(
                    &commit_live,
                    absolute_deadline,
                    end_stall_deadline,
                    final_through,
                ) {
                    return false;
                }
                drop(
                    release_on_commit
                        .lock()
                        .expect("transfer reservation poisoned")
                        .take(),
                );
                true
            },
        );
        tokio::pin!(write);

        loop {
            tokio::select! {
                biased;
                receipt = &mut write => {
                    if receipt == WriteReceipt::Discarded {
                        if context.binding.phase() == BindingPhase::Active
                            && context.binding.ingress.data_live.load(Ordering::Acquire)
                        {
                            // A record was queued under ACTIVE before the writer
                            // could commit END. Drain and validate it first; an
                            // idempotent CREDIT permits a fresh END attempt.
                            continue 'attempt;
                        }
                        // An earlier terminal lowered `data_live`; consume its
                        // sequenced event to choose the exact terminal result.
                        return Err(
                            await_active_stop(context.binding, state, stall_deadline, timing).await,
                        );
                    }
                    return Ok(receipt);
                }
                () = context.binding.ingress.mailbox.wait_ready() => {
                    // Coalesced/identical CREDIT deliberately creates no new
                    // event, but a stale Notify permit can remain after the
                    // pre-attempt drain. It must not cancel a valid END.
                    if !context.binding.ingress.mailbox.has_pending() {
                        continue;
                    }
                    if context.binding.ingress.cancel_end_attempt(&attempt_live) {
                        return Ok(write.await);
                    }
                    continue 'attempt;
                }
                () = tokio::time::sleep_until(timing.absolute_deadline) => {
                    if context.binding.ingress.cancel_end_attempt(&attempt_live) {
                        return Ok(write.await);
                    }
                    continue 'attempt;
                }
                () = tokio::time::sleep_until(*stall_deadline) => {
                    if context.binding.ingress.cancel_end_attempt(&attempt_live) {
                        return Ok(write.await);
                    }
                    continue 'attempt;
                }
            }
        }
    }
}

fn stream_record(
    identity: StreamIdentity,
    body: StreamRecordBody,
    bounds: &CodecBounds,
) -> Option<Vec<u8>> {
    encode_stream_record(&StreamRecord { identity, body }, bounds).ok()
}

fn reply_bytes(id: u64, result: Result<&FileReadOut, ApiError>) -> Vec<u8> {
    let reply = match result {
        Ok(out) => jeliya_codec::Reply {
            id,
            ok: true,
            out: serde_json::to_value(out).ok(),
            err: None,
        },
        Err(err) => jeliya_codec::Reply {
            id,
            ok: false,
            out: None,
            err: Some(err),
        },
    };
    reply.to_bytes()
}

async fn send_reply(
    outbound: &Outbound,
    id: u64,
    result: Result<&FileReadOut, ApiError>,
    request: &RequestLease,
) -> bool {
    let release = request.clone();
    outbound
        .text_with_on_sent(reply_bytes(id, result), move || release.release())
        .await
        == WriteReceipt::Sent
}

async fn send_reply_until(
    outbound: &Outbound,
    id: u64,
    result: Result<&FileReadOut, ApiError>,
    deadline: Instant,
    request: &RequestLease,
    retirement: Option<StreamRetirement>,
) -> bool {
    let write_live = Arc::new(AtomicBool::new(true));
    let attempt = Arc::new(AtomicU8::new(CONTROL_ATTEMPT_QUEUED));
    let start_attempt = attempt.clone();
    let release = request.clone();
    let write = outbound.text_with_hooks(
        reply_bytes(id, result),
        write_live.clone(),
        move || {
            start_attempt
                .compare_exchange(
                    CONTROL_ATTEMPT_QUEUED,
                    CONTROL_ATTEMPT_STARTED,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
        },
        move || {
            if let Some(retirement) = retirement {
                retirement.retire();
            }
            release.release();
        },
    );
    tokio::pin!(write);
    tokio::select! {
        biased;
        receipt = &mut write => {
            receipt == WriteReceipt::Sent
        }
        () = tokio::time::sleep_until(deadline) => {
            if attempt.compare_exchange(
                CONTROL_ATTEMPT_QUEUED,
                CONTROL_ATTEMPT_CANCELLED,
                Ordering::AcqRel,
                Ordering::Acquire,
            ).is_ok() {
                write_live.store(false, Ordering::Release);
                false
            } else {
                write.await == WriteReceipt::Sent
            }
        }
    }
}

fn daemon_error(failure: DaemonFailure, transferred: u64, total: u64) -> ApiError {
    match failure {
        DaemonFailure::Source => ApiError::StreamAborted {
            transferred_bytes: transferred,
            total: known_total(total),
            reason: StreamAbortReason::SourceFailed,
        },
        DaemonFailure::Protocol => ApiError::MalformedFrame,
        DaemonFailure::Deadline { budget_ms } => ApiError::TransferDeadlineExceeded {
            transferred_bytes: transferred,
            total: known_total(total),
            budget_ms,
        },
        DaemonFailure::Stall => ApiError::TransferStalled {
            transferred_bytes: transferred,
            total: known_total(total),
        },
    }
}

fn daemon_abort_reason(failure: DaemonFailure) -> BinaryAbortReason {
    match failure {
        DaemonFailure::Source => BinaryAbortReason::SourceFailed,
        DaemonFailure::Protocol => BinaryAbortReason::ProtocolError,
        DaemonFailure::Deadline { .. } | DaemonFailure::Stall => BinaryAbortReason::OperationError,
    }
}

fn signal_transport_close(closer: &ConnectionCloser) {
    closer.malformed();
}

async fn finish_client_abort(
    context: StreamTerminalContext<'_>,
    id: u64,
    request: &RequestLease,
    state: &ProducerState,
    accepted: u64,
    reason: StreamAbortReason,
    write_timeout: Duration,
) -> bool {
    let Some(ack) = stream_record(
        context.binding.identity(),
        StreamRecordBody::Ack {
            accepted_through: accepted,
        },
        context.bounds,
    ) else {
        return false;
    };
    let ack_deadline = Instant::now()
        .checked_add(write_timeout)
        .expect("served stall duration was validated");
    let retirement = context.binding.retirement();
    match send_client_abort_ack_until(&context, ack, ack_deadline, retirement).await {
        ClientAbortAckOutcome::Sent => {}
        ClientAbortAckOutcome::ProtocolFaultBeforeSent => {
            // A second ABORT, CREDIT, or malformed record followed the first
            // client ABORT before its ACK committed. Consume that latched
            // fault, promote to a crossed daemon protocol ABORT, and still
            // satisfy the ACK owed for the first client terminal.
            while context.binding.ingress.mailbox.try_recv().is_some() {}
            return finish_daemon_abort(
                context,
                id,
                request,
                state,
                DaemonFailure::Protocol,
                write_timeout,
                Some(accepted),
            )
            .await;
        }
        ClientAbortAckOutcome::ProtocolFaultAfterSent => {
            // The first client ABORT's ACK is already peer-visible, but the
            // exact pair stayed bound while that ACK flushed. Promote the
            // latched follow-up record without emitting a duplicate ACK.
            while context.binding.ingress.mailbox.try_recv().is_some() {}
            return finish_daemon_abort(
                context,
                id,
                request,
                state,
                DaemonFailure::Protocol,
                write_timeout,
                None,
            )
            .await;
        }
        ClientAbortAckOutcome::Failed => {
            signal_transport_close(context.closer);
            return false;
        }
    }
    let reply_deadline = Instant::now()
        .checked_add(write_timeout)
        .expect("served stall duration was validated");
    let replied = send_reply_until(
        context.outbound,
        id,
        Err(ApiError::StreamAborted {
            transferred_bytes: accepted,
            total: known_total(state.total),
            reason,
        }),
        reply_deadline,
        request,
        None,
    )
    .await;
    if !replied {
        signal_transport_close(context.closer);
    }
    replied
}

enum DaemonAckPoll {
    Event(MailboxEvent),
    Idle,
    TimedOut,
    Closed,
}

/// Atomically chooses the next daemon-ABORT handshake record or its timeout.
/// Inbound routing uses this same sequencing lock, so an ACK cannot be
/// timestamped in time between an empty mailbox observation and a separate
/// timeout decision.
fn poll_daemon_ack(binding: &StreamBinding, ack_deadline: Instant) -> DaemonAckPoll {
    let _sequencing = binding
        .ingress
        .sequencing
        .lock()
        .expect("stream sequencing poisoned");
    if let Some(event) = binding.ingress.mailbox.try_recv() {
        return DaemonAckPoll::Event(event);
    }
    if binding.ingress.mailbox.is_closed() {
        return DaemonAckPoll::Closed;
    }
    if Instant::now() >= ack_deadline {
        return DaemonAckPoll::TimedOut;
    }
    DaemonAckPoll::Idle
}

async fn send_receiver_abort_ack(
    context: &StreamTerminalContext<'_>,
    accepted_through: u64,
    ack_deadline: Instant,
) -> bool {
    let Some(ack) = stream_record(
        context.binding.identity(),
        StreamRecordBody::Ack { accepted_through },
        context.bounds,
    ) else {
        return false;
    };
    send_stream_control_until(context, ack, ack_deadline).await == WriteReceipt::Sent
}

/// A later malformed/duplicate record may collapse a queued crossed ABORT
/// into one mailbox fault. Preserve and discharge that first terminal's ACK
/// obligation before ending the daemon-ABORT handshake.
async fn acknowledge_retained_fault_abort(
    context: &StreamTerminalContext<'_>,
    state: &ProducerState,
    ack_deadline: Instant,
) -> bool {
    let Some((received_at, accepted_through, reason)) =
        context.binding.ingress.mailbox.take_fault_abort()
    else {
        return true;
    };
    if received_at > ack_deadline
        || !matches!(
            reason,
            BinaryAbortReason::Cancelled
                | BinaryAbortReason::SinkFailed
                | BinaryAbortReason::ProtocolError
        )
        || !state.valid_receiver_terminal(accepted_through)
    {
        return true;
    }
    send_receiver_abort_ack(context, accepted_through, ack_deadline).await
}

async fn finish_daemon_abort(
    context: StreamTerminalContext<'_>,
    id: u64,
    request: &RequestLease,
    state: &ProducerState,
    failure: DaemonFailure,
    ack_timeout: Duration,
    crossed_client_abort: Option<u64>,
) -> bool {
    context.binding.set_phase(BindingPhase::DaemonAbortQueued);
    let Some(abort) = stream_record(
        context.binding.identity(),
        StreamRecordBody::Abort {
            accepted_through: state.accepted,
            reason: daemon_abort_reason(failure),
        },
        context.bounds,
    ) else {
        return false;
    };
    let abort_deadline = Instant::now()
        .checked_add(ack_timeout)
        .expect("served stall duration was validated");
    if send_daemon_abort_until(&context, abort, abort_deadline).await != WriteReceipt::Sent {
        signal_transport_close(context.closer);
        return false;
    }

    let ack_deadline = Instant::now()
        .checked_add(ack_timeout)
        .expect("served stall duration was validated");
    if let Some(accepted_through) = crossed_client_abort {
        if !send_receiver_abort_ack(&context, accepted_through, ack_deadline).await {
            signal_transport_close(context.closer);
            return false;
        }
    } else if !acknowledge_retained_fault_abort(&context, state, ack_deadline).await {
        signal_transport_close(context.closer);
        return false;
    }
    let mut terminal_state = state.clone();
    let mut transferred = None;
    loop {
        match poll_daemon_ack(context.binding, ack_deadline) {
            DaemonAckPoll::Event(event) => {
                let received_in_time = event.received_at <= ack_deadline;
                match event.body {
                    MailboxEventBody::Record(StreamRecordBody::Ack { accepted_through })
                        if received_in_time
                            && terminal_state.valid_receiver_terminal(accepted_through) =>
                    {
                        transferred = Some(accepted_through);
                        break;
                    }
                    MailboxEventBody::Record(StreamRecordBody::Abort {
                        accepted_through,
                        reason,
                    }) if received_in_time
                        && matches!(
                            reason,
                            BinaryAbortReason::Cancelled
                                | BinaryAbortReason::SinkFailed
                                | BinaryAbortReason::ProtocolError
                        )
                        && terminal_state.valid_receiver_terminal(accepted_through) =>
                    {
                        // Crossed ABORT: the daemon's already-chosen terminal is
                        // authoritative, but the peer's ABORT still gets its own
                        // explicit ACK before we continue waiting for ours.
                        if !send_receiver_abort_ack(&context, accepted_through, ack_deadline).await
                        {
                            break;
                        }
                    }
                    MailboxEventBody::Record(StreamRecordBody::Credit {
                        accepted_through,
                        send_through,
                    }) if received_in_time
                        && terminal_state
                            .credit(accepted_through, send_through)
                            .is_ok() => {}
                    MailboxEventBody::Malformed => {
                        let _ = acknowledge_retained_fault_abort(
                            &context,
                            &terminal_state,
                            ack_deadline,
                        )
                        .await;
                        break;
                    }
                    _ => break,
                }
                continue;
            }
            DaemonAckPoll::TimedOut | DaemonAckPoll::Closed => break,
            DaemonAckPoll::Idle => {}
        }
        tokio::select! {
            biased;
            () = context.binding.ingress.mailbox.wait_ready() => {}
            () = tokio::time::sleep_until(ack_deadline) => {}
        }
    }

    let acknowledged = transferred.is_some();
    let transferred = transferred.unwrap_or(terminal_state.accepted);
    // ACK (or its bounded timeout) is the binding retirement point. The
    // request id remains separately outstanding through the Text reply.
    context.binding.retire();
    let reply_deadline = Instant::now()
        .checked_add(ack_timeout)
        .expect("served stall duration was validated");
    let replied = send_reply_until(
        context.outbound,
        id,
        Err(daemon_error(failure, transferred, state.total)),
        reply_deadline,
        request,
        None,
    )
    .await;
    if !acknowledged || !replied {
        signal_transport_close(context.closer);
    }
    replied
}

/// Execute one complete producer-direction `file.read` request.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_file_read(
    engine: Arc<Engine>,
    request: FileRead,
    id: u64,
    request_permit: RequestPermit,
    outbound: Outbound,
    registry: StreamRegistry,
    stream_ids: Arc<Mutex<StreamIdGenerator>>,
    transfer_pool: TransferPool,
    limits: RuntimeLimits,
    closer: ConnectionCloser,
) -> bool {
    let request_lease = RequestLease::new(request_permit);
    let prepared = match engine.prepare_file_read(&request).await {
        Ok(prepared) => prepared,
        Err(error) => return send_reply(&outbound, id, Err(error), &request_lease).await,
    };
    let metadata = prepared.out;
    let total = metadata.bytes;

    // Successful reservation is the transfer's admission instant. Source
    // setup follows it and is charged to the checked absolute budget.
    let admitted_at = Instant::now();
    let reservation = match transfer_pool.reserve(total) {
        Ok(reservation) => reservation,
        Err(error) => return send_reply(&outbound, id, Err(error), &request_lease).await,
    };
    let mut reservation = Some(reservation);
    let (absolute_deadline, budget_ms) = match limits.deadline(admitted_at, total) {
        Ok(deadline) => deadline,
        Err(_) => {
            drop(reservation.take());
            return send_reply(&outbound, id, Err(ApiError::NotReady), &request_lease).await;
        }
    };

    let source = tokio::select! {
        biased;
        () = tokio::time::sleep_until(absolute_deadline) => {
            drop(reservation.take());
            return send_reply(
                &outbound,
                id,
                Err(ApiError::TransferDeadlineExceeded {
                    transferred_bytes: 0,
                    total: known_total(total),
                    budget_ms,
                }),
                &request_lease,
            )
            .await;
        }
        opened = prepared.source.open() => match opened {
            Ok(source) => source,
            Err(_) => {
                drop(reservation.take());
                return send_reply(
                    &outbound,
                    id,
                    Err(ApiError::FileNotFetched {
                        file_id: request.file_id.clone(),
                    }),
                    &request_lease,
                )
                .await;
            }
        }
    };

    let identity_result = {
        let mut generator = stream_ids.lock().expect("stream id generator poisoned");
        generator.next(id)
    };
    let identity = match identity_result {
        Ok(identity) => identity,
        Err(_) => {
            drop(source);
            drop(reservation.take());
            return send_reply(&outbound, id, Err(ApiError::NotReady), &request_lease).await;
        }
    };
    let Some(binding) = registry.bind(identity) else {
        drop(source);
        drop(reservation.take());
        return send_reply(&outbound, id, Err(ApiError::NotReady), &request_lease).await;
    };
    let bounds = CodecBounds {
        max_frame_bytes: limits.max_frame_bytes(),
        ..CodecBounds::default()
    };
    let Some(open) = stream_record(identity, StreamRecordBody::Open { total }, &bounds) else {
        binding.retire();
        drop(source);
        drop(reservation.take());
        return send_reply(&outbound, id, Err(ApiError::NotReady), &request_lease).await;
    };
    if Instant::now() >= absolute_deadline {
        binding.ingress.data_live.store(false, Ordering::Release);
        binding.retire();
        drop(source);
        drop(reservation.take());
        let reply_deadline = limits
            .stall_deadline(Instant::now())
            .expect("served stall duration was validated");
        return send_reply_until(
            &outbound,
            id,
            Err(ApiError::TransferDeadlineExceeded {
                transferred_bytes: 0,
                total: known_total(total),
                budget_ms,
            }),
            reply_deadline,
            &request_lease,
            None,
        )
        .await;
    }
    let open_result = send_phase_control_until(
        &StreamTerminalContext {
            outbound: &outbound,
            binding: &binding,
            bounds: &bounds,
            closer: &closer,
        },
        open,
        absolute_deadline,
    )
    .await;
    match open_result {
        WriteReceipt::Sent => debug_assert_eq!(binding.phase(), BindingPhase::Active),
        WriteReceipt::Discarded if binding.phase() == BindingPhase::Opening => {
            let opening_fatal = binding.ingress.opening_fatal.load(Ordering::Acquire);
            binding.retire();
            drop(source);
            drop(reservation.take());
            if opening_fatal {
                // The connection owner has already selected the mandatory
                // uncorrelated 4007 path. Do not race it with a Text reply.
                closer.malformed();
                return false;
            }
            let reply_deadline = limits
                .stall_deadline(Instant::now())
                .expect("served stall duration was validated");
            let replied = send_reply_until(
                &outbound,
                id,
                Err(ApiError::TransferDeadlineExceeded {
                    transferred_bytes: 0,
                    total: known_total(total),
                    budget_ms,
                }),
                reply_deadline,
                &request_lease,
                None,
            )
            .await;
            if !replied {
                signal_transport_close(&closer);
            }
            return replied;
        }
        WriteReceipt::Discarded | WriteReceipt::Closed => {
            closer.malformed();
            return false;
        }
    }

    let mut source = Some(source);
    let reservation = Arc::new(Mutex::new(reservation));
    let mut state = ProducerState::new(total);
    let mut stall_deadline = limits
        .stall_deadline(Instant::now())
        .expect("served stall duration was validated");
    let timing = ActiveTiming {
        absolute_deadline,
        budget_ms,
        limits,
    };

    let stop = 'active: loop {
        match poll_active(&binding, &mut state, &mut stall_deadline, timing) {
            Ok(ActivePoll::Event) => continue,
            Ok(ActivePoll::Idle) => {}
            Err(stop) => break stop,
        }

        if state.ready_for_end() {
            let verify = source
                .take()
                .expect("active stream owns its source")
                .verify_eof();
            match await_active(verify, &binding, &mut state, &mut stall_deadline, timing).await {
                Ok(Ok(())) => {
                    let Some(end) =
                        stream_record(identity, StreamRecordBody::End { total }, &bounds)
                    else {
                        return false;
                    };
                    match send_end_while_active(
                        &StreamTerminalContext {
                            outbound: &outbound,
                            binding: &binding,
                            bounds: &bounds,
                            closer: &closer,
                        },
                        end,
                        &mut state,
                        reservation.clone(),
                        &mut stall_deadline,
                        timing,
                    )
                    .await
                    {
                        Ok(WriteReceipt::Sent) => {}
                        Ok(WriteReceipt::Closed) | Err(ActiveStop::TransportLost) => {
                            closer.malformed();
                            return false;
                        }
                        Ok(WriteReceipt::Discarded) => {
                            unreachable!("discarded END resolves to its active stop")
                        }
                        Err(stop) => break stop,
                    }
                    // END's writer acknowledgement is the ordering barrier:
                    // only now may the terminal success Text reply be queued.
                    let reply_deadline = limits
                        .stall_deadline(Instant::now())
                        .expect("served stall duration was validated");
                    let replied = send_reply_until(
                        &outbound,
                        id,
                        Ok(&metadata),
                        reply_deadline,
                        &request_lease,
                        Some(binding.retirement()),
                    )
                    .await;
                    if !replied {
                        signal_transport_close(&closer);
                    }
                    return replied;
                }
                Ok(Err(_)) => break ActiveStop::Daemon(DaemonFailure::Source),
                Err(stop) => break stop,
            }
        }

        if let Some(payload_bytes) = state.next_payload(limits.max_data_payload_bytes()) {
            let Some(record_bytes) = STREAM_HEADER_BYTES.checked_add(payload_bytes) else {
                break ActiveStop::Daemon(DaemonFailure::Protocol);
            };
            let Some(data_reservation) = outbound.reserve_data(record_bytes) else {
                // Runtime configuration reserves one complete DATA record per
                // admitted nonempty stream; failure here indicates transport
                // teardown rather than permission to read ahead.
                closer.malformed();
                return false;
            };
            let read = source
                .as_mut()
                .expect("active stream owns its source")
                .read_chunk(NonZeroUsize::new(payload_bytes).expect("positive payload"));
            let payload =
                match await_active(read, &binding, &mut state, &mut stall_deadline, timing).await {
                    Ok(Ok(Some(payload))) if payload.len() == payload_bytes => payload,
                    Ok(Ok(_)) | Ok(Err(_)) => break ActiveStop::Daemon(DaemonFailure::Source),
                    Err(stop) => break stop,
                };

            let offset = state.sent;
            let payload_len = payload.len();
            let Some(data) = stream_record(
                identity,
                StreamRecordBody::Data { offset, payload },
                &bounds,
            ) else {
                break ActiveStop::Daemon(DaemonFailure::Protocol);
            };
            let data = tokio_tungstenite::tungstenite::Bytes::from(data);
            'data: loop {
                let data_ingress = binding.ingress.clone();
                let data_absolute_deadline = timing.absolute_deadline;
                let data_stall_deadline = stall_deadline;
                let Some(receipt) = outbound.queue_data_with_start(
                    data_reservation.clone(),
                    data.clone(),
                    binding.ingress.data_live.clone(),
                    move || {
                        data_ingress.try_start_data(data_absolute_deadline, data_stall_deadline)
                    },
                ) else {
                    closer.malformed();
                    return false;
                };
                match await_data_write(
                    receipt,
                    &binding,
                    &mut state,
                    payload_len,
                    &mut stall_deadline,
                    timing,
                )
                .await
                {
                    Ok(WriteReceipt::Sent) => continue 'active,
                    Ok(WriteReceipt::Discarded) => {
                        // DATA bytes and their byte permit remain retained in
                        // this bounded actor slot. Drain every earlier inbound
                        // record, then retry the exact same bytes and offset;
                        // the source cursor is never advanced twice.
                        loop {
                            match poll_active(&binding, &mut state, &mut stall_deadline, timing) {
                                Ok(ActivePoll::Event) => continue,
                                Ok(ActivePoll::Idle) => break,
                                Err(stop) => break 'active stop,
                            }
                        }
                        if binding.phase() == BindingPhase::Active
                            && binding.ingress.data_live.load(Ordering::Acquire)
                        {
                            continue 'data;
                        }
                        break 'active await_active_stop(
                            &binding,
                            &mut state,
                            &mut stall_deadline,
                            timing,
                        )
                        .await;
                    }
                    Ok(WriteReceipt::Closed) | Err(ActiveStop::TransportLost) => {
                        closer.malformed();
                        return false;
                    }
                    Err(stop) => break 'active stop,
                }
            }
        }

        // Correct credit pause: only transfer deadline/stall and inbound
        // records govern it. The ordinary connection idle timer is suppressed
        // while the registry retains this binding.
        tokio::select! {
            biased;
            () = binding.ingress.mailbox.wait_ready() => {}
            () = tokio::time::sleep_until(timing.absolute_deadline) => {}
            () = tokio::time::sleep_until(stall_deadline) => {}
        }
    };

    let stop = claim_local_stop(&binding, &mut state, &mut stall_deadline, timing, stop);

    // The local terminal decision releases source and both admission
    // resources before any ABORT acknowledgement wait.
    binding.ingress.stop_active_data();
    drop(source.take());
    drop(
        reservation
            .lock()
            .expect("transfer reservation poisoned")
            .take(),
    );
    match stop {
        ActiveStop::ClientAbort { accepted, reason } => {
            finish_client_abort(
                StreamTerminalContext {
                    outbound: &outbound,
                    binding: &binding,
                    bounds: &bounds,
                    closer: &closer,
                },
                id,
                &request_lease,
                &state,
                accepted,
                reason,
                timing.limits.transfer_stall(),
            )
            .await
        }
        ActiveStop::Daemon(failure) => {
            finish_daemon_abort(
                StreamTerminalContext {
                    outbound: &outbound,
                    binding: &binding,
                    bounds: &bounds,
                    closer: &closer,
                },
                id,
                &request_lease,
                &state,
                failure,
                timing.limits.transfer_stall(),
                None,
            )
            .await
        }
        ActiveStop::TransportLost => false,
    }
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;
    use std::pin::Pin;
    use std::task::{Context, Poll, Waker};

    use futures_util::Sink;
    use tempfile::TempDir;
    use tokio_tungstenite::tungstenite::Message;

    use super::*;

    const REQUEST_ID: u64 = 17;
    const STREAM_ID: u128 = 0x1111_2222_3333_4444_5555_6666_7777_8888;

    fn bounds(max_frame_bytes: usize) -> CodecBounds {
        CodecBounds {
            max_frame_bytes,
            ..CodecBounds::default()
        }
    }

    /// Spec-authored raw record constructor, deliberately independent of the
    /// implementation encoder under test.
    fn wire(
        kind: u8,
        request_id: u64,
        stream_id: u128,
        offset: u64,
        value: u64,
        payload: &[u8],
    ) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(STREAM_HEADER_BYTES + payload.len());
        bytes.extend_from_slice(b"JBS2");
        bytes.push(kind);
        bytes.extend_from_slice(&[0, 0, 0]);
        bytes.extend_from_slice(&request_id.to_be_bytes());
        bytes.extend_from_slice(&stream_id.to_be_bytes());
        bytes.extend_from_slice(&offset.to_be_bytes());
        bytes.extend_from_slice(&value.to_be_bytes());
        bytes.extend_from_slice(payload);
        bytes
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct StreamedDataSummary {
        through: u64,
        records: usize,
        max_payload: usize,
        identity: Option<[u8; 24]>,
    }

    #[derive(Default)]
    struct StreamedDataState {
        summary: StreamedDataSummary,
        error: Option<String>,
    }

    impl StreamedDataState {
        fn accept(&mut self, bytes: &[u8]) -> Result<(), String> {
            if bytes.len() <= STREAM_HEADER_BYTES {
                return Err(format!("DATA record was too short: {}", bytes.len()));
            }
            if &bytes[..4] != b"JBS2" || bytes[4] != 0x02 {
                return Err("discarded message was not a JBS2 DATA record".into());
            }
            if bytes[5..8] != [0, 0, 0] {
                return Err("DATA reserved bytes were nonzero".into());
            }
            let identity: [u8; 24] = bytes[8..32]
                .try_into()
                .expect("fixed stream identity slice");
            if self
                .summary
                .identity
                .is_some_and(|expected| expected != identity)
            {
                return Err("DATA changed stream identity".into());
            }
            let offset =
                u64::from_be_bytes(bytes[32..40].try_into().expect("fixed DATA offset slice"));
            if offset != self.summary.through {
                return Err(format!(
                    "noncontiguous DATA offset {offset}, expected {}",
                    self.summary.through
                ));
            }
            if bytes[40..48] != [0; 8] {
                return Err("DATA value field was nonzero".into());
            }
            let payload = &bytes[STREAM_HEADER_BYTES..];
            if payload.iter().any(|byte| *byte != 0) {
                return Err("sparse zero source produced a nonzero DATA byte".into());
            }
            let payload_len = u64::try_from(payload.len())
                .map_err(|_| "DATA payload count was not representable".to_owned())?;
            self.summary.through = self
                .summary
                .through
                .checked_add(payload_len)
                .ok_or_else(|| "DATA byte count overflowed".to_owned())?;
            self.summary.records = self
                .summary
                .records
                .checked_add(1)
                .ok_or_else(|| "DATA record count overflowed".to_owned())?;
            self.summary.max_payload = self.summary.max_payload.max(payload.len());
            self.summary.identity = Some(identity);
            Ok(())
        }
    }

    struct FlushGate {
        block_next: AtomicBool,
        blocked: AtomicBool,
        released: AtomicBool,
        waker: Mutex<Option<Waker>>,
        changed: Notify,
    }

    impl Default for FlushGate {
        fn default() -> Self {
            Self {
                block_next: AtomicBool::new(false),
                blocked: AtomicBool::new(false),
                released: AtomicBool::new(true),
                waker: Mutex::new(None),
                changed: Notify::new(),
            }
        }
    }

    impl FlushGate {
        fn arm(&self) {
            assert!(!self.blocked.load(Ordering::Acquire));
            self.released.store(false, Ordering::Release);
            self.block_next.store(true, Ordering::Release);
        }

        fn message_started(&self) {
            if self.block_next.swap(false, Ordering::AcqRel) {
                self.blocked.store(true, Ordering::Release);
                self.changed.notify_waiters();
                self.changed.notify_one();
            }
        }

        fn poll_flush(&self, cx: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
            if !self.blocked.load(Ordering::Acquire) {
                return Poll::Ready(Ok(()));
            }
            if self.released.load(Ordering::Acquire) {
                self.blocked.store(false, Ordering::Release);
                return Poll::Ready(Ok(()));
            }
            *self.waker.lock().expect("flush gate poisoned") = Some(cx.waker().clone());
            if self.released.load(Ordering::Acquire) {
                self.blocked.store(false, Ordering::Release);
                Poll::Ready(Ok(()))
            } else {
                Poll::Pending
            }
        }

        async fn wait_blocked(&self) {
            loop {
                let notified = self.changed.notified();
                if self.blocked.load(Ordering::Acquire) {
                    return;
                }
                notified.await;
            }
        }

        fn release(&self) {
            self.released.store(true, Ordering::Release);
            if let Some(waker) = self.waker.lock().expect("flush gate poisoned").take() {
                waker.wake();
            }
        }
    }

    #[derive(Clone, Default)]
    struct RecordingSink {
        messages: Arc<Mutex<Vec<Message>>>,
        changed: Arc<Notify>,
        streamed_data: Option<Arc<Mutex<StreamedDataState>>>,
        flush_gate: Arc<FlushGate>,
    }

    impl Sink<Message> for RecordingSink {
        type Error = Infallible;

        fn poll_ready(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn start_send(self: Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
            if let (Some(streamed_data), Message::Binary(bytes)) = (&self.streamed_data, &item) {
                if bytes.get(4) == Some(&0x02) {
                    let mut state = streamed_data.lock().expect("streaming sink poisoned");
                    if state.error.is_none() {
                        if let Err(error) = state.accept(bytes) {
                            state.error = Some(error);
                        }
                    }
                    drop(state);
                    self.changed.notify_waiters();
                    return Ok(());
                }
            }
            self.messages
                .lock()
                .expect("recording sink poisoned")
                .push(item);
            self.changed.notify_waiters();
            self.flush_gate.message_started();
            Ok(())
        }

        fn poll_flush(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            self.flush_gate.poll_flush(_cx)
        }

        fn poll_close(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }
    }

    impl RecordingSink {
        fn discarding_zero_data() -> Self {
            Self {
                streamed_data: Some(Arc::new(Mutex::new(StreamedDataState::default()))),
                ..Self::default()
            }
        }

        async fn message(&self, index: usize) -> Message {
            loop {
                let notified = self.changed.notified();
                if let Some(message) = self
                    .messages
                    .lock()
                    .expect("recording sink poisoned")
                    .get(index)
                    .cloned()
                {
                    return message;
                }
                notified.await;
            }
        }

        fn block_next_flush(&self) {
            self.flush_gate.arm();
        }

        async fn wait_flush_blocked(&self) {
            self.flush_gate.wait_blocked().await;
        }

        fn release_flush(&self) {
            self.flush_gate.release();
        }

        async fn assert_no_message(&self, index: usize) {
            assert!(
                tokio::time::timeout(Duration::from_millis(20), self.message(index))
                    .await
                    .is_err(),
                "unexpected outbound message at index {index}"
            );
        }

        async fn streamed_through(&self, previous: u64) -> u64 {
            let streamed_data = self
                .streamed_data
                .as_ref()
                .expect("streaming DATA capture was not configured");
            loop {
                let notified = self.changed.notified();
                let through = {
                    let state = streamed_data.lock().expect("streaming sink poisoned");
                    if let Some(error) = &state.error {
                        panic!("invalid streamed DATA: {error}");
                    }
                    state.summary.through
                };
                if through > previous {
                    return through;
                }
                notified.await;
            }
        }

        fn streamed_summary(&self) -> StreamedDataSummary {
            let state = self
                .streamed_data
                .as_ref()
                .expect("streaming DATA capture was not configured")
                .lock()
                .expect("streaming sink poisoned");
            if let Some(error) = &state.error {
                panic!("invalid streamed DATA: {error}");
            }
            state.summary
        }

        fn retained_payload_bytes(&self) -> usize {
            self.messages
                .lock()
                .expect("recording sink poisoned")
                .iter()
                .map(|message| match message {
                    Message::Text(text) => text.len(),
                    Message::Binary(bytes) | Message::Ping(bytes) | Message::Pong(bytes) => {
                        bytes.len()
                    }
                    Message::Close(Some(frame)) => frame.reason.len().saturating_add(2),
                    Message::Close(None) | Message::Frame(_) => 0,
                })
                .sum()
        }
    }

    struct RuntimeHarness {
        _dir: TempDir,
        engine: Arc<Engine>,
        request: FileRead,
        source_path: std::path::PathBuf,
        limits: RuntimeLimits,
        pool: TransferPool,
        registry: StreamRegistry,
        tracker: RequestTracker,
        stream_ids: Arc<Mutex<StreamIdGenerator>>,
        outbound: Outbound,
        sink: RecordingSink,
        writer: tokio::task::JoinHandle<()>,
        closer: ConnectionCloser,
        close_rx: watch::Receiver<bool>,
        _close_completed_rx: watch::Receiver<bool>,
    }

    impl RuntimeHarness {
        async fn new(payload: &[u8], max_frame_bytes: u64) -> Self {
            Self::new_with_timers(payload, max_frame_bytes, 1_000, 1_000, 8_192).await
        }

        async fn new_sparse_zero(total: u64, max_frame_bytes: u64) -> Self {
            Self::new_configured(None, total, max_frame_bytes, 10_000, 1_000, 8_192, true).await
        }

        async fn new_with_timers(
            payload: &[u8],
            max_frame_bytes: u64,
            stall_ms: u64,
            connect_ms: u64,
            floor_bps: u64,
        ) -> Self {
            Self::new_configured(
                Some(payload),
                payload.len() as u64,
                max_frame_bytes,
                stall_ms,
                connect_ms,
                floor_bps,
                false,
            )
            .await
        }

        async fn new_configured(
            payload: Option<&[u8]>,
            source_bytes: u64,
            max_frame_bytes: u64,
            stall_ms: u64,
            connect_ms: u64,
            floor_bps: u64,
            discard_data: bool,
        ) -> Self {
            let dir = TempDir::new().expect("runtime tempdir");
            let (shutdown_tx, _shutdown_rx) = tokio::sync::mpsc::channel(1);
            let engine = Engine::new(
                dir.path().to_path_buf(),
                true,
                jeliya_core::engine::EngineConfig {
                    port: 0,
                    version: "test".into(),
                    shutdown_tx,
                },
            )
            .expect("test engine");
            engine
                .execute(jeliya_core::typed::TypedCall::SubjectEnsure(
                    jeliya_api::SubjectEnsure {},
                ))
                .await
                .reply
                .expect("subject.ensure");
            let created = engine
                .execute(jeliya_core::typed::TypedCall::RoomCreate(
                    jeliya_api::RoomCreate {
                        name: "stream runtime".into(),
                    },
                ))
                .await
                .reply
                .expect("room.create");
            let jeliya_core::typed::TypedReply::RoomCreate(created) = created else {
                panic!("wrong room.create reply");
            };
            engine
                .execute(jeliya_core::typed::TypedCall::RoomActivate(
                    jeliya_api::RoomActivate {
                        room_id: created.room_id.clone(),
                    },
                ))
                .await
                .reply
                .expect("room.activate");
            let staged = dir.path().join("stream-source.bin");
            match payload {
                Some(payload) => {
                    assert_eq!(payload.len() as u64, source_bytes);
                    std::fs::write(&staged, payload).expect("write staged source");
                }
                None => std::fs::File::create(&staged)
                    .and_then(|file| file.set_len(source_bytes))
                    .expect("create sparse staged source"),
            }
            let shared = engine
                .share_staged_file(
                    &jeliya_api::FileShare {
                        room_id: created.room_id.clone(),
                        name: "stream-source.bin".into(),
                        declared_bytes: source_bytes,
                        declared_content_type: "application/octet-stream".into(),
                    },
                    &staged,
                )
                .await
                .expect("host-staged share");
            // Fixture setup marks the staged bytes as a verified local fetch.
            // This writes core's existing private state format only in the
            // daemon test; the production streaming API still exposes no path.
            let state_path = dir.path().join("state.json");
            let mut local_state: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&state_path).expect("read local state"))
                    .expect("decode local state");
            local_state["rooms"][created.room_id.as_str()]["fetched_files"]
                [shared.file_id.as_str()] = serde_json::json!({
                "path": staged.clone(),
                "bytes": source_bytes,
                "fetched_at_ms": 0,
            });
            std::fs::write(
                &state_path,
                serde_json::to_vec_pretty(&local_state).expect("encode local state"),
            )
            .expect("write local state");
            let request = FileRead {
                room_id: created.room_id,
                file_id: shared.file_id,
            };

            let mut served = engine.limits();
            served.max_frame_bytes = max_frame_bytes;
            served.max_concurrent_transfers = 2;
            served.max_transfer_bytes_inflight = source_bytes.max(2 * 1024 * 1024);
            served.transfer_stall_ms = stall_ms;
            served.transfer_connect_allowance_ms = connect_ms;
            served.transfer_floor_bits_per_second = floor_bps;
            let limits = RuntimeLimits::from_served(&served).expect("test runtime limits");
            let pool = TransferPool::from_runtime(&limits);
            let registry = StreamRegistry::new();
            let tracker = RequestTracker::new(4);
            let stream_ids = Arc::new(Mutex::new(StreamIdGenerator::new()));
            let (outbound, queues) = Outbound::new(
                16,
                2,
                limits.control_queue_capacity_bytes(),
                limits.data_queue_capacity_bytes(),
                limits.max_frame_bytes(),
            );
            let sink = if discard_data {
                RecordingSink::discarding_zero_data()
            } else {
                RecordingSink::default()
            };
            let writer_sink = sink.clone();
            let writer = tokio::spawn(async move {
                let result = queues.run(writer_sink, limits.transfer_stall()).await;
                assert!(result.is_ok());
            });
            let (closer, close_rx, close_completed_rx) =
                ConnectionCloser::new(registry.clone(), outbound.clone());
            Self {
                _dir: dir,
                engine,
                request,
                source_path: staged,
                limits,
                pool,
                registry,
                tracker,
                stream_ids,
                outbound,
                sink,
                writer,
                closer,
                close_rx,
                _close_completed_rx: close_completed_rx,
            }
        }

        fn spawn_read(&self, id: u64) -> tokio::task::JoinHandle<bool> {
            let permit = self.tracker.acquire(id).expect("request permit");
            tokio::spawn(run_file_read(
                self.engine.clone(),
                self.request.clone(),
                id,
                permit,
                self.outbound.clone(),
                self.registry.clone(),
                self.stream_ids.clone(),
                self.pool.clone(),
                self.limits,
                self.closer.clone(),
            ))
        }

        async fn shutdown(self) {
            let Self {
                outbound,
                closer,
                writer,
                ..
            } = self;
            drop(closer);
            drop(outbound);
            writer.await.expect("writer join");
        }
    }

    fn binary(message: Message) -> Vec<u8> {
        let Message::Binary(bytes) = message else {
            panic!("expected Binary stream record, got {message:?}");
        };
        bytes.to_vec()
    }

    fn text_reply(message: Message) -> jeliya_codec::Reply {
        let Message::Text(text) = message else {
            panic!("expected Text reply");
        };
        serde_json::from_str(&text).expect("typed reply JSON")
    }

    async fn assert_success_flow(payload: Vec<u8>, max_frame_bytes: u64) {
        let harness = RuntimeHarness::new(&payload, max_frame_bytes).await;
        let actor = harness.spawn_read(REQUEST_ID);
        let open = decode_stream_record(
            &binary(harness.sink.message(0).await),
            &bounds(max_frame_bytes as usize),
        )
        .expect("OPEN record");
        assert_eq!(
            open.body,
            StreamRecordBody::Open {
                total: payload.len() as u64
            }
        );
        let identity = open.identity;
        assert_eq!(identity.request_id().get(), REQUEST_ID);
        assert_ne!(identity.stream_id().get(), 0);

        let total = payload.len() as u64;
        let initial = wire(0x03, REQUEST_ID, identity.stream_id().get(), 0, total, &[]);
        assert_eq!(
            harness
                .registry
                .route_binary(&initial, &bounds(max_frame_bytes as usize)),
            BinaryRoute::Delivered
        );

        let mut accepted = 0_u64;
        let mut observed = Vec::new();
        let mut index = 1;
        loop {
            let record = decode_stream_record(
                &binary(harness.sink.message(index).await),
                &bounds(max_frame_bytes as usize),
            )
            .expect("outbound stream record");
            index += 1;
            assert_eq!(record.identity, identity);
            match record.body {
                StreamRecordBody::Data { offset, payload } => {
                    assert_eq!(offset, accepted);
                    assert!(!payload.is_empty());
                    assert!(payload.len() <= max_frame_bytes as usize - STREAM_HEADER_BYTES);
                    accepted += payload.len() as u64;
                    observed.extend_from_slice(&payload);
                    let credit = wire(
                        0x03,
                        REQUEST_ID,
                        identity.stream_id().get(),
                        accepted,
                        total,
                        &[],
                    );
                    assert_eq!(
                        harness
                            .registry
                            .route_binary(&credit, &bounds(max_frame_bytes as usize)),
                        BinaryRoute::Delivered
                    );
                }
                StreamRecordBody::End { total: ended } => {
                    assert_eq!(ended, total);
                    assert_eq!(accepted, total);
                    break;
                }
                other => panic!("unexpected producer record: {other:?}"),
            }
        }
        assert_eq!(observed, payload);
        let reply = text_reply(harness.sink.message(index).await);
        assert!(reply.ok);
        assert_eq!(reply.id, REQUEST_ID);
        assert_eq!(
            reply.out.as_ref().and_then(|out| out["bytes"].as_u64()),
            Some(total)
        );
        for _ in 0..20 {
            if !harness.tracker.is_outstanding(REQUEST_ID) && !harness.registry.is_active() {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(
            !harness.tracker.is_outstanding(REQUEST_ID),
            "terminal Text send releases the request id before actor return"
        );
        assert!(
            !harness.registry.is_active(),
            "terminal success Text send retires the exact pair"
        );
        drop(
            harness
                .tracker
                .acquire(REQUEST_ID)
                .expect("peer-visible reply makes the request id reusable"),
        );
        assert_eq!(
            harness.registry.route_binary(
                &wire(
                    0x03,
                    REQUEST_ID,
                    identity.stream_id().get(),
                    total,
                    total,
                    &[],
                ),
                &bounds(max_frame_bytes as usize),
            ),
            BinaryRoute::CloseMalformed,
            "completed downloads retain no resumable binding"
        );
        assert!(actor.await.expect("file.read actor"));
        harness.shutdown().await;
    }

    async fn opened_identity(harness: &RuntimeHarness, id: u64) -> StreamIdentity {
        let open = decode_stream_record(
            &binary(harness.sink.message(0).await),
            &bounds(harness.limits.max_frame_bytes()),
        )
        .expect("OPEN record");
        assert!(matches!(open.body, StreamRecordBody::Open { .. }));
        assert_eq!(open.identity.request_id().get(), id);
        open.identity
    }

    fn route_body(
        harness: &RuntimeHarness,
        identity: StreamIdentity,
        body: StreamRecordBody,
    ) -> BinaryRoute {
        let (kind, offset, value) = match body {
            StreamRecordBody::Credit {
                accepted_through,
                send_through,
            } => (0x03, accepted_through, send_through),
            StreamRecordBody::Abort {
                accepted_through,
                reason,
            } => {
                let reason = match reason {
                    BinaryAbortReason::Cancelled => 0x01,
                    BinaryAbortReason::SourceFailed => 0x02,
                    BinaryAbortReason::SinkFailed => 0x03,
                    BinaryAbortReason::ProtocolError => 0x04,
                    BinaryAbortReason::OperationError => 0x05,
                };
                (0x05, accepted_through, reason)
            }
            StreamRecordBody::Ack { accepted_through } => (0x06, accepted_through, 0x05),
            other => panic!("test helper only constructs receiver controls: {other:?}"),
        };
        let record = wire(
            kind,
            identity.request_id().get(),
            identity.stream_id().get(),
            offset,
            value,
            &[],
        );
        harness
            .registry
            .route_binary(&record, &bounds(harness.limits.max_frame_bytes()))
    }

    #[tokio::test]
    async fn zero_one_multi_and_payload_boundary_reads_follow_open_credit_data_end_reply() {
        assert_success_flow(Vec::new(), 256).await;
        assert_success_flow(vec![0x7a], 256).await;
        assert_success_flow((0..300).map(|i| (i % 251) as u8).collect(), 256).await;
        let payload_limit = 256 - STREAM_HEADER_BYTES;
        assert_success_flow(vec![0x5a; payload_limit], 256).await;
        assert_success_flow(vec![0x5a; payload_limit + 1], 256).await;
    }

    #[tokio::test]
    async fn pending_credit_retries_the_same_bounded_data_without_rereading_source() {
        let harness = RuntimeHarness::new(&[0x11, 0x22, 0x33], 256).await;
        let actor = harness.spawn_read(REQUEST_ID);
        let identity = opened_identity(&harness, REQUEST_ID).await;

        harness.sink.block_next_flush();
        let ordinary_out = harness.outbound.clone();
        let ordinary =
            tokio::spawn(
                async move { ordinary_out.text(br#"{"id":99,"ok":true}"#.to_vec()).await },
            );
        harness.sink.wait_flush_blocked().await;

        assert_eq!(
            route_body(
                &harness,
                identity,
                StreamRecordBody::Credit {
                    accepted_through: 0,
                    send_through: 2,
                },
            ),
            BinaryRoute::Delivered
        );
        let full_capacity = harness.limits.data_queue_capacity_bytes();
        for _ in 0..100 {
            if harness.outbound.available_data_bytes() < full_capacity {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(
            harness.outbound.available_data_bytes() < full_capacity,
            "the first DATA was read only after retaining its byte permit"
        );
        assert_eq!(
            route_body(
                &harness,
                identity,
                StreamRecordBody::Credit {
                    accepted_through: 0,
                    send_through: 3,
                },
            ),
            BinaryRoute::Delivered
        );

        harness.sink.release_flush();
        assert_eq!(ordinary.await.unwrap(), WriteReceipt::Sent);
        let first = decode_stream_record(&binary(harness.sink.message(2).await), &bounds(256))
            .expect("first DATA after retry");
        assert_eq!(
            first.body,
            StreamRecordBody::Data {
                offset: 0,
                payload: vec![0x11, 0x22],
            }
        );
        route_body(
            &harness,
            identity,
            StreamRecordBody::Credit {
                accepted_through: 2,
                send_through: 3,
            },
        );
        let second = decode_stream_record(&binary(harness.sink.message(3).await), &bounds(256))
            .expect("second DATA");
        assert_eq!(
            second.body,
            StreamRecordBody::Data {
                offset: 2,
                payload: vec![0x33],
            }
        );
        route_body(
            &harness,
            identity,
            StreamRecordBody::Credit {
                accepted_through: 3,
                send_through: 3,
            },
        );
        assert!(matches!(
            decode_stream_record(&binary(harness.sink.message(4).await), &bounds(256))
                .unwrap()
                .body,
            StreamRecordBody::End { total: 3 }
        ));
        assert!(text_reply(harness.sink.message(5).await).ok);
        assert!(actor.await.unwrap());
        harness.shutdown().await;
    }

    #[tokio::test]
    async fn progress_while_data_flushes_is_reconciled_before_stall() {
        let harness = RuntimeHarness::new_with_timers(&[0x7a], 256, 10, 1_000, 8_192).await;
        tokio::time::pause();
        let actor = harness.spawn_read(REQUEST_ID);
        let identity = opened_identity(&harness, REQUEST_ID).await;
        tokio::time::advance(Duration::from_millis(5)).await;

        harness.sink.block_next_flush();
        route_body(
            &harness,
            identity,
            StreamRecordBody::Credit {
                accepted_through: 0,
                send_through: 1,
            },
        );
        let data = decode_stream_record(&binary(harness.sink.message(1).await), &bounds(256))
            .expect("started DATA");
        assert!(matches!(
            data.body,
            StreamRecordBody::Data { offset: 0, .. }
        ));
        harness.sink.wait_flush_blocked().await;

        tokio::time::advance(Duration::from_millis(4)).await;
        route_body(
            &harness,
            identity,
            StreamRecordBody::Credit {
                accepted_through: 1,
                send_through: 1,
            },
        );
        tokio::time::advance(Duration::from_millis(1)).await;
        tokio::task::yield_now().await;
        harness.sink.release_flush();
        tokio::task::yield_now().await;

        assert!(matches!(
            decode_stream_record(&binary(harness.sink.message(2).await), &bounds(256))
                .unwrap()
                .body,
            StreamRecordBody::End { total: 1 }
        ));
        assert!(text_reply(harness.sink.message(3).await).ok);
        assert!(actor.await.unwrap());
        assert!(!*harness.close_rx.borrow());
        harness.shutdown().await;
    }

    #[tokio::test]
    async fn exact_input_before_open_commit_closes_without_a_terminal_text_reply() {
        let harness = RuntimeHarness::new(&[1], 256).await;
        harness.sink.block_next_flush();
        let ordinary_out = harness.outbound.clone();
        let ordinary =
            tokio::spawn(
                async move { ordinary_out.text(br#"{"id":99,"ok":true}"#.to_vec()).await },
            );
        harness.sink.wait_flush_blocked().await;

        let actor = harness.spawn_read(REQUEST_ID);
        let identity = loop {
            if let Some(identity) = harness
                .registry
                .inner
                .lock()
                .expect("stream registry poisoned")
                .bindings
                .keys()
                .next()
                .copied()
            {
                break identity;
            }
            tokio::task::yield_now().await;
        };
        assert_eq!(
            route_body(
                &harness,
                identity,
                StreamRecordBody::Credit {
                    accepted_through: 0,
                    send_through: 1,
                },
            ),
            BinaryRoute::CloseMalformed
        );
        harness.sink.release_flush();
        assert_eq!(ordinary.await.unwrap(), WriteReceipt::Sent);
        assert!(!actor.await.unwrap());
        let Message::Close(Some(close)) = harness.sink.message(1).await else {
            panic!("pre-OPEN exact input must produce the uncorrelated Close");
        };
        assert_eq!(u16::from(close.code), 4007);
        harness.sink.assert_no_message(2).await;
        assert!(*harness.close_rx.borrow());
        assert!(!harness.registry.is_active());
        assert!(!harness.tracker.is_outstanding(REQUEST_ID));
        harness.shutdown().await;
    }

    #[tokio::test]
    async fn connection_fatal_invalidation_discards_a_queued_open_before_close() {
        let harness = RuntimeHarness::new(&[1], 256).await;
        harness.sink.block_next_flush();
        let ordinary_out = harness.outbound.clone();
        let ordinary =
            tokio::spawn(
                async move { ordinary_out.text(br#"{"id":99,"ok":true}"#.to_vec()).await },
            );
        harness.sink.wait_flush_blocked().await;

        let actor = harness.spawn_read(REQUEST_ID);
        loop {
            if harness.registry.is_active() {
                break;
            }
            tokio::task::yield_now().await;
        }

        // The connection owner atomically invalidates every stream writer
        // start and ordinary output before its sole Close task is launched.
        assert!(harness.closer.malformed());
        assert!(!harness.closer.malformed(), "Close ownership is single-use");

        harness.sink.release_flush();
        assert_eq!(ordinary.await.unwrap(), WriteReceipt::Sent);
        let Message::Close(Some(frame)) = harness.sink.message(1).await else {
            panic!("fatal invalidation must send Close after the older started message");
        };
        assert_eq!(u16::from(frame.code), 4007);
        assert!(*harness.close_rx.borrow());
        assert!(!actor.await.unwrap());
        harness.sink.assert_no_message(2).await;
        harness.shutdown().await;
    }

    #[tokio::test]
    async fn maximum_size_read_completes_without_retaining_the_file_in_the_sink() {
        const TOTAL: u64 = 104_857_600;
        const FRAME_BYTES: u64 = (STREAM_HEADER_BYTES + 65_536) as u64;

        // A sparse zero file exercises the complete configured file-size
        // maximum without allocating a 100 MiB input fixture. The sink below
        // validates each DATA record in place and drops it immediately.
        let harness = RuntimeHarness::new_sparse_zero(TOTAL, FRAME_BYTES).await;
        let actor = harness.spawn_read(REQUEST_ID);
        let open = decode_stream_record(
            &binary(harness.sink.message(0).await),
            &bounds(FRAME_BYTES as usize),
        )
        .expect("maximum-size OPEN record");
        assert_eq!(open.body, StreamRecordBody::Open { total: TOTAL });
        let identity = open.identity;

        let initial = wire(0x03, REQUEST_ID, identity.stream_id().get(), 0, TOTAL, &[]);
        assert_eq!(
            harness
                .registry
                .route_binary(&initial, &bounds(FRAME_BYTES as usize)),
            BinaryRoute::Delivered
        );

        let mut accepted = 0_u64;
        while accepted < TOTAL {
            accepted = harness.sink.streamed_through(accepted).await;
            assert!(accepted <= TOTAL);
            let credit = wire(
                0x03,
                REQUEST_ID,
                identity.stream_id().get(),
                accepted,
                TOTAL,
                &[],
            );
            assert_eq!(
                harness
                    .registry
                    .route_binary(&credit, &bounds(FRAME_BYTES as usize)),
                BinaryRoute::Delivered
            );
        }

        let summary = harness.sink.streamed_summary();
        let mut identity_wire = [0_u8; 24];
        identity_wire[..8].copy_from_slice(&REQUEST_ID.to_be_bytes());
        identity_wire[8..].copy_from_slice(&identity.stream_id().get().to_be_bytes());
        assert_eq!(summary.through, TOTAL);
        assert_eq!(summary.records, 1_600);
        assert_eq!(summary.max_payload, 65_536);
        assert_eq!(summary.identity, Some(identity_wire));

        let end = decode_stream_record(
            &binary(harness.sink.message(1).await),
            &bounds(FRAME_BYTES as usize),
        )
        .expect("maximum-size END record");
        assert_eq!(end.identity, identity);
        assert_eq!(end.body, StreamRecordBody::End { total: TOTAL });
        let reply = text_reply(harness.sink.message(2).await);
        assert!(reply.ok);
        assert_eq!(reply.id, REQUEST_ID);
        assert_eq!(
            reply.out.as_ref().and_then(|out| out["bytes"].as_u64()),
            Some(TOTAL)
        );
        assert!(
            harness.sink.retained_payload_bytes() < 4_096,
            "the streaming test sink must retain only OPEN, END, and reply"
        );
        assert!(actor.await.expect("maximum-size file.read actor"));
        assert!(!harness.tracker.is_outstanding(REQUEST_ID));
        harness.shutdown().await;
    }

    #[tokio::test]
    async fn runtime_never_reads_or_sends_beyond_credit_and_waits_for_final_ack() {
        let harness = RuntimeHarness::new(&[1, 2, 3, 4, 5], 256).await;
        let actor = harness.spawn_read(REQUEST_ID);
        let identity = opened_identity(&harness, REQUEST_ID).await;

        harness.sink.assert_no_message(1).await;
        assert_eq!(
            route_body(
                &harness,
                identity,
                StreamRecordBody::Credit {
                    accepted_through: 0,
                    send_through: 2,
                },
            ),
            BinaryRoute::Delivered
        );
        let first = decode_stream_record(&binary(harness.sink.message(1).await), &bounds(256))
            .expect("first DATA");
        assert_eq!(
            first.body,
            StreamRecordBody::Data {
                offset: 0,
                payload: vec![1, 2],
            }
        );
        harness.sink.assert_no_message(2).await;

        route_body(
            &harness,
            identity,
            StreamRecordBody::Credit {
                accepted_through: 2,
                send_through: 5,
            },
        );
        let second = decode_stream_record(&binary(harness.sink.message(2).await), &bounds(256))
            .expect("second DATA");
        assert_eq!(
            second.body,
            StreamRecordBody::Data {
                offset: 2,
                payload: vec![3, 4, 5],
            }
        );

        // Advancing only the send window is not final acknowledgement: no END
        // and no success reply may become observable.
        route_body(
            &harness,
            identity,
            StreamRecordBody::Credit {
                accepted_through: 2,
                send_through: 5,
            },
        );
        harness.sink.assert_no_message(3).await;
        route_body(
            &harness,
            identity,
            StreamRecordBody::Credit {
                accepted_through: 5,
                send_through: 5,
            },
        );
        let end = decode_stream_record(&binary(harness.sink.message(3).await), &bounds(256))
            .expect("END");
        assert_eq!(end.body, StreamRecordBody::End { total: 5 });
        assert!(text_reply(harness.sink.message(4).await).ok);
        assert!(actor.await.unwrap());
        harness.shutdown().await;
    }

    #[tokio::test]
    async fn transfer_count_and_byte_exhaustion_reply_before_open() {
        let count = RuntimeHarness::new(&[1], 256).await;
        let first = count.pool.reserve(0).expect("first count slot");
        let second = count.pool.reserve(0).expect("second count slot");
        let actor = count.spawn_read(REQUEST_ID);
        let reply = text_reply(count.sink.message(0).await);
        assert_eq!(
            reply.err,
            Some(ApiError::ResourceExhausted {
                resource: "max_concurrent_transfers".into(),
                limit: 2,
            })
        );
        assert!(actor.await.unwrap());
        assert!(!count.registry.is_active());
        assert!(!count.tracker.is_outstanding(REQUEST_ID));
        drop((first, second));
        assert_eq!(count.pool.usage(), (0, 0));
        count.shutdown().await;

        let bytes = RuntimeHarness::new(&[1], 256).await;
        let held = bytes
            .pool
            .reserve(2 * 1024 * 1024)
            .expect("entire byte capacity");
        let actor = bytes.spawn_read(REQUEST_ID);
        let reply = text_reply(bytes.sink.message(0).await);
        assert_eq!(
            reply.err,
            Some(ApiError::ResourceExhausted {
                resource: "max_transfer_bytes_inflight".into(),
                limit: 2 * 1024 * 1024,
            })
        );
        assert!(actor.await.unwrap());
        assert!(!bytes.registry.is_active());
        assert!(!bytes.tracker.is_outstanding(REQUEST_ID));
        drop(held);
        assert_eq!(bytes.pool.usage(), (0, 0));
        bytes.shutdown().await;
    }

    #[tokio::test]
    async fn client_abort_is_acked_before_stream_aborted_reply() {
        let harness = RuntimeHarness::new(&[1, 2, 3], 256).await;
        let actor = harness.spawn_read(REQUEST_ID);
        let identity = opened_identity(&harness, REQUEST_ID).await;
        assert_eq!(
            route_body(
                &harness,
                identity,
                StreamRecordBody::Credit {
                    accepted_through: 0,
                    send_through: 3,
                },
            ),
            BinaryRoute::Delivered
        );
        let data =
            decode_stream_record(&binary(harness.sink.message(1).await), &bounds(256)).unwrap();
        assert!(matches!(data.body, StreamRecordBody::Data { .. }));

        assert_eq!(
            route_body(
                &harness,
                identity,
                StreamRecordBody::Abort {
                    accepted_through: 0,
                    reason: BinaryAbortReason::Cancelled,
                },
            ),
            BinaryRoute::Delivered
        );
        let ack =
            decode_stream_record(&binary(harness.sink.message(2).await), &bounds(256)).unwrap();
        assert_eq!(
            ack.body,
            StreamRecordBody::Ack {
                accepted_through: 0
            }
        );
        for _ in 0..20 {
            if !harness.registry.is_active() {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(
            !harness.registry.is_active(),
            "the daemon ACK send retires the client-aborted pair"
        );
        let reply = text_reply(harness.sink.message(3).await);
        assert_eq!(
            reply.err,
            Some(ApiError::StreamAborted {
                transferred_bytes: 0,
                total: known_total(3),
                reason: StreamAbortReason::Cancelled,
            })
        );
        assert!(actor.await.unwrap());
        harness.shutdown().await;
    }

    #[tokio::test]
    async fn source_open_failure_after_admission_releases_every_reservation_without_open() {
        let harness = RuntimeHarness::new(&[1], 256).await;
        std::fs::remove_file(&harness.source_path).expect("remove prepared source before open");
        let actor = harness.spawn_read(REQUEST_ID);

        let reply = text_reply(harness.sink.message(0).await);
        assert_eq!(
            reply.err,
            Some(ApiError::FileNotFetched {
                file_id: harness.request.file_id.clone(),
            })
        );
        assert!(actor.await.unwrap());
        assert_eq!(harness.pool.usage(), (0, 0));
        assert!(!harness.registry.is_active());
        assert!(!harness.tracker.is_outstanding(REQUEST_ID));
        harness.sink.assert_no_message(1).await;
        harness.shutdown().await;
    }

    #[tokio::test]
    async fn buffered_duplicate_abort_becomes_correlated_protocol_failure() {
        let harness = RuntimeHarness::new(&[1], 256).await;
        let actor = harness.spawn_read(REQUEST_ID);
        let identity = opened_identity(&harness, REQUEST_ID).await;
        let abort = StreamRecordBody::Abort {
            accepted_through: 0,
            reason: BinaryAbortReason::Cancelled,
        };
        assert_eq!(
            route_body(&harness, identity, abort.clone()),
            BinaryRoute::Delivered
        );
        assert_eq!(
            route_body(&harness, identity, abort),
            BinaryRoute::Delivered
        );

        let daemon_abort =
            decode_stream_record(&binary(harness.sink.message(1).await), &bounds(256)).unwrap();
        assert_eq!(
            daemon_abort.body,
            StreamRecordBody::Abort {
                accepted_through: 0,
                reason: BinaryAbortReason::ProtocolError,
            }
        );
        route_body(
            &harness,
            identity,
            StreamRecordBody::Ack {
                accepted_through: 0,
            },
        );
        let crossed_ack =
            decode_stream_record(&binary(harness.sink.message(2).await), &bounds(256)).unwrap();
        assert_eq!(
            crossed_ack.body,
            StreamRecordBody::Ack {
                accepted_through: 0
            }
        );
        assert_eq!(
            text_reply(harness.sink.message(3).await).err,
            Some(ApiError::MalformedFrame)
        );
        assert!(actor.await.unwrap());
        harness.shutdown().await;
    }

    #[tokio::test]
    async fn post_client_abort_credit_or_duplicate_abort_promotes_to_crossed_protocol_abort() {
        for trailing in [
            StreamRecordBody::Credit {
                accepted_through: 0,
                send_through: 0,
            },
            StreamRecordBody::Abort {
                accepted_through: 0,
                reason: BinaryAbortReason::Cancelled,
            },
        ] {
            let harness = RuntimeHarness::new(&[1], 256).await;
            let actor = harness.spawn_read(REQUEST_ID);
            let identity = opened_identity(&harness, REQUEST_ID).await;

            harness.sink.block_next_flush();
            let ordinary_out = harness.outbound.clone();
            let ordinary =
                tokio::spawn(
                    async move { ordinary_out.text(br#"{"id":99,"ok":true}"#.to_vec()).await },
                );
            harness.sink.wait_flush_blocked().await;
            assert_eq!(
                route_body(
                    &harness,
                    identity,
                    StreamRecordBody::Abort {
                        accepted_through: 0,
                        reason: BinaryAbortReason::Cancelled,
                    },
                ),
                BinaryRoute::Delivered
            );

            let ingress = loop {
                let ingress = harness
                    .registry
                    .inner
                    .lock()
                    .expect("stream registry poisoned")
                    .bindings
                    .get(&identity)
                    .cloned()
                    .expect("client-aborted pair remains bound")
                    .download();
                if BindingPhase::load(&ingress.phase) == BindingPhase::ClientAbortPending {
                    break ingress;
                }
                tokio::task::yield_now().await;
            };
            assert_eq!(
                route_body(&harness, identity, trailing),
                BinaryRoute::Delivered
            );
            assert!(ingress.mailbox.has_pending());

            harness.sink.release_flush();
            assert_eq!(ordinary.await.unwrap(), WriteReceipt::Sent);
            let daemon_abort =
                decode_stream_record(&binary(harness.sink.message(2).await), &bounds(256)).unwrap();
            assert_eq!(
                daemon_abort.body,
                StreamRecordBody::Abort {
                    accepted_through: 0,
                    reason: BinaryAbortReason::ProtocolError,
                }
            );
            route_body(
                &harness,
                identity,
                StreamRecordBody::Ack {
                    accepted_through: 0,
                },
            );
            let client_abort_ack =
                decode_stream_record(&binary(harness.sink.message(3).await), &bounds(256)).unwrap();
            assert_eq!(
                client_abort_ack.body,
                StreamRecordBody::Ack {
                    accepted_through: 0
                }
            );
            assert_eq!(
                text_reply(harness.sink.message(4).await).err,
                Some(ApiError::MalformedFrame)
            );
            assert!(actor.await.unwrap());
            assert!(!*harness.close_rx.borrow());
            harness.shutdown().await;
        }
    }

    #[tokio::test]
    async fn exact_pair_stays_request_local_while_client_abort_ack_flushes() {
        for trailing in [
            StreamRecordBody::Credit {
                accepted_through: 0,
                send_through: 0,
            },
            StreamRecordBody::Abort {
                accepted_through: 0,
                reason: BinaryAbortReason::Cancelled,
            },
        ] {
            let harness = RuntimeHarness::new(&[1], 256).await;
            let actor = harness.spawn_read(REQUEST_ID);
            let identity = opened_identity(&harness, REQUEST_ID).await;

            // Block the client-ABORT ACK after writer start. Its exact pair is
            // still trustworthy until that peer-visible write is reconciled.
            harness.sink.block_next_flush();
            assert_eq!(
                route_body(
                    &harness,
                    identity,
                    StreamRecordBody::Abort {
                        accepted_through: 0,
                        reason: BinaryAbortReason::Cancelled,
                    },
                ),
                BinaryRoute::Delivered
            );
            harness.sink.wait_flush_blocked().await;
            let ingress = harness
                .registry
                .inner
                .lock()
                .expect("stream registry poisoned")
                .bindings
                .get(&identity)
                .cloned()
                .expect("ACK-in-flight pair remains bound")
                .download();
            assert_eq!(
                BindingPhase::load(&ingress.phase),
                BindingPhase::ClientAbortAckCommitted
            );
            assert_eq!(
                route_body(&harness, identity, trailing),
                BinaryRoute::Delivered,
                "an exact-pair fault during ACK flush is request-local"
            );

            harness.sink.release_flush();
            let first_ack =
                decode_stream_record(&binary(harness.sink.message(1).await), &bounds(256)).unwrap();
            assert_eq!(
                first_ack.body,
                StreamRecordBody::Ack {
                    accepted_through: 0
                }
            );
            let daemon_abort =
                decode_stream_record(&binary(harness.sink.message(2).await), &bounds(256)).unwrap();
            assert_eq!(
                daemon_abort.body,
                StreamRecordBody::Abort {
                    accepted_through: 0,
                    reason: BinaryAbortReason::ProtocolError,
                }
            );
            assert_eq!(
                route_body(
                    &harness,
                    identity,
                    StreamRecordBody::Ack {
                        accepted_through: 0,
                    },
                ),
                BinaryRoute::Delivered
            );
            assert_eq!(
                text_reply(harness.sink.message(3).await).err,
                Some(ApiError::MalformedFrame)
            );
            assert!(actor.await.unwrap());
            assert!(!*harness.close_rx.borrow());
            harness.shutdown().await;
        }
    }

    #[tokio::test]
    async fn bound_protocol_fault_aborts_only_stream_and_waits_for_ack() {
        let harness = RuntimeHarness::new(&[1], 256).await;
        let actor = harness.spawn_read(REQUEST_ID);
        let identity = opened_identity(&harness, REQUEST_ID).await;

        // Client-to-daemon DATA is the wrong direction for a download. The
        // exact pair makes this stream-local rather than a 4007 close.
        assert_eq!(
            harness.registry.route_binary(
                &wire(0x02, REQUEST_ID, identity.stream_id().get(), 0, 0, &[0xaa],),
                &bounds(256),
            ),
            BinaryRoute::Delivered
        );
        let abort =
            decode_stream_record(&binary(harness.sink.message(1).await), &bounds(256)).unwrap();
        assert_eq!(
            abort.body,
            StreamRecordBody::Abort {
                accepted_through: 0,
                reason: BinaryAbortReason::ProtocolError,
            }
        );
        assert_eq!(harness.pool.usage(), (0, 0));
        assert!(harness.registry.is_active(), "binding waits for ACK");
        assert!(harness.tracker.is_outstanding(REQUEST_ID));
        assert_eq!(
            route_body(
                &harness,
                identity,
                StreamRecordBody::Ack {
                    accepted_through: 0,
                },
            ),
            BinaryRoute::Delivered
        );
        assert_eq!(
            route_body(
                &harness,
                identity,
                StreamRecordBody::Ack {
                    accepted_through: 0,
                },
            ),
            BinaryRoute::CloseMalformed,
            "the first ACK latches retirement against duplicate terminals"
        );
        let reply = text_reply(harness.sink.message(2).await);
        assert_eq!(reply.err, Some(ApiError::MalformedFrame));
        assert!(actor.await.unwrap());
        assert_eq!(
            route_body(
                &harness,
                identity,
                StreamRecordBody::Ack {
                    accepted_through: 0,
                },
            ),
            BinaryRoute::CloseMalformed,
            "ACK retires the exact binding before the Text reply completes"
        );
        assert!(!*harness.close_rx.borrow());
        harness.shutdown().await;
    }

    #[tokio::test]
    async fn ack_before_daemon_abort_writer_start_is_not_accepted() {
        let harness = RuntimeHarness::new(&[1], 256).await;
        let actor = harness.spawn_read(REQUEST_ID);
        let identity = opened_identity(&harness, REQUEST_ID).await;

        harness.sink.block_next_flush();
        let ordinary_out = harness.outbound.clone();
        let ordinary =
            tokio::spawn(
                async move { ordinary_out.text(br#"{"id":99,"ok":true}"#.to_vec()).await },
            );
        harness.sink.wait_flush_blocked().await;
        assert_eq!(
            harness.registry.route_binary(
                &wire(0x02, REQUEST_ID, identity.stream_id().get(), 0, 0, &[0xaa]),
                &bounds(256),
            ),
            BinaryRoute::Delivered
        );

        let ingress = loop {
            let ingress = harness
                .registry
                .inner
                .lock()
                .expect("stream registry poisoned")
                .bindings
                .get(&identity)
                .cloned()
                .expect("active exact pair")
                .download();
            if BindingPhase::load(&ingress.phase) == BindingPhase::DaemonAbortQueued {
                break ingress;
            }
            tokio::task::yield_now().await;
        };
        assert_eq!(
            route_body(
                &harness,
                identity,
                StreamRecordBody::Ack {
                    accepted_through: 0,
                },
            ),
            BinaryRoute::Delivered,
            "the exact pair makes premature ACK a correlated protocol fault"
        );
        assert_eq!(
            BindingPhase::load(&ingress.phase),
            BindingPhase::DaemonAbortQueued,
            "ACK is not trustworthy before daemon ABORT writer start"
        );

        harness.sink.release_flush();
        assert_eq!(ordinary.await.unwrap(), WriteReceipt::Sent);
        let abort = decode_stream_record(&binary(harness.sink.message(2).await), &bounds(256))
            .expect("daemon ABORT");
        assert_eq!(
            abort.body,
            StreamRecordBody::Abort {
                accepted_through: 0,
                reason: BinaryAbortReason::ProtocolError,
            }
        );
        assert_eq!(
            text_reply(harness.sink.message(3).await).err,
            Some(ApiError::MalformedFrame)
        );
        let Message::Close(Some(close)) = harness.sink.message(4).await else {
            panic!("premature ACK fault must close after the terminal reply");
        };
        assert_eq!(u16::from(close.code), 4007);
        assert!(actor.await.unwrap());
        harness.shutdown().await;
    }

    #[tokio::test]
    async fn overflowing_credit_drives_correlated_protocol_abort() {
        let harness = RuntimeHarness::new(&[1], 256).await;
        let actor = harness.spawn_read(REQUEST_ID);
        let identity = opened_identity(&harness, REQUEST_ID).await;
        assert_eq!(
            route_body(
                &harness,
                identity,
                StreamRecordBody::Credit {
                    accepted_through: 0,
                    send_through: u64::MAX,
                },
            ),
            BinaryRoute::Delivered
        );
        let abort =
            decode_stream_record(&binary(harness.sink.message(1).await), &bounds(256)).unwrap();
        assert_eq!(
            abort.body,
            StreamRecordBody::Abort {
                accepted_through: 0,
                reason: BinaryAbortReason::ProtocolError,
            }
        );
        route_body(
            &harness,
            identity,
            StreamRecordBody::Ack {
                accepted_through: 0,
            },
        );
        assert_eq!(
            text_reply(harness.sink.message(2).await).err,
            Some(ApiError::MalformedFrame)
        );
        assert!(actor.await.unwrap());
        harness.shutdown().await;
    }

    #[tokio::test]
    async fn invalid_credit_during_daemon_abort_ack_wait_is_not_ignored() {
        let harness = RuntimeHarness::new(&[1], 256).await;
        let actor = harness.spawn_read(REQUEST_ID);
        let identity = opened_identity(&harness, REQUEST_ID).await;

        // Wrong-direction DATA chooses the daemon's correlated protocol
        // ABORT. CREDIT remains structurally routable while its ACK is in
        // flight, but its cumulative values are still semantically checked.
        assert_eq!(
            harness.registry.route_binary(
                &wire(0x02, REQUEST_ID, identity.stream_id().get(), 0, 0, &[0xaa]),
                &bounds(256),
            ),
            BinaryRoute::Delivered
        );
        let abort = decode_stream_record(&binary(harness.sink.message(1).await), &bounds(256))
            .expect("daemon protocol ABORT");
        assert!(matches!(abort.body, StreamRecordBody::Abort { .. }));

        assert_eq!(
            route_body(
                &harness,
                identity,
                StreamRecordBody::Credit {
                    accepted_through: 0,
                    send_through: 2,
                },
            ),
            BinaryRoute::Delivered
        );
        assert_eq!(
            route_body(
                &harness,
                identity,
                StreamRecordBody::Ack {
                    accepted_through: 0,
                },
            ),
            BinaryRoute::Delivered
        );
        assert_eq!(
            text_reply(harness.sink.message(2).await).err,
            Some(ApiError::MalformedFrame)
        );
        let Message::Close(Some(close)) = harness.sink.message(3).await else {
            panic!("invalid terminal-handshake CREDIT must close after the chosen reply");
        };
        assert_eq!(u16::from(close.code), 4007);
        assert!(actor.await.unwrap());
        assert!(*harness.close_rx.borrow());
        harness.shutdown().await;
    }

    #[tokio::test]
    async fn collapsed_crossed_abort_is_acked_before_malformed_handshake_closes() {
        let harness = RuntimeHarness::new(&[1], 256).await;
        let actor = harness.spawn_read(REQUEST_ID);
        let identity = opened_identity(&harness, REQUEST_ID).await;
        assert_eq!(
            harness.registry.route_binary(
                &wire(0x02, REQUEST_ID, identity.stream_id().get(), 0, 0, &[0xaa]),
                &bounds(256),
            ),
            BinaryRoute::Delivered
        );
        let daemon_abort =
            decode_stream_record(&binary(harness.sink.message(1).await), &bounds(256)).unwrap();
        assert!(matches!(daemon_abort.body, StreamRecordBody::Abort { .. }));

        // Prove the actor has passed its one-time pre-loop reconciliation by
        // making it consume a valid handshake CREDIT first.
        let ingress = harness
            .registry
            .inner
            .lock()
            .expect("stream registry poisoned")
            .bindings
            .get(&identity)
            .cloned()
            .expect("daemon-ABORT pair remains bound")
            .download();
        assert_eq!(
            route_body(
                &harness,
                identity,
                StreamRecordBody::Credit {
                    accepted_through: 0,
                    send_through: 0,
                },
            ),
            BinaryRoute::Delivered
        );
        while ingress.mailbox.has_pending() {
            tokio::task::yield_now().await;
        }

        // These synchronous routes cannot be interleaved with the actor. The
        // duplicate collapses the first crossed ABORT into a Malformed event;
        // the retained first terminal must still receive its ACK.
        let crossed = StreamRecordBody::Abort {
            accepted_through: 0,
            reason: BinaryAbortReason::Cancelled,
        };
        assert_eq!(
            route_body(&harness, identity, crossed.clone()),
            BinaryRoute::Delivered
        );
        assert_eq!(
            route_body(&harness, identity, crossed),
            BinaryRoute::Delivered
        );
        assert_eq!(
            route_body(
                &harness,
                identity,
                StreamRecordBody::Ack {
                    accepted_through: 0,
                },
            ),
            BinaryRoute::Delivered
        );

        let crossed_ack =
            decode_stream_record(&binary(harness.sink.message(2).await), &bounds(256)).unwrap();
        assert_eq!(
            crossed_ack.body,
            StreamRecordBody::Ack {
                accepted_through: 0
            }
        );
        assert_eq!(
            text_reply(harness.sink.message(3).await).err,
            Some(ApiError::MalformedFrame)
        );
        let Message::Close(Some(close)) = harness.sink.message(4).await else {
            panic!("malformed terminal batch must close after its correlated reply");
        };
        assert_eq!(u16::from(close.code), 4007);
        assert!(actor.await.unwrap());
        assert!(*harness.close_rx.borrow());
        harness.shutdown().await;
    }

    #[tokio::test]
    async fn crossed_abort_completes_both_ack_obligations_with_daemon_result_authoritative() {
        let harness = RuntimeHarness::new(&[1], 256).await;
        let actor = harness.spawn_read(REQUEST_ID);
        let identity = opened_identity(&harness, REQUEST_ID).await;
        harness.registry.route_binary(
            &wire(0x02, REQUEST_ID, identity.stream_id().get(), 0, 0, &[0xaa]),
            &bounds(256),
        );
        let daemon_abort =
            decode_stream_record(&binary(harness.sink.message(1).await), &bounds(256)).unwrap();
        assert!(matches!(daemon_abort.body, StreamRecordBody::Abort { .. }));

        route_body(
            &harness,
            identity,
            StreamRecordBody::Abort {
                accepted_through: 0,
                reason: BinaryAbortReason::Cancelled,
            },
        );
        route_body(
            &harness,
            identity,
            StreamRecordBody::Ack {
                accepted_through: 0,
            },
        );
        let crossed_ack =
            decode_stream_record(&binary(harness.sink.message(2).await), &bounds(256)).unwrap();
        assert_eq!(
            crossed_ack.body,
            StreamRecordBody::Ack {
                accepted_through: 0
            }
        );
        let reply = text_reply(harness.sink.message(3).await);
        assert_eq!(reply.err, Some(ApiError::MalformedFrame));
        assert!(actor.await.unwrap());
        harness.shutdown().await;
    }

    #[tokio::test]
    async fn truncation_and_growth_become_source_failed_abort_before_any_success() {
        for grow in [false, true] {
            let harness = RuntimeHarness::new(&[1, 2, 3], 256).await;
            let actor = harness.spawn_read(REQUEST_ID);
            let identity = opened_identity(&harness, REQUEST_ID).await;
            if !grow {
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(&harness.source_path)
                    .unwrap()
                    .set_len(0)
                    .unwrap();
            }
            route_body(
                &harness,
                identity,
                StreamRecordBody::Credit {
                    accepted_through: 0,
                    send_through: 3,
                },
            );

            let abort_index = if grow {
                let data =
                    decode_stream_record(&binary(harness.sink.message(1).await), &bounds(256))
                        .unwrap();
                assert!(matches!(data.body, StreamRecordBody::Data { .. }));
                use std::io::Write as _;
                std::fs::OpenOptions::new()
                    .append(true)
                    .open(&harness.source_path)
                    .unwrap()
                    .write_all(&[4])
                    .unwrap();
                route_body(
                    &harness,
                    identity,
                    StreamRecordBody::Credit {
                        accepted_through: 3,
                        send_through: 3,
                    },
                );
                2
            } else {
                1
            };
            let abort = decode_stream_record(
                &binary(harness.sink.message(abort_index).await),
                &bounds(256),
            )
            .unwrap();
            assert_eq!(
                abort.body,
                StreamRecordBody::Abort {
                    accepted_through: if grow { 3 } else { 0 },
                    reason: BinaryAbortReason::SourceFailed,
                }
            );
            route_body(
                &harness,
                identity,
                StreamRecordBody::Ack {
                    accepted_through: if grow { 3 } else { 0 },
                },
            );
            let reply = text_reply(harness.sink.message(abort_index + 1).await);
            assert_eq!(
                reply.err,
                Some(ApiError::StreamAborted {
                    transferred_bytes: if grow { 3 } else { 0 },
                    total: known_total(3),
                    reason: StreamAbortReason::SourceFailed,
                })
            );
            assert!(actor.await.unwrap());
            harness.shutdown().await;
        }
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn source_read_error_emits_abort_then_waits_for_ack_before_reply() {
        let harness = RuntimeHarness::new(&[], 256).await;
        let state_path = harness._dir.path().join("state.json");
        let mut local_state: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&state_path).expect("read local state"))
                .expect("decode local state");
        local_state["rooms"][harness.request.room_id.as_str()]["fetched_files"]
            [harness.request.file_id.as_str()]["path"] = serde_json::json!("/proc/self/mem");
        std::fs::write(
            &state_path,
            serde_json::to_vec_pretty(&local_state).expect("encode local state"),
        )
        .expect("install an exact-size source whose EOF probe returns EIO");

        let actor = harness.spawn_read(REQUEST_ID);
        let identity = opened_identity(&harness, REQUEST_ID).await;
        route_body(
            &harness,
            identity,
            StreamRecordBody::Credit {
                accepted_through: 0,
                send_through: 0,
            },
        );
        let abort =
            decode_stream_record(&binary(harness.sink.message(1).await), &bounds(256)).unwrap();
        assert_eq!(
            abort.body,
            StreamRecordBody::Abort {
                accepted_through: 0,
                reason: BinaryAbortReason::SourceFailed,
            }
        );
        route_body(
            &harness,
            identity,
            StreamRecordBody::Ack {
                accepted_through: 0,
            },
        );
        assert_eq!(
            text_reply(harness.sink.message(2).await).err,
            Some(ApiError::StreamAborted {
                transferred_bytes: 0,
                total: known_total(0),
                reason: StreamAbortReason::SourceFailed,
            })
        );
        assert!(actor.await.unwrap());
        harness.shutdown().await;
    }

    #[tokio::test]
    async fn stall_ignores_repeated_credit_then_abort_ack_timeout_replies_and_closes() {
        let harness = RuntimeHarness::new_with_timers(&[1], 256, 10, 1_000, 8_192).await;
        tokio::time::pause();
        let actor = harness.spawn_read(REQUEST_ID);
        let identity = opened_identity(&harness, REQUEST_ID).await;
        for _ in 0..4 {
            assert_eq!(
                route_body(
                    &harness,
                    identity,
                    StreamRecordBody::Credit {
                        accepted_through: 0,
                        send_through: 0,
                    },
                ),
                BinaryRoute::Delivered
            );
        }

        tokio::time::advance(Duration::from_millis(10)).await;
        tokio::task::yield_now().await;
        let abort =
            decode_stream_record(&binary(harness.sink.message(1).await), &bounds(256)).unwrap();
        assert_eq!(
            abort.body,
            StreamRecordBody::Abort {
                accepted_through: 0,
                reason: BinaryAbortReason::OperationError,
            }
        );

        tokio::time::advance(Duration::from_millis(10)).await;
        tokio::task::yield_now().await;
        let reply = text_reply(harness.sink.message(2).await);
        assert_eq!(
            reply.err,
            Some(ApiError::TransferStalled {
                transferred_bytes: 0,
                total: known_total(1),
            })
        );
        let Message::Close(Some(close)) = harness.sink.message(3).await else {
            panic!("expected close after ACK timeout");
        };
        assert_eq!(u16::from(close.code), 4007);
        assert!(actor.await.unwrap());
        assert!(*harness.close_rx.borrow());
        harness.shutdown().await;
    }

    #[tokio::test]
    async fn sent_unacknowledged_data_and_pong_do_not_reset_stall() {
        let harness = RuntimeHarness::new_with_timers(&[1], 256, 10, 1_000, 8_192).await;
        tokio::time::pause();
        let actor = harness.spawn_read(REQUEST_ID);
        let identity = opened_identity(&harness, REQUEST_ID).await;
        route_body(
            &harness,
            identity,
            StreamRecordBody::Credit {
                accepted_through: 0,
                send_through: 1,
            },
        );
        let data = decode_stream_record(&binary(harness.sink.message(1).await), &bounds(256))
            .expect("one credited DATA record");
        assert!(matches!(data.body, StreamRecordBody::Data { .. }));

        assert!(
            harness
                .outbound
                .pong(tokio_tungstenite::tungstenite::Bytes::from_static(
                    b"still-alive"
                ))
                .await
        );
        assert!(matches!(harness.sink.message(2).await, Message::Pong(_)));

        tokio::time::advance(Duration::from_millis(10)).await;
        tokio::task::yield_now().await;
        let abort = decode_stream_record(&binary(harness.sink.message(3).await), &bounds(256))
            .expect("stall ABORT");
        assert_eq!(
            abort.body,
            StreamRecordBody::Abort {
                accepted_through: 0,
                reason: BinaryAbortReason::OperationError,
            }
        );
        route_body(
            &harness,
            identity,
            StreamRecordBody::Ack {
                accepted_through: 0,
            },
        );
        assert_eq!(
            text_reply(harness.sink.message(4).await).err,
            Some(ApiError::TransferStalled {
                transferred_bytes: 0,
                total: known_total(1),
            })
        );
        assert!(actor.await.unwrap());
        harness.shutdown().await;
    }

    #[tokio::test]
    async fn absolute_deadline_aborts_even_while_correctly_credit_paused() {
        let harness = RuntimeHarness::new_with_timers(&[1], 256, 100, 0, u64::MAX).await;
        tokio::time::pause();
        let actor = harness.spawn_read(REQUEST_ID);
        let identity = opened_identity(&harness, REQUEST_ID).await;
        // Initial zero-window CREDIT is correct receiver backpressure. It does
        // not stop the size-aware absolute budget.
        route_body(
            &harness,
            identity,
            StreamRecordBody::Credit {
                accepted_through: 0,
                send_through: 0,
            },
        );
        tokio::time::advance(Duration::from_millis(1)).await;
        tokio::task::yield_now().await;
        let abort =
            decode_stream_record(&binary(harness.sink.message(1).await), &bounds(256)).unwrap();
        assert_eq!(
            abort.body,
            StreamRecordBody::Abort {
                accepted_through: 0,
                reason: BinaryAbortReason::OperationError,
            }
        );
        route_body(
            &harness,
            identity,
            StreamRecordBody::Ack {
                accepted_through: 0,
            },
        );
        let reply = text_reply(harness.sink.message(2).await);
        assert_eq!(
            reply.err,
            Some(ApiError::TransferDeadlineExceeded {
                transferred_bytes: 0,
                total: known_total(1),
                budget_ms: 1,
            })
        );
        assert!(actor.await.unwrap());
        harness.shutdown().await;
    }

    #[tokio::test]
    async fn accepted_progress_resets_stall_but_cannot_extend_absolute_deadline() {
        let harness = RuntimeHarness::new_with_timers(&[1, 2, 3], 256, 10, 0, 1_200).await;
        tokio::time::pause();
        let actor = harness.spawn_read(REQUEST_ID);
        let identity = opened_identity(&harness, REQUEST_ID).await;
        route_body(
            &harness,
            identity,
            StreamRecordBody::Credit {
                accepted_through: 0,
                send_through: 1,
            },
        );
        assert!(matches!(
            decode_stream_record(&binary(harness.sink.message(1).await), &bounds(256))
                .unwrap()
                .body,
            StreamRecordBody::Data { offset: 0, .. }
        ));

        tokio::time::advance(Duration::from_millis(9)).await;
        route_body(
            &harness,
            identity,
            StreamRecordBody::Credit {
                accepted_through: 1,
                send_through: 2,
            },
        );
        assert!(matches!(
            decode_stream_record(&binary(harness.sink.message(2).await), &bounds(256))
                .unwrap()
                .body,
            StreamRecordBody::Data { offset: 1, .. }
        ));
        tokio::time::advance(Duration::from_millis(9)).await;
        route_body(
            &harness,
            identity,
            StreamRecordBody::Credit {
                accepted_through: 2,
                send_through: 3,
            },
        );
        assert!(matches!(
            decode_stream_record(&binary(harness.sink.message(3).await), &bounds(256))
                .unwrap()
                .body,
            StreamRecordBody::Data { offset: 2, .. }
        ));

        tokio::time::advance(Duration::from_millis(2)).await;
        tokio::task::yield_now().await;
        let abort =
            decode_stream_record(&binary(harness.sink.message(4).await), &bounds(256)).unwrap();
        assert_eq!(
            abort.body,
            StreamRecordBody::Abort {
                accepted_through: 2,
                reason: BinaryAbortReason::OperationError,
            }
        );
        route_body(
            &harness,
            identity,
            StreamRecordBody::Ack {
                accepted_through: 2,
            },
        );
        let reply = text_reply(harness.sink.message(5).await);
        assert_eq!(
            reply.err,
            Some(ApiError::TransferDeadlineExceeded {
                transferred_bytes: 2,
                total: known_total(3),
                budget_ms: 20,
            })
        );
        assert!(actor.await.unwrap());
        harness.shutdown().await;
    }

    #[tokio::test]
    async fn client_abort_wins_an_exact_stall_timer_tie() {
        let harness = RuntimeHarness::new_with_timers(&[1], 256, 10, 1_000, 8_192).await;
        tokio::time::pause();
        let actor = harness.spawn_read(REQUEST_ID);
        let identity = opened_identity(&harness, REQUEST_ID).await;
        tokio::time::advance(Duration::from_millis(10)).await;
        route_body(
            &harness,
            identity,
            StreamRecordBody::Abort {
                accepted_through: 0,
                reason: BinaryAbortReason::Cancelled,
            },
        );
        tokio::task::yield_now().await;
        let ack =
            decode_stream_record(&binary(harness.sink.message(1).await), &bounds(256)).unwrap();
        assert_eq!(
            ack.body,
            StreamRecordBody::Ack {
                accepted_through: 0,
            }
        );
        let reply = text_reply(harness.sink.message(2).await);
        assert!(matches!(
            reply.err,
            Some(ApiError::StreamAborted {
                reason: StreamAbortReason::Cancelled,
                ..
            })
        ));
        assert!(actor.await.unwrap());
        harness.shutdown().await;
    }

    #[tokio::test]
    async fn disconnect_task_abort_drops_binding_source_request_and_reservations() {
        let harness = RuntimeHarness::new(&[1, 2, 3], 256).await;
        let actor = harness.spawn_read(REQUEST_ID);
        let _identity = opened_identity(&harness, REQUEST_ID).await;
        assert!(harness.registry.is_active());
        assert_eq!(harness.pool.usage(), (1, 3));
        assert!(harness.tracker.is_outstanding(REQUEST_ID));

        actor.abort();
        assert!(actor.await.unwrap_err().is_cancelled());
        tokio::task::yield_now().await;
        assert!(!harness.registry.is_active());
        assert_eq!(harness.pool.usage(), (0, 0));
        assert!(!harness.tracker.is_outstanding(REQUEST_ID));
        assert_eq!(
            harness.outbound.available_data_bytes(),
            harness.limits.data_queue_capacity_bytes()
        );
        harness.shutdown().await;
    }

    #[tokio::test]
    async fn ordinary_reply_and_push_interleave_while_download_waits_for_credit() {
        let harness = RuntimeHarness::new(&[1, 2, 3], 256).await;
        let actor = harness.spawn_read(REQUEST_ID);
        let identity = opened_identity(&harness, REQUEST_ID).await;

        let ordinary = jeliya_codec::Reply {
            id: 99,
            ok: true,
            out: Some(serde_json::json!({"rooms": []})),
            err: None,
        };
        assert_eq!(
            harness.outbound.text(ordinary.to_bytes()).await,
            WriteReceipt::Sent
        );
        let push = jeliya_api::Push::Transfer {
            transfer_op_id: jeliya_api::OpId::new("other-transfer"),
            transferred_bytes: 7,
            total: known_total(9),
        };
        assert_eq!(
            harness
                .outbound
                .text(jeliya_codec::push_to_bytes(&push))
                .await,
            WriteReceipt::Sent
        );
        assert!(matches!(harness.sink.message(1).await, Message::Text(_)));
        assert!(matches!(harness.sink.message(2).await, Message::Text(_)));
        assert!(harness.registry.is_active());

        route_body(
            &harness,
            identity,
            StreamRecordBody::Abort {
                accepted_through: 0,
                reason: BinaryAbortReason::Cancelled,
            },
        );
        let ack =
            decode_stream_record(&binary(harness.sink.message(3).await), &bounds(256)).unwrap();
        assert!(matches!(ack.body, StreamRecordBody::Ack { .. }));
        let aborted = text_reply(harness.sink.message(4).await);
        assert!(matches!(aborted.err, Some(ApiError::StreamAborted { .. })));
        assert!(actor.await.unwrap());
        harness.shutdown().await;
    }

    #[test]
    fn request_ids_remain_outstanding_until_raii_terminal_release() {
        let tracker = RequestTracker::new(2);
        let first = tracker.acquire(1).expect("first request");
        assert!(tracker.is_outstanding(1));
        assert_eq!(
            tracker.acquire(1).err(),
            Some(RequestAdmissionError::Duplicate)
        );
        let second = tracker.acquire(2).expect("second request");
        assert_eq!(
            tracker.acquire(3).err(),
            Some(RequestAdmissionError::Exhausted(
                ApiError::ResourceExhausted {
                    resource: "max_inflight_requests".into(),
                    limit: 2,
                }
            ))
        );
        drop(first);
        assert!(!tracker.is_outstanding(1));
        assert!(tracker.acquire(1).is_ok());
        drop(second);
    }

    #[test]
    fn credit_is_cumulative_monotonic_bounded_and_boundary_exact() {
        let mut state = ProducerState::new(10);
        assert_eq!(state.next_payload(4), None, "no DATA before initial credit");
        assert_eq!(
            state.credit(0, 0),
            Ok(CreditEffect::Advanced { accepted: false })
        );
        assert_eq!(state.credit(0, 0), Ok(CreditEffect::Repeated));
        assert_eq!(
            state.credit(0, 4),
            Ok(CreditEffect::Advanced { accepted: false })
        );
        assert_eq!(state.next_payload(64), Some(4));
        state.data_sent(4).expect("first DATA boundary");

        assert_eq!(
            state.credit(4, 8),
            Ok(CreditEffect::Advanced { accepted: true })
        );
        state.data_sent(4).expect("second DATA boundary");
        for invalid in [(3, 8), (4, 7), (9, 9), (8, 11)] {
            assert_eq!(state.credit(invalid.0, invalid.1), Err(()), "{invalid:?}");
        }
        // Atomic DATA acceptance forbids acknowledging the middle of a record.
        assert_eq!(state.credit(6, 8), Err(()));
    }

    #[test]
    fn checked_data_and_credit_edges_never_wrap_or_cross_credit() {
        let mut state = ProducerState::new(u64::MAX);
        state.credit_seen = true;
        state.send_through = u64::MAX;
        state.sent = u64::MAX;
        assert_eq!(state.data_sent(1), Err(()));

        let mut bounded = ProducerState::new(5);
        bounded.credit(0, 1).unwrap();
        assert_eq!(bounded.data_sent(2), Err(()), "DATA may not exceed credit");
        bounded.data_sent(1).unwrap();
        assert_eq!(bounded.next_payload(65_536), None);
        assert_eq!(bounded.credit(2, 2), Err(()), "cannot ACK unsent bytes");
    }

    #[test]
    fn zero_one_multi_and_maximum_totals_segment_without_whole_file_state() {
        let zero = ProducerState::new(0);
        assert!(!zero.ready_for_end(), "zero still requires initial CREDIT");

        let mut one = ProducerState::new(1);
        one.credit(0, 1).unwrap();
        assert_eq!(one.next_payload(65_536), Some(1));
        one.data_sent(1).unwrap();
        one.credit(1, 1).unwrap();
        assert!(one.ready_for_end());

        let total = 104_857_600_u64;
        let mut maximum = ProducerState::new(total);
        maximum.credit(0, total).unwrap();
        let mut records = 0_usize;
        while let Some(bytes) = maximum.next_payload(65_536) {
            assert!((1..=65_536).contains(&bytes));
            maximum.data_sent(bytes).unwrap();
            records += 1;
            maximum.credit(maximum.sent, total).unwrap();
        }
        assert_eq!(maximum.sent, total);
        assert_eq!(records, 1_600);
        assert!(maximum.ready_for_end());

        // Adversarial one-byte credit cannot accumulate one boundary per byte:
        // the producer keeps exactly one atomic DATA record outstanding.
        let mut tiny_windows = ProducerState::new(100_000);
        for end in 1..=100_000 {
            tiny_windows.credit(tiny_windows.accepted, end).unwrap();
            assert_eq!(tiny_windows.next_payload(65_536), Some(1));
            tiny_windows.data_sent(1).unwrap();
            assert_eq!(tiny_windows.next_payload(65_536), None);
            tiny_windows.credit(end, end).unwrap();
        }
        assert!(tiny_windows.ready_for_end());
    }

    #[test]
    fn binary_routing_checks_size_then_full_pair_then_direction_and_structure() {
        let registry = StreamRegistry::new();
        let identity = StreamIdentity::new(REQUEST_ID, STREAM_ID).unwrap();
        let binding = registry.bind(identity).expect("fresh pair");
        binding.set_phase(BindingPhase::Active);
        let limits = bounds(64);

        let oversized_bad_magic = vec![0; 65];
        assert_eq!(
            registry.route_binary(&oversized_bad_magic, &limits),
            BinaryRoute::CloseTooLarge
        );
        assert_eq!(
            registry.route_binary(b"JBS2", &limits),
            BinaryRoute::CloseMalformed
        );
        let mut bad_magic = wire(0x03, REQUEST_ID, STREAM_ID, 0, 1, &[]);
        bad_magic[0] = b'X';
        assert_eq!(
            registry.route_binary(&bad_magic, &limits),
            BinaryRoute::CloseMalformed
        );
        assert_eq!(
            registry.route_binary(&wire(0x03, REQUEST_ID + 1, STREAM_ID, 0, 1, &[]), &limits,),
            BinaryRoute::CloseMalformed,
            "wrong request id cannot half-bind"
        );
        assert_eq!(
            registry.route_binary(&wire(0x03, REQUEST_ID, STREAM_ID + 1, 0, 1, &[]), &limits,),
            BinaryRoute::CloseMalformed,
            "wrong stream id cannot half-bind"
        );

        assert_eq!(
            registry.route_binary(&wire(0x03, REQUEST_ID, STREAM_ID, 0, 1, &[]), &limits,),
            BinaryRoute::Delivered
        );
        assert!(matches!(
            binding.ingress.mailbox.try_recv(),
            Some(MailboxEvent {
                body: MailboxEventBody::Record(StreamRecordBody::Credit {
                    accepted_through: 0,
                    send_through: 1
                }),
                ..
            })
        ));

        // DATA is structurally plausible but the client is the receiver. Kind
        // and direction reject it before the full decoder can allocate payload.
        assert_eq!(
            registry.route_binary(&wire(0x02, REQUEST_ID, STREAM_ID, 0, 0, &[0xaa]), &limits,),
            BinaryRoute::Delivered
        );
        assert!(matches!(
            binding.ingress.mailbox.try_recv(),
            Some(MailboxEvent {
                body: MailboxEventBody::Malformed,
                ..
            })
        ));
    }

    #[test]
    fn records_before_writer_commits_open_have_no_trustworthy_binding() {
        let registry = StreamRegistry::new();
        let identity = StreamIdentity::new(REQUEST_ID, STREAM_ID).unwrap();
        let binding = registry.bind(identity).expect("opening pair");
        assert_eq!(
            registry.route_binary(&wire(0x03, REQUEST_ID, STREAM_ID, 0, 0, &[]), &bounds(64),),
            BinaryRoute::CloseMalformed
        );
        assert!(binding.ingress.mailbox.try_recv().is_none());
    }

    #[test]
    fn malformed_bound_record_is_local_but_unbound_malformed_is_connection_fatal() {
        let registry = StreamRegistry::new();
        let one = StreamIdentity::new(1, 11).unwrap();
        let two = StreamIdentity::new(2, 22).unwrap();
        let one_binding = registry.bind(one).unwrap();
        let two_binding = registry.bind(two).unwrap();
        one_binding.set_phase(BindingPhase::Active);
        two_binding.set_phase(BindingPhase::Active);
        let limits = bounds(64);

        let mut bound_bad_reserved = wire(0x03, 1, 11, 0, 1, &[]);
        bound_bad_reserved[5] = 1;
        assert_eq!(
            registry.route_binary(&bound_bad_reserved, &limits),
            BinaryRoute::Delivered
        );
        assert!(matches!(
            one_binding.ingress.mailbox.try_recv(),
            Some(MailboxEvent {
                body: MailboxEventBody::Malformed,
                ..
            })
        ));

        // The unrelated exact pair remains fully usable.
        assert_eq!(
            registry.route_binary(&wire(0x03, 2, 22, 0, 1, &[]), &limits),
            BinaryRoute::Delivered
        );
        assert!(matches!(
            two_binding.ingress.mailbox.try_recv(),
            Some(MailboxEvent {
                body: MailboxEventBody::Record(StreamRecordBody::Credit { .. }),
                ..
            })
        ));

        let mut unbound_bad_reserved = wire(0x03, 3, 33, 0, 1, &[]);
        unbound_bad_reserved[5] = 1;
        assert_eq!(
            registry.route_binary(&unbound_bad_reserved, &limits),
            BinaryRoute::CloseMalformed
        );
    }

    #[test]
    fn duplicate_terminal_is_a_bound_stream_protocol_fault() {
        let registry = StreamRegistry::new();
        let identity = StreamIdentity::new(REQUEST_ID, STREAM_ID).unwrap();
        let binding = registry.bind(identity).unwrap();
        binding.set_phase(BindingPhase::Active);
        let abort = wire(0x05, REQUEST_ID, STREAM_ID, 0, 1, &[]);

        assert_eq!(
            registry.route_binary(&abort, &bounds(64)),
            BinaryRoute::Delivered
        );
        assert_eq!(
            registry.route_binary(&abort, &bounds(64)),
            BinaryRoute::Delivered
        );
        assert!(matches!(
            binding.ingress.mailbox.try_recv(),
            Some(MailboxEvent {
                body: MailboxEventBody::Malformed,
                ..
            })
        ));
        assert!(binding.ingress.mailbox.try_recv().is_none());
    }

    #[test]
    fn coalesced_credit_keeps_wire_order_ahead_of_later_abort() {
        let registry = StreamRegistry::new();
        let identity = StreamIdentity::new(REQUEST_ID, STREAM_ID).unwrap();
        let binding = registry.bind(identity).unwrap();
        binding.set_phase(BindingPhase::Active);
        assert_eq!(
            registry.route_binary(&wire(0x03, REQUEST_ID, STREAM_ID, 0, 1, &[]), &bounds(64),),
            BinaryRoute::Delivered
        );
        assert_eq!(
            registry.route_binary(&wire(0x05, REQUEST_ID, STREAM_ID, 0, 1, &[]), &bounds(64),),
            BinaryRoute::Delivered
        );
        assert!(matches!(
            binding.ingress.mailbox.try_recv(),
            Some(MailboxEvent {
                body: MailboxEventBody::Record(StreamRecordBody::Credit { .. }),
                ..
            })
        ));
        assert!(matches!(
            binding.ingress.mailbox.try_recv(),
            Some(MailboxEvent {
                body: MailboxEventBody::Record(StreamRecordBody::Abort { .. }),
                ..
            })
        ));
    }

    #[tokio::test]
    async fn credit_coalescing_preserves_progress_time_and_invalid_intermediate_values() {
        tokio::time::pause();
        let mailbox = InboundMailbox::new();
        mailbox.push(
            StreamRecordBody::Credit {
                accepted_through: 0,
                send_through: 0,
            },
            false,
        );
        let progress_at = Instant::now();
        mailbox.push(
            StreamRecordBody::Credit {
                accepted_through: 4,
                send_through: 4,
            },
            false,
        );
        tokio::time::advance(Duration::from_millis(9)).await;
        mailbox.push(
            StreamRecordBody::Credit {
                accepted_through: 4,
                send_through: 8,
            },
            false,
        );

        assert!(matches!(
            mailbox.try_recv(),
            Some(MailboxEvent {
                body: MailboxEventBody::Record(StreamRecordBody::Credit {
                    accepted_through: 0,
                    send_through: 0,
                }),
                ..
            })
        ));
        let progress = mailbox
            .try_recv()
            .expect("accepted progress remains visible");
        assert_eq!(progress.received_at, progress_at);
        assert!(matches!(
            progress.body,
            MailboxEventBody::Record(StreamRecordBody::Credit {
                accepted_through: 4,
                send_through: 8,
            })
        ));

        let mailbox = InboundMailbox::new();
        mailbox.push(
            StreamRecordBody::Credit {
                accepted_through: 2,
                send_through: 4,
            },
            false,
        );
        mailbox.push(
            StreamRecordBody::Credit {
                accepted_through: 4,
                send_through: 4,
            },
            false,
        );
        let mut producer = ProducerState::new(4);
        producer.credit(0, 4).unwrap();
        producer.data_sent(4).unwrap();
        let partial = mailbox.try_recv().expect("partial ACK is retained");
        let MailboxEventBody::Record(StreamRecordBody::Credit {
            accepted_through,
            send_through,
        }) = partial.body
        else {
            panic!("expected retained CREDIT");
        };
        assert_eq!(producer.credit(accepted_through, send_through), Err(()));
    }

    #[test]
    fn terminal_fault_preserves_earlier_credit_order() {
        let mailbox = InboundMailbox::new();
        mailbox.push(
            StreamRecordBody::Credit {
                accepted_through: 0,
                send_through: 1,
            },
            false,
        );
        mailbox.push(
            StreamRecordBody::Abort {
                accepted_through: 0,
                reason: BinaryAbortReason::Cancelled,
            },
            false,
        );
        mailbox.push(
            StreamRecordBody::Abort {
                accepted_through: 0,
                reason: BinaryAbortReason::Cancelled,
            },
            false,
        );
        assert!(matches!(
            mailbox.try_recv(),
            Some(MailboxEvent {
                body: MailboxEventBody::Record(StreamRecordBody::Credit { .. }),
                ..
            })
        ));
        assert!(matches!(
            mailbox.try_recv(),
            Some(MailboxEvent {
                body: MailboxEventBody::Malformed,
                ..
            })
        ));
        assert!(mailbox.try_recv().is_none());
    }

    #[test]
    fn duplicate_bind_does_not_replace_the_live_ingress() {
        let registry = StreamRegistry::new();
        let identity = StreamIdentity::new(REQUEST_ID, STREAM_ID).unwrap();
        let binding = registry.bind(identity).unwrap();
        binding.set_phase(BindingPhase::Active);
        assert!(registry.bind(identity).is_none());
        assert_eq!(
            registry.route_binary(&wire(0x03, REQUEST_ID, STREAM_ID, 0, 0, &[]), &bounds(64),),
            BinaryRoute::Delivered
        );
        assert!(matches!(
            binding.ingress.mailbox.try_recv(),
            Some(MailboxEvent {
                body: MailboxEventBody::Record(StreamRecordBody::Credit { .. }),
                ..
            })
        ));
    }

    #[test]
    fn stale_routing_clone_cannot_deliver_after_retirement() {
        let registry = StreamRegistry::new();
        let identity = StreamIdentity::new(REQUEST_ID, STREAM_ID).unwrap();
        let binding = registry.bind(identity).unwrap();
        binding.set_phase(BindingPhase::Active);
        let stale = binding.ingress.clone();
        binding.retire();
        assert_eq!(BindingPhase::load(&stale.phase), BindingPhase::Retired);
        assert_eq!(
            StreamRegistry::route_bound(
                &stale,
                &wire(0x03, REQUEST_ID, STREAM_ID, 0, 1, &[]),
                &bounds(64),
            ),
            BinaryRoute::CloseMalformed
        );
        assert!(stale.mailbox.try_recv().is_none());
    }

    #[tokio::test]
    async fn timely_ack_at_the_daemon_abort_deadline_wins_the_atomic_poll() {
        tokio::time::pause();
        let registry = StreamRegistry::new();
        let identity = StreamIdentity::new(REQUEST_ID, STREAM_ID).unwrap();
        let binding = registry.bind(identity).unwrap();
        binding.set_phase(BindingPhase::DaemonAbortWaitAck);
        let deadline = Instant::now() + Duration::from_millis(10);

        tokio::time::advance(Duration::from_millis(10)).await;
        assert_eq!(
            registry.route_binary(
                &wire(0x06, REQUEST_ID, STREAM_ID, 0, 0x05, &[]),
                &bounds(64),
            ),
            BinaryRoute::Delivered
        );
        let DaemonAckPoll::Event(event) = poll_daemon_ack(&binding, deadline) else {
            panic!("an ACK sequenced at the deadline must be observed before timeout");
        };
        assert!(event.received_at <= deadline);
        assert!(matches!(
            event.body,
            MailboxEventBody::Record(StreamRecordBody::Ack {
                accepted_through: 0
            })
        ));
    }

    #[test]
    fn end_finalizing_ignores_only_valid_late_controls_and_retains_no_resume_state() {
        let registry = StreamRegistry::new();
        let identity = StreamIdentity::new(REQUEST_ID, STREAM_ID).unwrap();
        let binding = registry.bind(identity).unwrap();
        binding.set_phase(BindingPhase::Finalizing);
        let abort = wire(0x05, REQUEST_ID, STREAM_ID, 0, 1, &[]);
        assert_eq!(
            registry.route_binary(&wire(0x03, REQUEST_ID, STREAM_ID, 0, 0, &[]), &bounds(64),),
            BinaryRoute::Delivered,
            "the final repeated CREDIT cannot alter committed END"
        );
        assert_eq!(
            registry.route_binary(&abort, &bounds(64)),
            BinaryRoute::Delivered
        );
        assert!(binding.ingress.mailbox.try_recv().is_none());
        assert_eq!(
            registry.route_binary(&wire(0x03, REQUEST_ID, STREAM_ID, 0, 1, &[]), &bounds(64),),
            BinaryRoute::CloseMalformed,
            "one-past late CREDIT is validated rather than silently dropped"
        );
        assert_eq!(
            registry.route_binary(
                &wire(0x06, REQUEST_ID, STREAM_ID, 0, 0x05, &[]),
                &bounds(64),
            ),
            BinaryRoute::CloseMalformed,
            "unsolicited ACK has no valid FINALIZING transition"
        );
        drop(binding);
        assert!(!registry.is_active());
        assert_eq!(
            registry.route_binary(&abort, &bounds(64)),
            BinaryRoute::CloseMalformed,
            "a retired pair is never resumable or tombstoned as active"
        );
    }
}
