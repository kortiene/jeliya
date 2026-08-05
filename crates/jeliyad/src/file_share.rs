//! Connection-local protocol-v2 `file.share` consumer runtime.
//!
//! The upload path owns no caller-visible filesystem object. One exclusive
//! daemon staging file is created only after transfer admission, written
//! sequentially through bounded DATA records, and consumed exactly once by
//! finalization. The connection registry retains the complete
//! `(request_id, stream_id)` identity and direction before any DATA payload is
//! inspected or copied.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering},
    Arc, Mutex, Weak,
};

use jeliya_api::{
    ApiError, ByteTotal, CancelOutcome, EnforcedAt, FileShare, FileShareOut, OpId,
    StreamAbortReason, TransferCancel, TransferCancelOut,
};
use jeliya_codec::{
    decode_stream_kind, decode_stream_record_view, encode_stream_record, BinaryAbortReason,
    CodecBounds, StreamIdentity, StreamRecord, StreamRecordBody, StreamRecordBodyView,
    StreamRecordKind,
};
use jeliya_core::engine::{
    Engine, FileShareFinalizer, FileShareLedgerGate, FileShareLedgerOwner, FileShareSinkError,
};
use tokio::sync::{mpsc, oneshot, Notify, OwnedSemaphorePermit, Semaphore};
use tokio::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::Bytes;

use crate::file_read::{
    BinaryRoute, ConnectionCloser, RequestPermit, StreamRegistry, UploadStreamBinding,
    UploadStreamRetirement,
};
use crate::outbound::{Outbound, WriteReceipt};
use crate::transfer::{RuntimeLimits, StreamIdGenerator, TransferPool};

#[cfg(test)]
const UPLOAD_DATA_EVENT_SLOTS: usize = 4;
const UPLOAD_TERMINAL_EVENT_SLOTS: usize = 4;
const CONTROL_ATTEMPT_QUEUED: u8 = 0;
const CONTROL_ATTEMPT_STARTED: u8 = 1;
const CONTROL_ATTEMPT_CANCELLED: u8 = 2;
const CONTROL_COMMIT_PENDING: u8 = 0;
const PRODUCER_TERMINAL_NONE: u8 = 0;
const PRODUCER_TERMINAL_END: u8 = 1;
const PRODUCER_TERMINAL_ABORT: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum ControlCommit {
    Committed = 1,
    Deadline = 2,
    Rejected = 3,
}

impl ControlCommit {
    fn load(value: &AtomicU8) -> Self {
        match value.load(Ordering::Acquire) {
            1 => Self::Committed,
            2 => Self::Deadline,
            _ => Self::Rejected,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoundedControlOutcome {
    Sent,
    Deadline,
    DiscardedOrClosed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DaemonAbortSelection {
    Selected,
    TerminalPending,
    Lost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalYield {
    None,
    Any,
    Preemptive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClientAbortAckOutcome {
    Sent,
    ProtocolFaultBeforeSent,
    ProtocolFaultAfterSent,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum UploadPhase {
    Opening = 0,
    Active = 1,
    DaemonAbortQueued = 2,
    DaemonAbortWaitAck = 3,
    ClientAbortPending = 4,
    ClientAbortAckCommitted = 5,
    ClientAbortAckSent = 6,
    Finalizing = 7,
    AckPending = 8,
    Retired = 9,
}

impl UploadPhase {
    fn load(value: &AtomicU8) -> Self {
        match value.load(Ordering::Acquire) {
            0 => Self::Opening,
            1 => Self::Active,
            2 => Self::DaemonAbortQueued,
            3 => Self::DaemonAbortWaitAck,
            4 => Self::ClientAbortPending,
            5 => Self::ClientAbortAckCommitted,
            6 => Self::ClientAbortAckSent,
            7 => Self::Finalizing,
            8 => Self::AckPending,
            _ => Self::Retired,
        }
    }
}

/// Connection-wide byte budget shared by every upload ingress on that
/// connection. Record count is independently bounded by each mailbox.
#[derive(Clone)]
pub(crate) struct UploadIngressBudget {
    bytes: Arc<Semaphore>,
    data_messages: usize,
}

impl UploadIngressBudget {
    pub(crate) fn new(limits: RuntimeLimits) -> Self {
        Self {
            // This is an inbound bound, distinct from the producer-direction
            // writer queue. It covers every one-byte record in one complete
            // legal CREDIT window across all connection-local uploads, so a
            // compliant producer cannot strand later control traffic behind
            // ordinary staging pressure.
            bytes: Arc::new(Semaphore::new(limits.upload_ingress_capacity_bytes())),
            data_messages: limits.upload_ingress_capacity_messages(),
        }
    }
}

enum UploadEventBody {
    Record {
        kind: StreamRecordKind,
        bytes: Bytes,
        _bytes: Option<OwnedSemaphorePermit>,
    },
    RejectedData(DataFailure),
    Malformed,
    Cancel {
        response: oneshot::Sender<CancelAttempt>,
    },
}

struct UploadEvent {
    received_at: Instant,
    body: UploadEventBody,
    // Installed at the DATA sequencing boundary and retained until the actor
    // has completely handled (or discarded) that record. A later END can be
    // visible in the priority lane without overtaking this older work.
    _pending_data: Option<PendingData>,
}

impl UploadEvent {
    fn is_terminal(&self) -> bool {
        !matches!(
            &self.body,
            UploadEventBody::Record {
                kind: StreamRecordKind::Data,
                ..
            }
        )
    }

    fn is_end(&self) -> bool {
        matches!(
            &self.body,
            UploadEventBody::Record {
                kind: StreamRecordKind::End,
                ..
            }
        )
    }

    fn is_cancel(&self) -> bool {
        matches!(&self.body, UploadEventBody::Cancel { .. })
    }

    fn is_preemptive(&self) -> bool {
        self.is_cancel()
            || matches!(&self.body, UploadEventBody::RejectedData(_))
            || matches!(
                &self.body,
                UploadEventBody::Record {
                    kind: StreamRecordKind::Abort,
                    ..
                }
            )
    }
}

struct PendingData {
    ingress: Arc<UploadIngress>,
    credit_at_receipt: u64,
}

impl PendingData {
    fn new(ingress: Arc<UploadIngress>, credit_at_receipt: u64) -> Self {
        ingress.pending_data.fetch_add(1, Ordering::AcqRel);
        Self {
            ingress,
            credit_at_receipt,
        }
    }
}

impl Drop for PendingData {
    fn drop(&mut self) {
        let previous = self.ingress.pending_data.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0, "upload pending DATA underflow");
        self.ingress.terminal_notify.notify_waiters();
        self.ingress.terminal_notify.notify_one();
    }
}

#[derive(Default)]
struct TerminalOrderState {
    next: u64,
    serving: u64,
    cancelled: BTreeSet<u64>,
}

/// A visible, ordered terminal admission. The marker is installed while the
/// sequencing lock is held, before bounded mailbox capacity is awaited, so a
/// complete terminal received before a timer/disconnect cannot disappear in
/// the wait for a full terminal lane.
struct TerminalAdmission {
    ingress: Arc<UploadIngress>,
    sequence: u64,
    cancel: bool,
    preemptive: bool,
    completed: bool,
}

impl TerminalAdmission {
    async fn wait_for_turn(&self) {
        loop {
            let notified = self.ingress.terminal_order_notify.notified();
            let is_turn = self
                .ingress
                .terminal_order
                .lock()
                .expect("upload terminal order poisoned")
                .serving
                == self.sequence;
            if is_turn {
                return;
            }
            notified.await;
        }
    }

    /// Complete while the upload sequencing lock is held.
    fn complete_locked(&mut self) {
        if self.completed {
            return;
        }
        self.completed = true;
        self.ingress.finish_terminal_admission_locked(
            self.sequence,
            self.cancel,
            self.preemptive,
            false,
        );
    }
}

impl Drop for TerminalAdmission {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        let ingress = self.ingress.clone();
        let _sequencing = ingress
            .sequencing
            .lock()
            .expect("upload sequencing poisoned");
        ingress.finish_terminal_admission_locked(self.sequence, self.cancel, self.preemptive, true);
        self.completed = true;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CancelAttempt {
    Cancelled { accepted: u64 },
    Unknown,
}

/// Direction-specific ingress installed in the unified stream registry.
pub(crate) struct UploadIngress {
    phase: AtomicU8,
    sequencing: Mutex<()>,
    opening_fatal: AtomicBool,
    closed: AtomicBool,
    accepted: AtomicU64,
    // The largest writer-committed send_through CREDIT. DATA snapshots this at
    // the complete-message receive boundary so a queued record cannot borrow
    // credit that was granted only afterwards.
    served_send_through: AtomicU64,
    declared: u64,
    file_limit: u64,
    received_through: AtomicU64,
    data_refusal_pending: AtomicBool,
    queued_events: AtomicU64,
    queued_terminals: AtomicU64,
    queued_cancels: AtomicU64,
    queued_preemptive: AtomicU64,
    admitting_terminals: AtomicU64,
    admitting_cancels: AtomicU64,
    admitting_preemptive: AtomicU64,
    processing_terminal: AtomicBool,
    processing_preemptive: AtomicBool,
    pending_data: AtomicU64,
    producer_terminal: AtomicU8,
    post_end_fatal: AtomicBool,
    data_sender: mpsc::Sender<UploadEvent>,
    terminal_sender: mpsc::Sender<UploadEvent>,
    budget: UploadIngressBudget,
    closed_notify: Notify,
    terminal_notify: Notify,
    terminal_order: Mutex<TerminalOrderState>,
    terminal_order_notify: Notify,
}

struct UploadEvents {
    data_receiver: mpsc::Receiver<UploadEvent>,
    terminal_receiver: mpsc::Receiver<UploadEvent>,
    deferred_end: Option<UploadEvent>,
}

impl UploadEvents {
    fn account_dequeue(ingress: &UploadIngress, event: &UploadEvent) {
        if event.is_terminal() {
            let _sequencing = ingress
                .sequencing
                .lock()
                .expect("upload sequencing poisoned");
            ingress.processing_terminal.store(true, Ordering::Release);
            ingress
                .processing_preemptive
                .store(event.is_preemptive(), Ordering::Release);
            ingress.queued_terminals.fetch_sub(1, Ordering::AcqRel);
            if event.is_preemptive() {
                ingress.queued_preemptive.fetch_sub(1, Ordering::AcqRel);
            }
        }
        if event.is_cancel() {
            ingress.queued_cancels.fetch_sub(1, Ordering::AcqRel);
        }
        ingress.queued_events.fetch_sub(1, Ordering::AcqRel);
    }

    fn has_deferred_end(&self) -> bool {
        self.deferred_end.is_some()
    }

    fn defer_end_if_needed(
        &mut self,
        ingress: &UploadIngress,
        event: UploadEvent,
    ) -> Option<UploadEvent> {
        if event.is_end() && ingress.pending_data.load(Ordering::Acquire) != 0 {
            debug_assert!(self.deferred_end.is_none());
            self.deferred_end = Some(event);
            None
        } else {
            Some(event)
        }
    }

    async fn recv(&mut self, ingress: &UploadIngress) -> Option<UploadEvent> {
        loop {
            if self.deferred_end.is_some() {
                if ingress.pending_data.load(Ordering::Acquire) == 0 {
                    return self.deferred_end.take();
                }

                let notified = ingress.terminal_notify.notified();
                if let Ok(event) = self.data_receiver.try_recv() {
                    Self::account_dequeue(ingress, &event);
                    return Some(event);
                }
                if ingress.pending_data.load(Ordering::Acquire) == 0 {
                    continue;
                }
                tokio::select! {
                    event = self.data_receiver.recv() => {
                        let event = event?;
                        Self::account_dequeue(ingress, &event);
                        return Some(event);
                    }
                    () = notified => continue,
                }
            }

            let event = tokio::select! {
                biased;
                event = self.terminal_receiver.recv() => event,
                event = self.data_receiver.recv() => event,
            }?;
            Self::account_dequeue(ingress, &event);
            if let Some(event) = self.defer_end_if_needed(ingress, event) {
                return Some(event);
            }
        }
    }

    fn try_recv(&mut self, ingress: &UploadIngress) -> Option<UploadEvent> {
        loop {
            if self.deferred_end.is_some() {
                if ingress.pending_data.load(Ordering::Acquire) == 0 {
                    return self.deferred_end.take();
                }
                let event = self.data_receiver.try_recv().ok()?;
                Self::account_dequeue(ingress, &event);
                return Some(event);
            }

            let event = self
                .terminal_receiver
                .try_recv()
                .or_else(|_| self.data_receiver.try_recv())
                .ok()?;
            Self::account_dequeue(ingress, &event);
            if let Some(event) = self.defer_end_if_needed(ingress, event) {
                return Some(event);
            }
        }
    }

    /// Discard DATA that was already admitted before a producer terminal.
    /// Terminal/control records use a distinct lane and remain available for
    /// crossed-terminal and duplicate-terminal validation.
    fn drain_data(&mut self, ingress: &UploadIngress) {
        while let Ok(event) = self.data_receiver.try_recv() {
            Self::account_dequeue(ingress, &event);
        }
    }
}

impl UploadIngress {
    fn new(
        budget: UploadIngressBudget,
        declared: u64,
        file_limit: u64,
    ) -> (Arc<Self>, UploadEvents) {
        let (data_sender, data_receiver) = mpsc::channel(budget.data_messages.max(1));
        let (terminal_sender, terminal_receiver) = mpsc::channel(UPLOAD_TERMINAL_EVENT_SLOTS);
        (
            Arc::new(Self {
                phase: AtomicU8::new(UploadPhase::Opening as u8),
                sequencing: Mutex::new(()),
                opening_fatal: AtomicBool::new(false),
                closed: AtomicBool::new(false),
                accepted: AtomicU64::new(0),
                served_send_through: AtomicU64::new(0),
                declared,
                file_limit,
                received_through: AtomicU64::new(0),
                data_refusal_pending: AtomicBool::new(false),
                queued_events: AtomicU64::new(0),
                queued_terminals: AtomicU64::new(0),
                queued_cancels: AtomicU64::new(0),
                queued_preemptive: AtomicU64::new(0),
                admitting_terminals: AtomicU64::new(0),
                admitting_cancels: AtomicU64::new(0),
                admitting_preemptive: AtomicU64::new(0),
                processing_terminal: AtomicBool::new(false),
                processing_preemptive: AtomicBool::new(false),
                pending_data: AtomicU64::new(0),
                producer_terminal: AtomicU8::new(PRODUCER_TERMINAL_NONE),
                post_end_fatal: AtomicBool::new(false),
                data_sender,
                terminal_sender,
                budget,
                closed_notify: Notify::new(),
                terminal_notify: Notify::new(),
                terminal_order: Mutex::new(TerminalOrderState::default()),
                terminal_order_notify: Notify::new(),
            }),
            UploadEvents {
                data_receiver,
                terminal_receiver,
                deferred_end: None,
            },
        )
    }

    fn phase(&self) -> UploadPhase {
        UploadPhase::load(&self.phase)
    }

    fn set_phase_locked(&self, phase: UploadPhase) {
        self.phase.store(phase as u8, Ordering::Release);
    }

    /// Apply every DATA check that can be decided without touching staging.
    /// The caller holds `sequencing`, making the receive frontier and CREDIT
    /// snapshot one ordered admission decision.
    fn admit_received_data_locked(
        &self,
        offset: u64,
        payload_len: usize,
        credit_at_receipt: u64,
    ) -> Result<u64, DataFailure> {
        let payload = u64::try_from(payload_len).map_err(|_| DataFailure::Protocol)?;
        let candidate = offset.checked_add(payload).ok_or(DataFailure::Protocol)?;
        if candidate > self.file_limit {
            return Err(DataFailure::TooLarge {
                observed: candidate,
            });
        }
        if candidate > self.declared {
            return Err(DataFailure::Declared {
                observed: candidate,
            });
        }
        if offset != self.received_through.load(Ordering::Acquire) {
            return Err(DataFailure::Protocol);
        }
        if candidate > credit_at_receipt {
            return Err(DataFailure::Protocol);
        }
        self.received_through.store(candidate, Ordering::Release);
        Ok(candidate)
    }

    /// Admit one terminal at the complete-message sequencing boundary.
    ///
    /// The ticket is visible before bounded terminal-lane capacity is
    /// awaited. This prevents deadline, disconnect, sink acceptance, or an
    /// active CREDIT commit from overtaking a terminal that the connection
    /// has already received and structurally classified. Tickets serialize
    /// the eventual mailbox sends in the same order as this lock.
    fn begin_terminal_locked(
        self: &Arc<Self>,
        cancel: bool,
        preemptive: bool,
    ) -> TerminalAdmission {
        let sequence = {
            let mut order = self
                .terminal_order
                .lock()
                .expect("upload terminal order poisoned");
            // Reuse the sequence space only at a quiescent boundary. In
            // practice exhausting u64 is unreachable, but the transition is
            // checked rather than allowed to wrap into a live ticket.
            if order.next == u64::MAX
                && self.admitting_terminals.load(Ordering::Acquire) == 0
                && order.serving == order.next
                && order.cancelled.is_empty()
            {
                order.next = 0;
                order.serving = 0;
            }
            let sequence = order.next;
            order.next = order
                .next
                .checked_add(1)
                .expect("upload terminal admission sequence exhausted");
            sequence
        };
        self.admitting_terminals.fetch_add(1, Ordering::AcqRel);
        if cancel {
            self.admitting_cancels.fetch_add(1, Ordering::AcqRel);
        }
        if preemptive {
            self.admitting_preemptive.fetch_add(1, Ordering::AcqRel);
        }
        self.terminal_notify.notify_waiters();
        self.terminal_notify.notify_one();
        TerminalAdmission {
            ingress: self.clone(),
            sequence,
            cancel,
            preemptive,
            completed: false,
        }
    }

    /// Finish or roll back one ordered admission while `sequencing` is held.
    fn finish_terminal_admission_locked(
        &self,
        sequence: u64,
        cancel: bool,
        preemptive: bool,
        _rolled_back: bool,
    ) {
        self.admitting_terminals.fetch_sub(1, Ordering::AcqRel);
        if cancel {
            self.admitting_cancels.fetch_sub(1, Ordering::AcqRel);
        }
        if preemptive {
            self.admitting_preemptive.fetch_sub(1, Ordering::AcqRel);
        }

        let mut order = self
            .terminal_order
            .lock()
            .expect("upload terminal order poisoned");
        if sequence == order.serving {
            order.serving = order
                .serving
                .checked_add(1)
                .expect("upload terminal serving sequence exhausted");
            loop {
                let serving = order.serving;
                if !order.cancelled.remove(&serving) {
                    break;
                }
                order.serving = order
                    .serving
                    .checked_add(1)
                    .expect("upload terminal serving sequence exhausted");
            }
        } else if sequence > order.serving {
            // Only a cancelled future can finish out of order: a live future
            // waits for `serving == sequence` before reserving the mailbox.
            order.cancelled.insert(sequence);
        }
        drop(order);
        self.terminal_order_notify.notify_waiters();
        self.terminal_order_notify.notify_one();
        self.terminal_notify.notify_waiters();
        self.terminal_notify.notify_one();
    }

    fn clear_processing_terminal(&self) {
        let _sequencing = self.sequencing.lock().expect("upload sequencing poisoned");
        self.processing_terminal.store(false, Ordering::Release);
        self.processing_preemptive.store(false, Ordering::Release);
    }

    fn commit_open_with<F>(&self, deadline: Instant, admit: F) -> ControlCommit
    where
        F: FnOnce() -> bool,
    {
        let _sequencing = self.sequencing.lock().expect("upload sequencing poisoned");
        // Preserve the already-sequenced cause. In particular, a connection
        // invalidated before the deadline remains transport loss even if the
        // writer task does not observe this callback until after the clock has
        // advanced past the deadline.
        if self.closed.load(Ordering::Acquire) || self.phase() != UploadPhase::Opening {
            return ControlCommit::Rejected;
        }
        if Instant::now() >= deadline {
            return ControlCommit::Deadline;
        }
        if !admit() {
            return ControlCommit::Rejected;
        }
        self.set_phase_locked(UploadPhase::Active);
        ControlCommit::Committed
    }

    fn try_start_active_credit(
        &self,
        attempt: &AtomicU8,
        absolute_deadline: Instant,
        stall_deadline: Instant,
        send_through: u64,
    ) -> bool {
        let _sequencing = self.sequencing.lock().expect("upload sequencing poisoned");
        let now = Instant::now();
        if self.closed.load(Ordering::Acquire)
            || self.phase() != UploadPhase::Active
            || self.has_terminal_pending()
            || now >= absolute_deadline
            || now >= stall_deadline
        {
            return false;
        }
        let committed = attempt
            .compare_exchange(
                CONTROL_ATTEMPT_QUEUED,
                CONTROL_ATTEMPT_STARTED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok();
        if committed {
            // Writer start is the peer-visible CREDIT commit boundary used by
            // every stream-control race. If the subsequent socket write
            // fails, connection invalidation prevents any guessed DATA from
            // producing a surviving result.
            self.served_send_through
                .store(send_through, Ordering::Release);
        }
        committed
    }

    fn commit_daemon_abort(&self, deadline: Instant) -> ControlCommit {
        let _sequencing = self.sequencing.lock().expect("upload sequencing poisoned");
        if self.closed.load(Ordering::Acquire) || self.phase() != UploadPhase::DaemonAbortQueued {
            return ControlCommit::Rejected;
        }
        if Instant::now() >= deadline {
            return ControlCommit::Deadline;
        }
        self.set_phase_locked(UploadPhase::DaemonAbortWaitAck);
        ControlCommit::Committed
    }

    fn commit_finalizing(&self, accepted: u64) -> bool {
        let _sequencing = self.sequencing.lock().expect("upload sequencing poisoned");
        if self.phase() != UploadPhase::Active {
            return false;
        }
        self.accepted.store(accepted, Ordering::Release);
        self.set_phase_locked(UploadPhase::Finalizing);
        true
    }

    fn select_daemon_abort(&self, terminal_yield: TerminalYield) -> DaemonAbortSelection {
        let _sequencing = self.sequencing.lock().expect("upload sequencing poisoned");
        match self.phase() {
            UploadPhase::Active => {
                let should_yield = match terminal_yield {
                    TerminalYield::None => false,
                    TerminalYield::Any => self.has_terminal_pending(),
                    TerminalYield::Preemptive => self.has_preemptive_terminal_pending(),
                };
                if should_yield {
                    return DaemonAbortSelection::TerminalPending;
                }
                self.set_phase_locked(UploadPhase::DaemonAbortQueued);
                DaemonAbortSelection::Selected
            }
            UploadPhase::DaemonAbortQueued => DaemonAbortSelection::Selected,
            UploadPhase::Opening
            | UploadPhase::DaemonAbortWaitAck
            | UploadPhase::ClientAbortPending
            | UploadPhase::ClientAbortAckCommitted
            | UploadPhase::ClientAbortAckSent
            | UploadPhase::Finalizing
            | UploadPhase::AckPending
            | UploadPhase::Retired => DaemonAbortSelection::Lost,
        }
    }

    fn try_select_daemon_abort(&self) -> bool {
        self.select_daemon_abort(TerminalYield::None) == DaemonAbortSelection::Selected
    }

    fn try_select_client_abort(&self) -> bool {
        let _sequencing = self.sequencing.lock().expect("upload sequencing poisoned");
        if self.phase() != UploadPhase::Active {
            return false;
        }
        self.set_phase_locked(UploadPhase::ClientAbortPending);
        true
    }

    fn promote_client_abort_protocol(&self) -> bool {
        let _sequencing = self.sequencing.lock().expect("upload sequencing poisoned");
        if !matches!(
            self.phase(),
            UploadPhase::ClientAbortPending
                | UploadPhase::ClientAbortAckCommitted
                | UploadPhase::ClientAbortAckSent
        ) {
            return false;
        }
        self.set_phase_locked(UploadPhase::DaemonAbortQueued);
        true
    }

    fn try_sequence_client_abort_ack(&self, attempt: &AtomicU8, deadline: Instant) -> bool {
        let _sequencing = self.sequencing.lock().expect("upload sequencing poisoned");
        if self.closed.load(Ordering::Acquire)
            || self.phase() != UploadPhase::ClientAbortPending
            || self.has_non_cancel_terminal_pending()
            || Instant::now() >= deadline
            || attempt
                .compare_exchange(
                    CONTROL_ATTEMPT_QUEUED,
                    CONTROL_ATTEMPT_STARTED,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_err()
        {
            return false;
        }
        self.set_phase_locked(UploadPhase::ClientAbortAckCommitted);
        true
    }

    fn cancel_client_abort_ack(&self, attempt: &AtomicU8) -> ClientAbortAckOutcome {
        let _sequencing = self.sequencing.lock().expect("upload sequencing poisoned");
        if attempt
            .compare_exchange(
                CONTROL_ATTEMPT_QUEUED,
                CONTROL_ATTEMPT_CANCELLED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            return if self.phase() == UploadPhase::ClientAbortPending
                && self.has_non_cancel_terminal_pending()
            {
                ClientAbortAckOutcome::ProtocolFaultBeforeSent
            } else {
                ClientAbortAckOutcome::Failed
            };
        }
        if self.phase() == UploadPhase::ClientAbortAckCommitted {
            ClientAbortAckOutcome::Sent
        } else {
            ClientAbortAckOutcome::Failed
        }
    }

    pub(crate) fn client_abort_ack_sent_locked(&self) -> bool {
        let _sequencing = self.sequencing.lock().expect("upload sequencing poisoned");
        if self.phase() == UploadPhase::ClientAbortAckCommitted
            && self.has_non_cancel_terminal_pending()
        {
            self.set_phase_locked(UploadPhase::ClientAbortAckSent);
            return false;
        }
        self.closed.store(true, Ordering::Release);
        self.set_phase_locked(UploadPhase::Retired);
        self.closed_notify.notify_waiters();
        self.closed_notify.notify_one();
        true
    }

    pub(crate) fn daemon_ack_retire_locked(&self) -> bool {
        let _sequencing = self.sequencing.lock().expect("upload sequencing poisoned");
        if self.phase() != UploadPhase::AckPending || self.has_non_cancel_terminal_pending() {
            return false;
        }
        self.closed.store(true, Ordering::Release);
        self.set_phase_locked(UploadPhase::Retired);
        self.closed_notify.notify_waiters();
        self.closed_notify.notify_one();
        true
    }

    fn classify_client_abort_ack_sent(&self) -> ClientAbortAckOutcome {
        let _sequencing = self.sequencing.lock().expect("upload sequencing poisoned");
        if self.phase() == UploadPhase::ClientAbortAckSent {
            ClientAbortAckOutcome::ProtocolFaultAfterSent
        } else if self.phase() == UploadPhase::Retired {
            ClientAbortAckOutcome::Sent
        } else {
            ClientAbortAckOutcome::Failed
        }
    }

    fn has_terminal_pending(&self) -> bool {
        self.admitting_terminals.load(Ordering::Acquire) != 0
            || self.queued_terminals.load(Ordering::Acquire) != 0
            || self.processing_terminal.load(Ordering::Acquire)
    }

    fn has_preemptive_terminal_pending(&self) -> bool {
        self.admitting_preemptive.load(Ordering::Acquire) != 0
            || self.queued_preemptive.load(Ordering::Acquire) != 0
            || self.processing_preemptive.load(Ordering::Acquire)
    }

    fn has_non_cancel_terminal_pending(&self) -> bool {
        self.admitting_terminals.load(Ordering::Acquire)
            > self.admitting_cancels.load(Ordering::Acquire)
            || self.queued_terminals.load(Ordering::Acquire)
                > self.queued_cancels.load(Ordering::Acquire)
    }

    fn commit_accepted(
        &self,
        accepted: u64,
        absolute_deadline: Instant,
        stall_deadline: Instant,
    ) -> Result<(), ActiveInterrupt> {
        let _sequencing = self.sequencing.lock().expect("upload sequencing poisoned");
        if self.has_preemptive_terminal_pending() {
            return Err(ActiveInterrupt::TerminalPending);
        }
        if self.closed.load(Ordering::Acquire) {
            return if self.has_terminal_pending() {
                Err(ActiveInterrupt::TerminalPending)
            } else {
                Err(ActiveInterrupt::TransportLost)
            };
        }
        if self.phase() != UploadPhase::Active {
            return Err(ActiveInterrupt::TransportLost);
        }
        let now = Instant::now();
        if now >= absolute_deadline {
            if !self.has_terminal_pending() {
                self.set_phase_locked(UploadPhase::DaemonAbortQueued);
            }
            return Err(ActiveInterrupt::Deadline);
        }
        if now >= stall_deadline {
            if !self.has_terminal_pending() {
                self.set_phase_locked(UploadPhase::DaemonAbortQueued);
            }
            return Err(ActiveInterrupt::Stall);
        }
        self.accepted.store(accepted, Ordering::Release);
        Ok(())
    }

    /// Called with the registry lock held, preserving the shared retirement
    /// lock order. FINALIZING is deliberately not cancelled by disconnect.
    pub(crate) fn invalidate_connection_locked(&self) {
        let _sequencing = self.sequencing.lock().expect("upload sequencing poisoned");
        self.closed.store(true, Ordering::Release);
        if self.phase() != UploadPhase::Finalizing
            && !self.has_terminal_pending()
            && !self.processing_terminal.load(Ordering::Acquire)
        {
            self.set_phase_locked(UploadPhase::Retired);
        }
        self.closed_notify.notify_waiters();
        self.closed_notify.notify_one();
    }

    /// Called with the registry lock held by `UploadStreamBinding::retire`.
    pub(crate) fn retire_locked(&self) {
        let _sequencing = self.sequencing.lock().expect("upload sequencing poisoned");
        self.closed.store(true, Ordering::Release);
        self.set_phase_locked(UploadPhase::Retired);
        self.closed_notify.notify_waiters();
        self.closed_notify.notify_one();
    }

    async fn enqueue(
        self: &Arc<Self>,
        event: UploadEvent,
        terminal: bool,
        mut admission: Option<TerminalAdmission>,
    ) -> BinaryRoute {
        let cancel = event.is_cancel();
        let preemptive = event.is_preemptive();
        if let Some(admission) = admission.as_ref() {
            admission.wait_for_turn().await;
        }
        let sender = if terminal {
            &self.terminal_sender
        } else {
            &self.data_sender
        };
        let permit = match sender.reserve().await {
            Ok(permit) => permit,
            Err(_) => return BinaryRoute::CloseMalformed,
        };
        let accepted = {
            let _sequencing = self.sequencing.lock().expect("upload sequencing poisoned");
            if self.phase() == UploadPhase::Retired
                || (self.closed.load(Ordering::Acquire) && admission.is_none())
            {
                false
            } else {
                self.queued_events.fetch_add(1, Ordering::AcqRel);
                if terminal {
                    self.queued_terminals.fetch_add(1, Ordering::AcqRel);
                }
                if cancel {
                    self.queued_cancels.fetch_add(1, Ordering::AcqRel);
                }
                if preemptive {
                    self.queued_preemptive.fetch_add(1, Ordering::AcqRel);
                }
                permit.send(event);
                if terminal {
                    admission
                        .as_mut()
                        .expect("terminal enqueue owns an admission marker")
                        .complete_locked();
                }
                true
            }
        };
        if !accepted {
            return BinaryRoute::CloseMalformed;
        }
        if terminal {
            self.terminal_notify.notify_waiters();
            self.terminal_notify.notify_one();
        }
        BinaryRoute::Delivered
    }

    async fn malformed(self: &Arc<Self>, received_at: Instant) -> BinaryRoute {
        let admission = {
            let _sequencing = self.sequencing.lock().expect("upload sequencing poisoned");
            if self.producer_terminal.load(Ordering::Acquire) == PRODUCER_TERMINAL_END {
                // Record the fault before awaiting mailbox capacity. If the
                // already-sequenced END wins while this follower is waiting,
                // the actor will still request the same connection-fatal 4007.
                self.post_end_fatal.store(true, Ordering::Release);
            }
            if self.phase() == UploadPhase::Finalizing
                || self.phase() == UploadPhase::Retired
                || self.closed.load(Ordering::Acquire)
            {
                None
            } else {
                Some(self.begin_terminal_locked(false, false))
            }
        };
        let Some(admission) = admission else {
            return BinaryRoute::CloseMalformed;
        };
        self.enqueue(
            UploadEvent {
                received_at,
                body: UploadEventBody::Malformed,
                _pending_data: None,
            },
            true,
            Some(admission),
        )
        .await
    }

    /// Route one exact-bound record after the registry has validated its full
    /// identity. Queue pressure awaits bounded capacity and applies ordinary
    /// WebSocket/TCP backpressure; it is never reclassified as malformed.
    pub(crate) async fn route_bound(
        self: &Arc<Self>,
        bytes: Bytes,
        bounds: &CodecBounds,
    ) -> BinaryRoute {
        let received_at = Instant::now();
        // Linearize credit observation before parsing or awaiting any ingress
        // capacity. A later writer-committed CREDIT must not retroactively
        // authorize this already received DATA message.
        let credit_at_receipt = self.served_send_through.load(Ordering::Acquire);
        let phase = {
            let _sequencing = self.sequencing.lock().expect("upload sequencing poisoned");
            let phase = self.phase();
            if phase == UploadPhase::Opening {
                self.opening_fatal.store(true, Ordering::Release);
                self.closed.store(true, Ordering::Release);
                return BinaryRoute::CloseMalformed;
            }
            if phase == UploadPhase::Retired {
                return BinaryRoute::CloseMalformed;
            }
            phase
        };

        let kind = match decode_stream_kind(&bytes, bounds) {
            Ok(kind) => kind,
            Err(_) if phase == UploadPhase::Finalizing => return BinaryRoute::CloseMalformed,
            Err(_) => return self.malformed(received_at).await,
        };
        let direction_ok = match phase {
            UploadPhase::Opening | UploadPhase::Retired => false,
            UploadPhase::Active => matches!(
                kind,
                StreamRecordKind::Data | StreamRecordKind::End | StreamRecordKind::Abort
            ),
            UploadPhase::DaemonAbortQueued => matches!(
                kind,
                StreamRecordKind::Data | StreamRecordKind::End | StreamRecordKind::Abort
            ),
            UploadPhase::DaemonAbortWaitAck => matches!(
                kind,
                StreamRecordKind::Data
                    | StreamRecordKind::End
                    | StreamRecordKind::Abort
                    | StreamRecordKind::Ack
            ),
            UploadPhase::ClientAbortPending => matches!(
                kind,
                StreamRecordKind::Data | StreamRecordKind::End | StreamRecordKind::Abort
            ),
            UploadPhase::ClientAbortAckCommitted | UploadPhase::ClientAbortAckSent => matches!(
                kind,
                StreamRecordKind::Data | StreamRecordKind::End | StreamRecordKind::Abort
            ),
            UploadPhase::Finalizing => kind == StreamRecordKind::Abort,
            UploadPhase::AckPending => false,
        };
        if !direction_ok {
            return if phase == UploadPhase::Finalizing {
                BinaryRoute::CloseMalformed
            } else {
                self.malformed(received_at).await
            };
        }

        let view = match decode_stream_record_view(&bytes, bounds) {
            Ok(view) => view,
            Err(_) if phase == UploadPhase::Finalizing => return BinaryRoute::CloseMalformed,
            Err(_) => return self.malformed(received_at).await,
        };
        if let StreamRecordBodyView::Abort { reason, .. } = view.body {
            if !matches!(
                reason,
                BinaryAbortReason::Cancelled
                    | BinaryAbortReason::SourceFailed
                    | BinaryAbortReason::ProtocolError
            ) {
                return if phase == UploadPhase::Finalizing {
                    BinaryRoute::CloseMalformed
                } else {
                    self.malformed(received_at).await
                };
            }
        }
        let (
            current_phase,
            terminal_conflict,
            admission,
            pending_data,
            rejected_data,
            discard_data,
        ) = {
            let _sequencing = self.sequencing.lock().expect("upload sequencing poisoned");
            let current_phase = self.phase();
            if self.closed.load(Ordering::Acquire) {
                return BinaryRoute::CloseMalformed;
            }
            let prior = self.producer_terminal.load(Ordering::Acquire);
            let valid_late_end_abort = prior == PRODUCER_TERMINAL_END
                && matches!(
                    view.body,
                    StreamRecordBodyView::Abort {
                        accepted_through,
                        ..
                    } if accepted_through == self.accepted.load(Ordering::Acquire)
                );
            let mut terminal_conflict = match prior {
                PRODUCER_TERMINAL_NONE => {
                    if kind == StreamRecordKind::End {
                        self.producer_terminal
                            .store(PRODUCER_TERMINAL_END, Ordering::Release);
                    } else if kind == StreamRecordKind::Abort {
                        self.producer_terminal
                            .store(PRODUCER_TERMINAL_ABORT, Ordering::Release);
                    }
                    false
                }
                // ACK can still discharge a daemon ABORT whose timer won the
                // actor race. No producer payload/control may follow END.
                PRODUCER_TERMINAL_END => kind != StreamRecordKind::Ack && !valid_late_end_abort,
                // ABORT ends producer traffic. A later producer record is a
                // request-local protocol fault; only ACK for a crossed daemon
                // ABORT remains legal.
                PRODUCER_TERMINAL_ABORT => kind != StreamRecordKind::Ack,
                _ => true,
            };
            if kind == StreamRecordKind::Ack && current_phase != UploadPhase::DaemonAbortWaitAck {
                terminal_conflict = true;
            }
            if terminal_conflict && prior == PRODUCER_TERMINAL_END {
                self.post_end_fatal.store(true, Ordering::Release);
            }
            if current_phase == UploadPhase::DaemonAbortWaitAck && kind == StreamRecordKind::Ack {
                self.set_phase_locked(UploadPhase::AckPending);
            }
            let mut rejected_data = None;
            let mut discard_data = false;
            if kind == StreamRecordKind::Data && !terminal_conflict {
                if current_phase != UploadPhase::Active
                    || self.data_refusal_pending.load(Ordering::Acquire)
                    || self.has_preemptive_terminal_pending()
                {
                    // A daemon/client terminal that already won needs only its
                    // exact ACK obligations. Older producer DATA is discarded
                    // without occupying the legal receive window so a later
                    // ACK or unrelated control remains readable.
                    discard_data = true;
                } else if let StreamRecordBodyView::Data { offset, payload } = view.body {
                    if let Err(failure) =
                        self.admit_received_data_locked(offset, payload.len(), credit_at_receipt)
                    {
                        self.data_refusal_pending.store(true, Ordering::Release);
                        rejected_data = Some(failure);
                    }
                }
            }
            // A DATA record is normally routed through the bounded data lane,
            // but a DATA follower after a producer terminal is itself a
            // terminal protocol fault. A preflight DATA refusal also bypasses
            // DATA capacity as ordered, preemptive terminal work.
            let admission =
                (kind != StreamRecordKind::Data || terminal_conflict || rejected_data.is_some())
                    .then(|| {
                        self.begin_terminal_locked(
                            false,
                            rejected_data.is_some()
                                || (kind == StreamRecordKind::Abort && !terminal_conflict),
                        )
                    });
            let pending_data = (kind == StreamRecordKind::Data
                && !terminal_conflict
                && rejected_data.is_none()
                && !discard_data)
                .then(|| PendingData::new(self.clone(), credit_at_receipt));
            (
                current_phase,
                terminal_conflict,
                admission,
                pending_data,
                rejected_data,
                discard_data,
            )
        };
        if current_phase == UploadPhase::Finalizing {
            // END already owns the result. A structurally valid late producer
            // ABORT cannot change it and needs no second terminal exchange.
            let valid_late_abort = matches!(
                view.body,
                StreamRecordBodyView::Abort {
                    accepted_through,
                    ..
                } if accepted_through == self.accepted.load(Ordering::Acquire)
            );
            return if valid_late_abort {
                BinaryRoute::Delivered
            } else {
                BinaryRoute::CloseMalformed
            };
        }
        if terminal_conflict {
            return self
                .enqueue(
                    UploadEvent {
                        received_at,
                        body: UploadEventBody::Malformed,
                        _pending_data: None,
                    },
                    true,
                    admission,
                )
                .await;
        }
        if discard_data {
            return BinaryRoute::Delivered;
        }
        if let Some(failure) = rejected_data {
            return self
                .enqueue(
                    UploadEvent {
                        received_at,
                        body: UploadEventBody::RejectedData(failure),
                        _pending_data: None,
                    },
                    true,
                    admission,
                )
                .await;
        }

        let byte_permit = if kind == StreamRecordKind::Data {
            let permits = match u32::try_from(bytes.len()) {
                Ok(permits) => permits,
                Err(_) => return self.malformed(received_at).await,
            };
            match self.budget.bytes.clone().acquire_many_owned(permits).await {
                Ok(permit) => Some(permit),
                Err(_) => return BinaryRoute::CloseMalformed,
            }
        } else {
            None
        };
        self.enqueue(
            UploadEvent {
                received_at,
                body: UploadEventBody::Record {
                    kind,
                    bytes,
                    _bytes: byte_permit,
                },
                _pending_data: pending_data,
            },
            kind != StreamRecordKind::Data,
            admission,
        )
        .await
    }

    async fn request_cancel(self: &Arc<Self>) -> CancelAttempt {
        let received_at = Instant::now();
        let admission = {
            let _sequencing = self.sequencing.lock().expect("upload sequencing poisoned");
            if self.phase() != UploadPhase::Active || self.closed.load(Ordering::Acquire) {
                return CancelAttempt::Unknown;
            }
            self.begin_terminal_locked(true, true)
        };
        let (response, result) = oneshot::channel();
        if self
            .enqueue(
                UploadEvent {
                    received_at,
                    body: UploadEventBody::Cancel { response },
                    _pending_data: None,
                },
                true,
                Some(admission),
            )
            .await
            != BinaryRoute::Delivered
        {
            return CancelAttempt::Unknown;
        }
        result.await.unwrap_or(CancelAttempt::Unknown)
    }
}

#[derive(Clone)]
struct UploadCancelTarget {
    ingress: Arc<UploadIngress>,
    total: u64,
}

/// Daemon-global principal-scoped cancellation table for streamed uploads.
#[derive(Clone)]
pub(crate) struct UploadCancellationRegistry {
    inner: Arc<Mutex<HashMap<(String, OpId), UploadCancelTarget>>>,
    engine: Option<Weak<Engine>>,
}

pub(crate) struct UploadCancellationGuard {
    registry: UploadCancellationRegistry,
    key: (String, OpId),
    ingress: Arc<UploadIngress>,
}

impl UploadCancellationRegistry {
    pub(crate) fn with_engine(engine: &Arc<Engine>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            engine: Some(Arc::downgrade(engine)),
        }
    }

    fn register(
        &self,
        principal: String,
        op_id: OpId,
        ingress: Arc<UploadIngress>,
        total: u64,
    ) -> Option<UploadCancellationGuard> {
        let key = (principal, op_id);
        let mut entries = self
            .inner
            .lock()
            .expect("upload cancellation registry poisoned");
        if entries.contains_key(&key) {
            return None;
        }
        entries.insert(
            key.clone(),
            UploadCancelTarget {
                ingress: ingress.clone(),
                total,
            },
        );
        Some(UploadCancellationGuard {
            registry: self.clone(),
            key,
            ingress,
        })
    }

    pub(crate) async fn cancel(
        &self,
        principal: &str,
        request: &TransferCancel,
    ) -> Result<TransferCancelOut, ApiError> {
        let key = (principal.to_owned(), request.transfer_op_id.clone());
        let target = {
            let entries = self
                .inner
                .lock()
                .expect("upload cancellation registry poisoned");
            match entries.get(&key) {
                Some(target) => target.clone(),
                None => {
                    return self
                        .recorded_cancellation(principal, request)
                        .ok_or_else(|| ApiError::TransferUnknown {
                            transfer_op_id: request.transfer_op_id.clone(),
                        });
                }
            }
        };

        match target.ingress.request_cancel().await {
            CancelAttempt::Cancelled { accepted } => {
                // The actor annotates the canonical ledger before removing
                // the active target and satisfying this oneshot. Therefore a
                // concurrent or immediately repeated cancel cannot observe an
                // Active/recorded gap and misreport `transfer_unknown`.
                Ok(TransferCancelOut {
                    transfer_op_id: request.transfer_op_id.clone(),
                    outcome: CancelOutcome::Cancelled,
                    transferred_bytes: accepted,
                    total: ByteTotal::Known {
                        bytes: target.total,
                    },
                })
            }
            CancelAttempt::Unknown => {
                let mut entries = self
                    .inner
                    .lock()
                    .expect("upload cancellation registry poisoned");
                if matches!(
                    entries.get(&key),
                    Some(current) if Arc::ptr_eq(&current.ingress, &target.ingress)
                ) {
                    entries.remove(&key);
                }
                drop(entries);
                self.recorded_cancellation(principal, request)
                    .ok_or_else(|| ApiError::TransferUnknown {
                        transfer_op_id: request.transfer_op_id.clone(),
                    })
            }
        }
    }

    fn recorded_cancellation(
        &self,
        principal: &str,
        request: &TransferCancel,
    ) -> Option<TransferCancelOut> {
        let engine = self.engine.as_ref()?.upgrade()?;
        let (transferred_bytes, total) =
            engine.recorded_file_share_cancellation(principal, &request.transfer_op_id)?;
        Some(TransferCancelOut {
            transfer_op_id: request.transfer_op_id.clone(),
            outcome: CancelOutcome::AlreadyCancelled,
            transferred_bytes,
            total: ByteTotal::Known { bytes: total },
        })
    }
}

impl Default for UploadCancellationRegistry {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            engine: None,
        }
    }
}

impl UploadCancellationGuard {
    fn select_cancelled(
        &self,
        owner: &mut FileShareLedgerOwner,
        transferred: u64,
        total: u64,
    ) -> bool {
        let mut entries = self
            .registry
            .inner
            .lock()
            .expect("upload cancellation registry poisoned");
        if !matches!(
            entries.get(&self.key),
            Some(current) if Arc::ptr_eq(&current.ingress, &self.ingress)
        ) {
            return false;
        }
        if !self.ingress.try_select_daemon_abort() {
            return false;
        }
        // Preserve the lock order registry -> ingress sequencing -> ledger.
        // Removing the live target only after this annotation makes a miss
        // provably replayable as `already_cancelled`, even before ABORT/ACK
        // publishes the file.share terminal result.
        owner.record_cancel_selected(transferred, total);
        entries.remove(&self.key);
        true
    }

    fn unregister(&self) {
        let mut entries = self
            .registry
            .inner
            .lock()
            .expect("upload cancellation registry poisoned");
        if matches!(
            entries.get(&self.key),
            Some(current) if Arc::ptr_eq(&current.ingress, &self.ingress)
        ) {
            entries.remove(&self.key);
        }
    }
}

impl Drop for UploadCancellationGuard {
    fn drop(&mut self) {
        self.unregister();
    }
}

#[derive(Clone)]
struct UploadRequestLease {
    permit: Arc<Mutex<Option<RequestPermit>>>,
}

impl UploadRequestLease {
    fn new(permit: RequestPermit) -> Self {
        Self {
            permit: Arc::new(Mutex::new(Some(permit))),
        }
    }

    fn release(&self) {
        drop(
            self.permit
                .lock()
                .expect("upload request lease poisoned")
                .take(),
        );
    }
}

#[derive(Debug)]
struct ConsumerState {
    declared: u64,
    file_limit: u64,
    accepted: u64,
    credited_accepted: u64,
    send_through: u64,
    credit_history: VecDeque<(u64, u64)>,
}

impl ConsumerState {
    fn new(declared: u64, file_limit: u64) -> Self {
        Self {
            declared,
            file_limit,
            accepted: 0,
            credited_accepted: 0,
            send_through: 0,
            credit_history: VecDeque::new(),
        }
    }

    fn probe_limit(&self) -> Option<u64> {
        self.declared.min(self.file_limit).checked_add(1)
    }

    fn next_credit(&self, window: u64) -> Option<(u64, u64)> {
        let probe = self.probe_limit()?;
        // The observation sentinel is capacity only after every declared
        // logical byte is durably accepted. Before that boundary CREDIT may
        // reach the logical limit, but never cross it.
        let send = if self.accepted == self.declared.min(self.file_limit) {
            probe
        } else {
            self.accepted
                .checked_add(window)?
                .min(self.declared.min(self.file_limit))
        };
        Some((self.accepted, send))
    }

    fn record_credit(&mut self, accepted: u64, send: u64) -> Result<(), ()> {
        if accepted != self.accepted
            || accepted < self.credited_accepted
            || send < self.send_through
            || accepted > send
            || send > self.probe_limit().ok_or(())?
        {
            return Err(());
        }
        self.credited_accepted = accepted;
        self.send_through = send;
        if self.credit_history.back().copied() != Some((accepted, send)) {
            self.credit_history.push_back((accepted, send));
        }
        Ok(())
    }

    #[cfg(test)]
    fn validate_data(&mut self, offset: u64, payload_len: usize) -> Result<u64, DataFailure> {
        self.validate_data_at_credit(offset, payload_len, self.send_through)
    }

    fn validate_data_at_credit(
        &mut self,
        offset: u64,
        payload_len: usize,
        credit_at_receipt: u64,
    ) -> Result<u64, DataFailure> {
        let payload = u64::try_from(payload_len).map_err(|_| DataFailure::Protocol)?;
        let candidate = offset.checked_add(payload).ok_or(DataFailure::Protocol)?;
        if candidate > self.file_limit {
            return Err(DataFailure::TooLarge {
                observed: candidate,
            });
        }
        if candidate > self.declared {
            return Err(DataFailure::Declared {
                observed: candidate,
            });
        }
        if offset != self.accepted {
            return Err(DataFailure::Protocol);
        }
        if candidate > credit_at_receipt || credit_at_receipt > self.send_through {
            return Err(DataFailure::Protocol);
        }
        // Crossing a prior send limit proves the producer observed a later
        // CREDIT. Older accepted values can no longer be the high-water mark
        // it legitimately echoes in ABORT, keeping this history bounded by
        // the explicit in-flight window rather than by file size.
        while self.credit_history.len() > 1
            && self
                .credit_history
                .front()
                .is_some_and(|(_, send)| *send < candidate)
        {
            self.credit_history.pop_front();
        }
        Ok(candidate)
    }

    fn data_accepted(&mut self, candidate: u64) {
        self.accepted = candidate;
    }

    fn validate_end(&self, total: u64) -> Result<(), DataFailure> {
        if total != self.accepted {
            return Err(DataFailure::Protocol);
        }
        if total < self.declared {
            return Err(DataFailure::Declared { observed: total });
        }
        if total != self.declared
            || self.credited_accepted != self.accepted
            || self.send_through != self.probe_limit().ok_or(DataFailure::Protocol)?
        {
            return Err(DataFailure::Protocol);
        }
        Ok(())
    }

    fn should_credit(&self) -> bool {
        self.send_through == 0
            || self.accepted == self.send_through
            || self.accepted == self.declared
    }

    fn valid_producer_abort(&self, echoed_accepted: u64) -> bool {
        self.credit_history
            .iter()
            .any(|(accepted, _)| *accepted == echoed_accepted)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DataFailure {
    Protocol,
    TooLarge { observed: u64 },
    Declared { observed: u64 },
}

#[derive(Debug, Clone)]
enum UploadFailure {
    Protocol,
    Sink,
    Cancelled,
    Deadline { budget_ms: u64 },
    Stall,
    TooLarge,
    Declared { observed: u64 },
}

fn known_total(total: u64) -> ByteTotal {
    ByteTotal::Known { bytes: total }
}

fn stream_bytes(
    identity: StreamIdentity,
    body: StreamRecordBody,
    bounds: &CodecBounds,
) -> Option<Vec<u8>> {
    encode_stream_record(&StreamRecord { identity, body }, bounds).ok()
}

fn reply_bytes<E>(id: u64, result: Result<&FileShareOut, E>) -> Vec<u8>
where
    E: std::borrow::Borrow<ApiError>,
{
    let reply = match result {
        Ok(out) => jeliya_codec::Reply {
            id,
            ok: true,
            out: serde_json::to_value(out).ok(),
            err: None,
        },
        Err(error) => jeliya_codec::Reply {
            id,
            ok: false,
            out: None,
            err: Some(error.borrow().clone()),
        },
    };
    reply.to_bytes()
}

async fn send_reply<E>(
    outbound: &Outbound,
    id: u64,
    result: Result<&FileShareOut, E>,
    request: &UploadRequestLease,
) -> bool
where
    E: std::borrow::Borrow<ApiError>,
{
    let release = request.clone();
    outbound
        .text_with_on_sent(reply_bytes(id, result), move || release.release())
        .await
        == WriteReceipt::Sent
}

async fn send_reply_and_retire<E>(
    outbound: &Outbound,
    id: u64,
    result: Result<&FileShareOut, E>,
    request: &UploadRequestLease,
    binding: &UploadStreamBinding,
) -> bool
where
    E: std::borrow::Borrow<ApiError>,
{
    let release = request.clone();
    let retirement = binding.retirement();
    outbound
        .text_with_on_sent(reply_bytes(id, result), move || {
            retirement.retire();
            release.release();
        })
        .await
        == WriteReceipt::Sent
}

fn failure_error(failure: &UploadFailure, state: &ConsumerState) -> ApiError {
    match failure {
        UploadFailure::Protocol => ApiError::MalformedFrame,
        UploadFailure::Sink => ApiError::StreamAborted {
            transferred_bytes: state.accepted,
            total: known_total(state.declared),
            reason: StreamAbortReason::SinkFailed,
        },
        UploadFailure::Cancelled => ApiError::StreamAborted {
            transferred_bytes: state.accepted,
            total: known_total(state.declared),
            reason: StreamAbortReason::Cancelled,
        },
        UploadFailure::Deadline { budget_ms } => ApiError::TransferDeadlineExceeded {
            transferred_bytes: state.accepted,
            total: known_total(state.declared),
            budget_ms: *budget_ms,
        },
        UploadFailure::Stall => ApiError::TransferStalled {
            transferred_bytes: state.accepted,
            total: known_total(state.declared),
        },
        UploadFailure::TooLarge => ApiError::FileTooLarge {
            declared_bytes: state.declared,
            limit_bytes: state.file_limit,
            enforced_at: EnforcedAt::StageStream,
        },
        UploadFailure::Declared { observed } => ApiError::DeclaredSizeMismatch {
            declared_bytes: state.declared,
            observed_bytes: *observed,
        },
    }
}

fn failure_abort_reason(failure: &UploadFailure) -> BinaryAbortReason {
    match failure {
        UploadFailure::Protocol => BinaryAbortReason::ProtocolError,
        UploadFailure::Sink => BinaryAbortReason::SinkFailed,
        UploadFailure::Cancelled => BinaryAbortReason::Cancelled,
        UploadFailure::Deadline { .. }
        | UploadFailure::Stall
        | UploadFailure::TooLarge
        | UploadFailure::Declared { .. } => BinaryAbortReason::OperationError,
    }
}

fn final_sink_error(error: FileShareSinkError, state: &ConsumerState) -> ApiError {
    match error {
        FileShareSinkError::FileTooLarge {
            candidate: _,
            limit,
        } => ApiError::FileTooLarge {
            declared_bytes: state.declared,
            limit_bytes: limit,
            enforced_at: EnforcedAt::StageStream,
        },
        FileShareSinkError::DeclaredSizeMismatch { declared, observed } => {
            ApiError::DeclaredSizeMismatch {
                declared_bytes: declared,
                observed_bytes: observed,
            }
        }
        FileShareSinkError::OffsetMismatch { .. }
        | FileShareSinkError::Arithmetic
        | FileShareSinkError::EmptyRecord => ApiError::MalformedFrame,
        FileShareSinkError::CountDisagreement { observed, .. } => ApiError::DeclaredSizeMismatch {
            declared_bytes: state.declared,
            observed_bytes: observed,
        },
        FileShareSinkError::Create(_)
        | FileShareSinkError::Write { .. }
        | FileShareSinkError::PartialWrite { .. }
        | FileShareSinkError::Flush(_)
        | FileShareSinkError::Sync(_)
        | FileShareSinkError::StagingDisappeared
        | FileShareSinkError::StagingReplaced
        | FileShareSinkError::Cleanup(_)
        | FileShareSinkError::SinkClosed => ApiError::StreamAborted {
            transferred_bytes: state.accepted,
            total: known_total(state.declared),
            reason: StreamAbortReason::SinkFailed,
        },
    }
}

async fn send_control_until<F>(
    outbound: &Outbound,
    bytes: Vec<u8>,
    deadline: Instant,
    commit: F,
) -> BoundedControlOutcome
where
    F: FnOnce() -> ControlCommit + Send + 'static,
{
    let attempt = Arc::new(AtomicU8::new(CONTROL_ATTEMPT_QUEUED));
    let commit_result = Arc::new(AtomicU8::new(CONTROL_COMMIT_PENDING));
    let stream_live = Arc::new(AtomicBool::new(true));
    let commit_attempt = attempt.clone();
    let writer_commit_result = commit_result.clone();
    let write = outbound.binary_control_with_start(bytes, stream_live.clone(), move || {
        if commit_attempt
            .compare_exchange(
                CONTROL_ATTEMPT_QUEUED,
                CONTROL_ATTEMPT_STARTED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            return false;
        }
        let decision = commit();
        writer_commit_result.store(decision as u8, Ordering::Release);
        decision == ControlCommit::Committed
    });
    tokio::pin!(write);
    let classify = |receipt| match receipt {
        WriteReceipt::Sent => BoundedControlOutcome::Sent,
        WriteReceipt::Discarded
            if ControlCommit::load(&commit_result) == ControlCommit::Deadline =>
        {
            BoundedControlOutcome::Deadline
        }
        WriteReceipt::Discarded | WriteReceipt::Closed => BoundedControlOutcome::DiscardedOrClosed,
    };
    tokio::select! {
        biased;
        receipt = &mut write => classify(receipt),
        () = tokio::time::sleep_until(deadline) => {
            if attempt.compare_exchange(
                CONTROL_ATTEMPT_QUEUED,
                CONTROL_ATTEMPT_CANCELLED,
                Ordering::AcqRel,
                Ordering::Acquire,
            ).is_ok() {
                stream_live.store(false, Ordering::Release);
                BoundedControlOutcome::Deadline
            } else {
                // Writer start won before the bound. Reconcile the finite
                // writer watchdog because the control may be peer-visible.
                classify(write.await)
            }
        }
    }
}

async fn send_open<F>(
    outbound: &Outbound,
    ingress: &Arc<UploadIngress>,
    identity: StreamIdentity,
    total: u64,
    bounds: &CodecBounds,
    deadline: Instant,
    admit: F,
) -> BoundedControlOutcome
where
    F: FnOnce() -> bool + Send + 'static,
{
    let Some(open) = stream_bytes(identity, StreamRecordBody::Open { total }, bounds) else {
        return BoundedControlOutcome::DiscardedOrClosed;
    };
    let commit_ingress = ingress.clone();
    send_control_until(outbound, open, deadline, move || {
        commit_ingress.commit_open_with(deadline, admit)
    })
    .await
}

#[derive(Clone, Copy)]
enum ControlInterrupt {
    TerminalPending,
    Deadline,
    Stall,
    TransportLost,
}

struct ActiveControlResult {
    receipt: WriteReceipt,
    interrupt: Option<ControlInterrupt>,
}

#[allow(clippy::too_many_arguments)]
async fn send_active_control(
    outbound: &Outbound,
    ingress: &Arc<UploadIngress>,
    identity: StreamIdentity,
    body: StreamRecordBody,
    bounds: &CodecBounds,
    absolute_deadline: Instant,
    stall_deadline: Instant,
    committed_send_through: u64,
) -> ActiveControlResult {
    let Some(bytes) = stream_bytes(identity, body, bounds) else {
        return ActiveControlResult {
            receipt: WriteReceipt::Closed,
            interrupt: None,
        };
    };
    let attempt = Arc::new(AtomicU8::new(CONTROL_ATTEMPT_QUEUED));
    let live = Arc::new(AtomicBool::new(true));
    let commit_attempt = attempt.clone();
    let commit_ingress = ingress.clone();
    let write = outbound.binary_control_with_start(bytes, live.clone(), move || {
        commit_ingress.try_start_active_credit(
            &commit_attempt,
            absolute_deadline,
            stall_deadline,
            committed_send_through,
        )
    });
    tokio::pin!(write);
    loop {
        let interrupt_now = if ingress.has_terminal_pending() {
            Some(ControlInterrupt::TerminalPending)
        } else if ingress.closed.load(Ordering::Acquire) {
            Some(ControlInterrupt::TransportLost)
        } else if Instant::now() > absolute_deadline {
            Some(ControlInterrupt::Deadline)
        } else if Instant::now() > stall_deadline {
            Some(ControlInterrupt::Stall)
        } else {
            None
        };
        if let Some(interrupt) = interrupt_now {
            if attempt
                .compare_exchange(
                    CONTROL_ATTEMPT_QUEUED,
                    CONTROL_ATTEMPT_CANCELLED,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                live.store(false, Ordering::Release);
                return ActiveControlResult {
                    receipt: WriteReceipt::Discarded,
                    interrupt: Some(interrupt),
                };
            }
            return ActiveControlResult {
                receipt: write.await,
                interrupt: Some(interrupt),
            };
        }
        tokio::select! {
            biased;
            () = ingress.terminal_notify.notified() => {
                if !ingress.has_terminal_pending() {
                    continue;
                }
                let interrupt = ControlInterrupt::TerminalPending;
                if attempt.compare_exchange(
                    CONTROL_ATTEMPT_QUEUED,
                    CONTROL_ATTEMPT_CANCELLED,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ).is_ok() {
                    live.store(false, Ordering::Release);
                    return ActiveControlResult {
                        receipt: WriteReceipt::Discarded,
                        interrupt: Some(interrupt),
                    };
                }
                return ActiveControlResult {
                    receipt: write.await,
                    interrupt: Some(interrupt),
                };
            }
            () = ingress.closed_notify.notified() => {
                let interrupt = ControlInterrupt::TransportLost;
                if attempt.compare_exchange(
                    CONTROL_ATTEMPT_QUEUED,
                    CONTROL_ATTEMPT_CANCELLED,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ).is_ok() {
                    live.store(false, Ordering::Release);
                    return ActiveControlResult {
                        receipt: WriteReceipt::Discarded,
                        interrupt: Some(interrupt),
                    };
                }
                return ActiveControlResult {
                    receipt: write.await,
                    interrupt: Some(interrupt),
                };
            }
            () = tokio::time::sleep_until(absolute_deadline) => {
                let interrupt = ControlInterrupt::Deadline;
                if attempt.compare_exchange(
                    CONTROL_ATTEMPT_QUEUED,
                    CONTROL_ATTEMPT_CANCELLED,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ).is_ok() {
                    live.store(false, Ordering::Release);
                    return ActiveControlResult {
                        receipt: WriteReceipt::Discarded,
                        interrupt: Some(interrupt),
                    };
                }
                return ActiveControlResult {
                    receipt: write.await,
                    interrupt: Some(interrupt),
                };
            }
            () = tokio::time::sleep_until(stall_deadline) => {
                let interrupt = ControlInterrupt::Stall;
                if attempt.compare_exchange(
                    CONTROL_ATTEMPT_QUEUED,
                    CONTROL_ATTEMPT_CANCELLED,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ).is_ok() {
                    live.store(false, Ordering::Release);
                    return ActiveControlResult {
                        receipt: WriteReceipt::Discarded,
                        interrupt: Some(interrupt),
                    };
                }
                return ActiveControlResult {
                    receipt: write.await,
                    interrupt: Some(interrupt),
                };
            }
            receipt = &mut write => {
                return ActiveControlResult {
                    receipt,
                    interrupt: None,
                };
            }
        }
    }
}

async fn send_abort(
    outbound: &Outbound,
    ingress: &Arc<UploadIngress>,
    identity: StreamIdentity,
    accepted: u64,
    reason: BinaryAbortReason,
    bounds: &CodecBounds,
    deadline: Instant,
) -> BoundedControlOutcome {
    let Some(bytes) = stream_bytes(
        identity,
        StreamRecordBody::Abort {
            accepted_through: accepted,
            reason,
        },
        bounds,
    ) else {
        return BoundedControlOutcome::DiscardedOrClosed;
    };
    let commit_ingress = ingress.clone();
    send_control_until(outbound, bytes, deadline, move || {
        commit_ingress.commit_daemon_abort(deadline)
    })
    .await
}

async fn send_ack_until(
    outbound: &Outbound,
    identity: StreamIdentity,
    accepted: u64,
    bounds: &CodecBounds,
    deadline: Instant,
) -> BoundedControlOutcome {
    let Some(bytes) = stream_bytes(
        identity,
        StreamRecordBody::Ack {
            accepted_through: accepted,
        },
        bounds,
    ) else {
        return BoundedControlOutcome::DiscardedOrClosed;
    };
    send_control_until(outbound, bytes, deadline, move || {
        if Instant::now() < deadline {
            ControlCommit::Committed
        } else {
            ControlCommit::Deadline
        }
    })
    .await
}

async fn send_client_abort_ack_until(
    outbound: &Outbound,
    ingress: &Arc<UploadIngress>,
    retirement: UploadStreamRetirement,
    identity: StreamIdentity,
    accepted: u64,
    bounds: &CodecBounds,
    deadline: Instant,
) -> ClientAbortAckOutcome {
    let Some(bytes) = stream_bytes(
        identity,
        StreamRecordBody::Ack {
            accepted_through: accepted,
        },
        bounds,
    ) else {
        return ClientAbortAckOutcome::Failed;
    };
    let attempt = Arc::new(AtomicU8::new(CONTROL_ATTEMPT_QUEUED));
    let stream_live = Arc::new(AtomicBool::new(true));
    let commit_attempt = attempt.clone();
    let commit_ingress = ingress.clone();
    let write = outbound.binary_control_with_hooks(
        bytes,
        stream_live.clone(),
        move || commit_ingress.try_sequence_client_abort_ack(&commit_attempt, deadline),
        move || retirement.retire_client_abort_ack(),
    );
    tokio::pin!(write);
    tokio::select! {
        biased;
        receipt = &mut write => match receipt {
            WriteReceipt::Sent => ingress.classify_client_abort_ack_sent(),
            WriteReceipt::Discarded => ingress.cancel_client_abort_ack(&attempt),
            WriteReceipt::Closed => ClientAbortAckOutcome::Failed,
        },
        () = tokio::time::sleep_until(deadline) => {
            let outcome = ingress.cancel_client_abort_ack(&attempt);
            match outcome {
                ClientAbortAckOutcome::Sent => {
                    if write.await == WriteReceipt::Sent {
                        ingress.classify_client_abort_ack_sent()
                    } else {
                        ClientAbortAckOutcome::Failed
                    }
                }
                other => {
                    stream_live.store(false, Ordering::Release);
                    other
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn issue_credit(
    outbound: &Outbound,
    ingress: &Arc<UploadIngress>,
    identity: StreamIdentity,
    state: &mut ConsumerState,
    window: u64,
    bounds: &CodecBounds,
    absolute_deadline: Instant,
    stall_deadline: Instant,
    budget_ms: u64,
) -> Result<CreditOutcome, UploadFailure> {
    let (accepted, send_through) = state.next_credit(window).ok_or(UploadFailure::Protocol)?;
    let receipt = send_active_control(
        outbound,
        ingress,
        identity,
        StreamRecordBody::Credit {
            accepted_through: accepted,
            send_through,
        },
        bounds,
        absolute_deadline,
        stall_deadline,
        send_through,
    )
    .await;
    if receipt.receipt == WriteReceipt::Sent {
        state
            .record_credit(accepted, send_through)
            .map_err(|()| UploadFailure::Protocol)?;
    }
    match receipt.interrupt {
        Some(ControlInterrupt::TerminalPending) => Ok(CreditOutcome::TerminalPending),
        Some(ControlInterrupt::TransportLost) => Ok(CreditOutcome::TransportLost),
        Some(ControlInterrupt::Deadline) => Err(UploadFailure::Deadline { budget_ms }),
        Some(ControlInterrupt::Stall) => Err(UploadFailure::Stall),
        None if receipt.receipt == WriteReceipt::Sent => Ok(CreditOutcome::Sent),
        None if receipt.receipt == WriteReceipt::Closed => Ok(CreditOutcome::TransportLost),
        None => Err(UploadFailure::Protocol),
    }
}

enum CreditOutcome {
    Sent,
    TerminalPending,
    TransportLost,
}

enum ActiveInput {
    Event(UploadEvent),
    Deadline,
    Stall,
    TransportLost,
}

enum ActiveInterrupt {
    TerminalPending,
    Deadline,
    Stall,
    TransportLost,
}

fn explicit_terminal_wins_timer_tie(
    event: &UploadEvent,
    state: &ConsumerState,
    bounds: &CodecBounds,
) -> bool {
    match &event.body {
        UploadEventBody::Cancel { .. } => true,
        UploadEventBody::Record {
            kind: StreamRecordKind::Abort,
            bytes,
            ..
        } => decode_stream_record_view(bytes, bounds).is_ok_and(|view| {
            matches!(
                view.body,
                StreamRecordBodyView::Abort {
                    accepted_through,
                    reason:
                        BinaryAbortReason::Cancelled
                        | BinaryAbortReason::SourceFailed
                        | BinaryAbortReason::ProtocolError,
                } if state.valid_producer_abort(accepted_through)
            )
        }),
        UploadEventBody::Record { .. }
        | UploadEventBody::RejectedData(_)
        | UploadEventBody::Malformed => false,
    }
}

fn active_event_timer_failure(
    event: &UploadEvent,
    state: &ConsumerState,
    bounds: &CodecBounds,
    absolute_deadline: Instant,
    stall_deadline: Instant,
    budget_ms: u64,
) -> Option<UploadFailure> {
    let explicit_terminal = explicit_terminal_wins_timer_tie(event, state, bounds);
    if event.received_at > absolute_deadline
        || (event.received_at == absolute_deadline && !explicit_terminal)
    {
        Some(UploadFailure::Deadline { budget_ms })
    } else if event.received_at > stall_deadline
        || (event.received_at == stall_deadline && !explicit_terminal)
    {
        Some(UploadFailure::Stall)
    } else {
        None
    }
}

async fn await_active_operation<F, T>(
    operation: F,
    ingress: &Arc<UploadIngress>,
    absolute_deadline: Instant,
    stall_deadline: Instant,
) -> Result<T, ActiveInterrupt>
where
    F: std::future::Future<Output = T>,
{
    tokio::pin!(operation);
    loop {
        if ingress.has_preemptive_terminal_pending() {
            return Err(ActiveInterrupt::TerminalPending);
        }
        if ingress.closed.load(Ordering::Acquire) {
            return if ingress.has_terminal_pending() {
                Err(ActiveInterrupt::TerminalPending)
            } else {
                Err(ActiveInterrupt::TransportLost)
            };
        }
        let now = Instant::now();
        if now > absolute_deadline {
            return Err(ActiveInterrupt::Deadline);
        }
        if now > stall_deadline {
            return Err(ActiveInterrupt::Stall);
        }
        tokio::select! {
            biased;
            () = ingress.terminal_notify.notified() => {
                if ingress.has_preemptive_terminal_pending() {
                    return Err(ActiveInterrupt::TerminalPending);
                }
            }
            () = ingress.closed_notify.notified() => {
                if ingress.has_terminal_pending() {
                    return Err(ActiveInterrupt::TerminalPending);
                }
                return Err(ActiveInterrupt::TransportLost);
            }
            () = tokio::time::sleep_until(absolute_deadline) => {
                return Err(ActiveInterrupt::Deadline);
            }
            () = tokio::time::sleep_until(stall_deadline) => {
                return Err(ActiveInterrupt::Stall);
            }
            output = &mut operation => return Ok(output),
        }
    }
}

async fn next_active(
    events: &mut UploadEvents,
    ingress: &Arc<UploadIngress>,
    absolute_deadline: Instant,
    stall_deadline: Instant,
) -> ActiveInput {
    loop {
        // The prior terminal has now been fully handled. Clear its processing
        // marker under the same sequencing lock used by dequeue and writer
        // commit hooks, preserving an atomic queued -> processing -> handled
        // handoff with no CREDIT-commit gap.
        if !events.has_deferred_end() {
            ingress.clear_processing_terminal();
        }
        // A record already admitted through the shared sequencing lock
        // precedes later socket teardown and timers. If it is still waiting
        // for its separately bounded terminal-lane slot, wait for that
        // admission to enqueue or roll back before considering either.
        if let Some(event) = events.try_recv(ingress) {
            return ActiveInput::Event(event);
        }
        if ingress.has_terminal_pending() {
            tokio::select! {
                biased;
                event = events.recv(ingress) => match event {
                    Some(event) => return ActiveInput::Event(event),
                    None => return ActiveInput::TransportLost,
                },
                () = ingress.terminal_notify.notified() => continue,
            }
        }
        if ingress.closed.load(Ordering::Acquire) {
            return ActiveInput::TransportLost;
        }
        tokio::select! {
            biased;
            event = events.recv(ingress) => match event {
                Some(event) => return ActiveInput::Event(event),
                None => return ActiveInput::TransportLost,
            },
            () = ingress.terminal_notify.notified() => continue,
            () = ingress.closed_notify.notified() => continue,
            () = tokio::time::sleep_until(absolute_deadline) => return ActiveInput::Deadline,
            () = tokio::time::sleep_until(stall_deadline) => return ActiveInput::Stall,
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn finish_client_abort(
    outbound: &Outbound,
    ingress: &Arc<UploadIngress>,
    events: &mut UploadEvents,
    binding: &UploadStreamBinding,
    identity: StreamIdentity,
    state: &ConsumerState,
    bounds: &CodecBounds,
    id: u64,
    request: &UploadRequestLease,
    reason: StreamAbortReason,
    stall: Duration,
    closer: &ConnectionCloser,
    owner: FileShareLedgerOwner,
) -> bool {
    let terminal_error = ApiError::StreamAborted {
        transferred_bytes: state.accepted,
        total: known_total(state.declared),
        reason,
    };
    // ABORT owns the terminal lane and may overtake older DATA waiting in the
    // bounded data lane. Those records are explicitly discarded before ACK;
    // later producer traffic is classified into the terminal lane as a fault.
    events.drain_data(ingress);
    let ack_deadline = Instant::now()
        .checked_add(stall)
        .expect("served stall duration was validated");
    let ack_outcome = send_client_abort_ack_until(
        outbound,
        ingress,
        binding.retirement(),
        identity,
        state.accepted,
        bounds,
        ack_deadline,
    )
    .await;
    let protocol_followup = match ack_outcome {
        ClientAbortAckOutcome::Sent => false,
        ClientAbortAckOutcome::ProtocolFaultBeforeSent => {
            // The first producer ABORT is still owed its ACK. Discharge it,
            // then promote the queued follower into a crossed daemon
            // protocol terminal.
            if send_ack_until(outbound, identity, state.accepted, bounds, ack_deadline).await
                != BoundedControlOutcome::Sent
            {
                binding.retire();
                let _ = owner.complete(Err(ApiError::MalformedFrame));
                closer.malformed();
                return false;
            }
            true
        }
        ClientAbortAckOutcome::ProtocolFaultAfterSent => true,
        ClientAbortAckOutcome::Failed => {
            binding.retire();
            let _ = owner.complete(Err(terminal_error));
            closer.malformed();
            return false;
        }
    };
    if protocol_followup {
        while let Some(event) = events.try_recv(ingress) {
            if let UploadEventBody::Cancel { response } = event.body {
                let _ = response.send(CancelAttempt::Unknown);
            }
        }
        if !ingress.promote_client_abort_protocol() {
            binding.retire();
            let _ = owner.complete(Err(ApiError::MalformedFrame));
            return false;
        }
        return finish_daemon_abort(
            outbound,
            ingress,
            events,
            binding,
            identity,
            state,
            bounds,
            id,
            request,
            UploadFailure::Protocol,
            stall,
            closer,
            owner,
        )
        .await;
    }
    // Publish only after the explicit ACK boundary so a joined replay cannot
    // emit terminal Text ahead of the owner handshake. Every failed ACK path
    // above publishes the same selected result before returning.
    let terminal = owner.complete(Err(terminal_error));
    let replied = send_reply(outbound, id, terminal.as_ref(), request).await;
    if !replied {
        closer.malformed();
    }
    replied
}

#[allow(clippy::too_many_arguments)]
async fn finish_daemon_abort(
    outbound: &Outbound,
    ingress: &Arc<UploadIngress>,
    events: &mut UploadEvents,
    binding: &UploadStreamBinding,
    identity: StreamIdentity,
    state: &ConsumerState,
    bounds: &CodecBounds,
    id: u64,
    request: &UploadRequestLease,
    failure: UploadFailure,
    stall: Duration,
    closer: &ConnectionCloser,
    owner: FileShareLedgerOwner,
) -> bool {
    let terminal_error = failure_error(&failure, state);
    if ingress.phase() != UploadPhase::DaemonAbortQueued {
        let _ = owner.complete(Err(terminal_error));
        return false;
    }
    let abort_deadline = Instant::now()
        .checked_add(stall)
        .expect("served stall duration was validated");
    if send_abort(
        outbound,
        ingress,
        identity,
        state.accepted,
        failure_abort_reason(&failure),
        bounds,
        abort_deadline,
    )
    .await
        != BoundedControlOutcome::Sent
    {
        let _ = owner.complete(Err(terminal_error));
        closer.malformed();
        return false;
    }

    let ack_deadline = Instant::now()
        .checked_add(stall)
        .expect("served stall duration was validated");
    let mut acknowledged = false;
    let mut crossed_abort_acked = false;
    loop {
        let next = tokio::select! {
            biased;
            event = events.recv(ingress) => event,
            () = ingress.closed_notify.notified() => None,
            () = tokio::time::sleep_until(ack_deadline) => None,
        };
        let Some(event) = next else {
            break;
        };
        if event.received_at > ack_deadline {
            break;
        }
        match event.body {
            UploadEventBody::Record { bytes, .. } => {
                match decode_stream_record_view(&bytes, bounds).map(|view| view.body) {
                    Ok(StreamRecordBodyView::Ack { accepted_through })
                        if accepted_through == state.accepted =>
                    {
                        // Routing latched this first ACK as AckPending. Retire
                        // under the registry+sequencing locks only when no
                        // duplicate exact-pair record is already queued.
                        acknowledged = binding.retire_daemon_ack();
                        break;
                    }
                    Ok(StreamRecordBodyView::Abort {
                        accepted_through,
                        reason,
                    }) if matches!(
                        reason,
                        BinaryAbortReason::Cancelled
                            | BinaryAbortReason::SourceFailed
                            | BinaryAbortReason::ProtocolError
                    ) && state.valid_producer_abort(accepted_through) =>
                    {
                        // Crossed ABORT: discharge the producer's explicit ACK
                        // obligation, then continue waiting for ours. The
                        // daemon-selected terminal remains authoritative.
                        if crossed_abort_acked
                            || send_ack_until(
                                outbound,
                                identity,
                                state.accepted,
                                bounds,
                                ack_deadline,
                            )
                            .await
                                != BoundedControlOutcome::Sent
                        {
                            break;
                        }
                        crossed_abort_acked = true;
                    }
                    Ok(StreamRecordBodyView::Data { .. } | StreamRecordBodyView::End { .. }) => {
                        // Older in-flight producer records are drained and
                        // discarded until ACK proves the boundary.
                    }
                    _ => break,
                }
            }
            UploadEventBody::Cancel { response } => {
                let _ = response.send(CancelAttempt::Unknown);
            }
            UploadEventBody::RejectedData(_) | UploadEventBody::Malformed => break,
        }
    }
    if !acknowledged {
        binding.retire();
    }
    // ACK or its bounded timeout is the publication boundary. A joined
    // faithful replay now receives the exact selected result, but never
    // outruns the owner's ABORT/ACK obligations.
    let terminal = owner.complete(Err(terminal_error));
    let replied = send_reply(outbound, id, terminal.as_ref(), request).await;
    if !acknowledged || !replied {
        closer.malformed();
    }
    replied
}

#[allow(clippy::too_many_arguments)]
async fn finalize_detached(
    finalizer: FileShareFinalizer,
    owner: FileShareLedgerOwner,
    outbound: Outbound,
    id: u64,
    request: UploadRequestLease,
    binding: UploadStreamBinding,
) {
    // The exact END produced one opaque, single-use import/event capability.
    // Its private staging object is removed on either result.
    let result = finalizer.finalize().await;
    let result = owner.complete(result);
    let sent = send_reply_and_retire(&outbound, id, result.as_ref(), &request, &binding).await;
    if !sent {
        // Connection teardown may legitimately discard a lost final reply;
        // the ledger already owns the exact replayable result.
        binding.retire();
        request.release();
    }
}

/// Execute one complete consumer-direction `file.share` request.
///
/// The caller should spawn this owner independently of the connection's
/// abort-on-teardown request set. Registry invalidation supplies the pre-END
/// transport-loss signal, while exact-END finalization is moved into its own
/// detached task before this future returns.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_file_share(
    engine: Arc<Engine>,
    request_body: FileShare,
    op_id: Option<OpId>,
    principal_key: String,
    id: u64,
    request_permit: RequestPermit,
    outbound: Outbound,
    registry: StreamRegistry,
    stream_ids: Arc<Mutex<StreamIdGenerator>>,
    transfer_pool: TransferPool,
    limits: RuntimeLimits,
    closer: ConnectionCloser,
    cancellations: UploadCancellationRegistry,
    ingress_budget: UploadIngressBudget,
) -> bool {
    let request = UploadRequestLease::new(request_permit);
    // This detached task must finish the structural/subject/ledger boundary
    // even if its socket disappears concurrently. Otherwise an op_id could
    // vanish before acquiring ownership and a reconnect would incorrectly
    // open a fresh stream instead of replaying transport_lost.
    let gate = engine
        .gate_file_share(&request_body, op_id.clone(), &principal_key)
        .await;
    let owner = match gate {
        FileShareLedgerGate::Replay(result) => {
            return send_reply(&outbound, id, result.as_ref(), &request).await;
        }
        FileShareLedgerGate::Owner(owner) => owner,
    };
    if !registry.is_accepting() {
        let _ = owner.transport_lost(0);
        request.release();
        return false;
    }
    let progress = owner.progress();
    let prepared = match tokio::select! {
        biased;
        () = registry.wait_invalidated() => {
            drop(owner);
            request.release();
            return false;
        }
        prepared = engine.prepare_file_share_after_gate(&request_body) => prepared,
    } {
        Ok(prepared) => prepared,
        Err(error) => {
            let result = owner.complete(Err(error));
            return send_reply(&outbound, id, result.as_ref(), &request).await;
        }
    };
    let file_limit = engine.limits().max_shared_file_bytes;

    // A successful pool reservation is the absolute-deadline start and
    // precedes both staging creation and OPEN.
    let admitted_at = Instant::now();
    let reservation = match transfer_pool.reserve(request_body.declared_bytes) {
        Ok(reservation) => reservation,
        Err(error) => {
            let result = owner.complete(Err(error));
            return send_reply(&outbound, id, result.as_ref(), &request).await;
        }
    };
    let (absolute_deadline, budget_ms) =
        match limits.deadline(admitted_at, request_body.declared_bytes) {
            Ok(deadline) => deadline,
            Err(_) => {
                drop(reservation);
                let result = owner.complete(Err(ApiError::NotReady));
                return send_reply(&outbound, id, result.as_ref(), &request).await;
            }
        };

    let stage = tokio::select! {
        biased;
        () = registry.wait_invalidated() => {
            drop(reservation);
            drop(owner);
            request.release();
            return false;
        }
        () = tokio::time::sleep_until(absolute_deadline) => {
            drop(reservation);
            let result = owner.complete(Err(ApiError::TransferDeadlineExceeded {
                transferred_bytes: 0,
                total: known_total(request_body.declared_bytes),
                budget_ms,
            }));
            return send_reply(&outbound, id, result.as_ref(), &request).await;
        }
        stage = prepared.open_sink() => match stage {
            Ok(stage) => stage,
            Err(_) => {
                drop(reservation);
                let result = owner.complete(Err(ApiError::StreamAborted {
                    transferred_bytes: 0,
                    total: known_total(request_body.declared_bytes),
                    reason: StreamAbortReason::SinkFailed,
                }));
                return send_reply(&outbound, id, result.as_ref(), &request).await;
            }
        }
    };

    let identity = {
        let mut generator = stream_ids.lock().expect("stream id generator poisoned");
        generator.next(id)
    };
    let identity = match identity {
        Ok(identity) => identity,
        Err(_) => {
            drop(stage);
            drop(reservation);
            let result = owner.complete(Err(ApiError::NotReady));
            return send_reply(&outbound, id, result.as_ref(), &request).await;
        }
    };
    let (ingress, mut events) =
        UploadIngress::new(ingress_budget, request_body.declared_bytes, file_limit);
    let Some(binding) = registry.bind_upload(identity, ingress.clone()) else {
        drop(stage);
        drop(reservation);
        let result = owner.complete(Err(ApiError::NotReady));
        return send_reply(&outbound, id, result.as_ref(), &request).await;
    };
    let bounds = CodecBounds {
        max_frame_bytes: limits.max_frame_bytes(),
        ..CodecBounds::default()
    };
    let cancellation_slot = Arc::new(Mutex::new(None));
    let install_slot = cancellation_slot.clone();
    let install_cancellations = cancellations.clone();
    let install_ingress = ingress.clone();
    let install_principal = principal_key;
    let install_op_id = op_id;
    let declared_total = request_body.declared_bytes;
    match send_open(
        &outbound,
        &ingress,
        identity,
        request_body.declared_bytes,
        &bounds,
        absolute_deadline,
        move || {
            let Some(op_id) = install_op_id else {
                return true;
            };
            let Some(guard) = install_cancellations.register(
                install_principal,
                op_id,
                install_ingress,
                declared_total,
            ) else {
                return false;
            };
            *install_slot
                .lock()
                .expect("upload cancellation slot poisoned") = Some(guard);
            true
        },
    )
    .await
    {
        BoundedControlOutcome::Sent => {}
        _ if ingress.opening_fatal.load(Ordering::Acquire) => {
            drop(stage);
            drop(reservation);
            drop(binding);
            drop(owner);
            closer.malformed();
            return false;
        }
        BoundedControlOutcome::Deadline => {
            drop(stage);
            drop(reservation);
            binding.retire();
            let result = owner.complete(Err(ApiError::TransferDeadlineExceeded {
                transferred_bytes: 0,
                total: known_total(request_body.declared_bytes),
                budget_ms,
            }));
            return send_reply(&outbound, id, result.as_ref(), &request).await;
        }
        BoundedControlOutcome::DiscardedOrClosed => {
            drop(stage);
            drop(reservation);
            drop(binding);
            drop(owner);
            closer.malformed();
            return false;
        }
    }

    let mut cancellation_guard = cancellation_slot
        .lock()
        .expect("upload cancellation slot poisoned")
        .take();
    let mut reservation = Some(reservation);
    let mut stage = Some(stage);
    let mut owner = Some(owner);
    let mut pending_cancel_response = None;
    let mut state = ConsumerState::new(request_body.declared_bytes, file_limit);
    let window = u64::try_from(limits.max_data_payload_bytes()).unwrap_or(u64::MAX);
    let mut stall_deadline = limits
        .stall_deadline(Instant::now())
        .expect("served stall duration was validated");

    let initial_credit = issue_credit(
        &outbound,
        &ingress,
        identity,
        &mut state,
        window,
        &bounds,
        absolute_deadline,
        stall_deadline,
        budget_ms,
    )
    .await;
    if matches!(initial_credit, Ok(CreditOutcome::TransportLost)) {
        drop(stage.take());
        drop(reservation.take());
        drop(cancellation_guard.take());
        binding.retire();
        let _ = owner
            .take()
            .expect("upload owns ledger completion")
            .transport_lost(state.accepted);
        request.release();
        return false;
    }
    if let Err(failure) = initial_credit {
        match ingress.select_daemon_abort(TerminalYield::Any) {
            DaemonAbortSelection::TerminalPending => {}
            DaemonAbortSelection::Lost => {
                drop(stage.take());
                drop(reservation.take());
                drop(cancellation_guard.take());
                let _ = owner
                    .take()
                    .expect("upload owns ledger completion")
                    .transport_lost(state.accepted);
                request.release();
                return false;
            }
            DaemonAbortSelection::Selected => {
                drop(stage.take());
                drop(reservation.take());
                drop(cancellation_guard.take());
                return finish_daemon_abort(
                    &outbound,
                    &ingress,
                    &mut events,
                    &binding,
                    identity,
                    &state,
                    &bounds,
                    id,
                    &request,
                    failure,
                    limits.transfer_stall(),
                    &closer,
                    owner.take().expect("upload owns ledger completion"),
                )
                .await;
            }
        }
    }

    loop {
        let input = next_active(&mut events, &ingress, absolute_deadline, stall_deadline).await;
        let mut terminal_yield = if matches!(&input, ActiveInput::Deadline | ActiveInput::Stall) {
            TerminalYield::Any
        } else {
            TerminalYield::None
        };
        let failure = match input {
            ActiveInput::Deadline => Some(UploadFailure::Deadline { budget_ms }),
            ActiveInput::Stall => Some(UploadFailure::Stall),
            ActiveInput::TransportLost => {
                drop(stage.take());
                drop(reservation.take());
                drop(cancellation_guard.take());
                binding.retire();
                let _ = owner
                    .take()
                    .expect("upload owns ledger completion")
                    .transport_lost(state.accepted);
                request.release();
                return false;
            }
            ActiveInput::Event(event) => {
                // Timestamped input never gets to jump an already-expired
                // absolute deadline or stall boundary merely because the
                // actor was descheduled. An explicitly sequenced
                // transfer.cancel and a semantically valid producer ABORT win
                // exact equality. Invalid ABORT, END, and ordinary wire input
                // lose ties to deadline, then stall.
                if let Some(failure) = active_event_timer_failure(
                    &event,
                    &state,
                    &bounds,
                    absolute_deadline,
                    stall_deadline,
                    budget_ms,
                ) {
                    Some(failure)
                } else {
                    let credit_at_receipt = event
                        ._pending_data
                        .as_ref()
                        .map(|pending| pending.credit_at_receipt);
                    match event.body {
                        UploadEventBody::Malformed => Some(UploadFailure::Protocol),
                        UploadEventBody::RejectedData(DataFailure::Protocol) => {
                            Some(UploadFailure::Protocol)
                        }
                        UploadEventBody::RejectedData(DataFailure::TooLarge { observed }) => {
                            let _ = observed;
                            Some(UploadFailure::TooLarge)
                        }
                        UploadEventBody::RejectedData(DataFailure::Declared { observed }) => {
                            Some(UploadFailure::Declared { observed })
                        }
                        UploadEventBody::Cancel { response } => {
                            let selected = cancellation_guard.as_ref().is_some_and(|guard| {
                                guard.select_cancelled(
                                    owner
                                        .as_mut()
                                        .expect("active upload owns ledger completion"),
                                    state.accepted,
                                    state.declared,
                                )
                            });
                            if selected {
                                pending_cancel_response = Some(response);
                                Some(UploadFailure::Cancelled)
                            } else {
                                let _ = response.send(CancelAttempt::Unknown);
                                None
                            }
                        }
                        UploadEventBody::Record { kind, bytes, .. } => {
                            let view = match decode_stream_record_view(&bytes, &bounds) {
                                Ok(view) => view,
                                Err(_) => {
                                    drop(bytes);
                                    if kind == StreamRecordKind::Data {
                                        // Permit is retained by the event until
                                        // this whole match arm leaves.
                                    }
                                    return finish_daemon_abort(
                                        &outbound,
                                        &ingress,
                                        &mut events,
                                        &binding,
                                        identity,
                                        &state,
                                        &bounds,
                                        id,
                                        &request,
                                        UploadFailure::Protocol,
                                        limits.transfer_stall(),
                                        &closer,
                                        owner.take().expect("upload owns ledger completion"),
                                    )
                                    .await;
                                }
                            };
                            match view.body {
                                StreamRecordBodyView::Data { offset, payload } => {
                                    let Some(credit_at_receipt) = credit_at_receipt else {
                                        return finish_daemon_abort(
                                            &outbound,
                                            &ingress,
                                            &mut events,
                                            &binding,
                                            identity,
                                            &state,
                                            &bounds,
                                            id,
                                            &request,
                                            UploadFailure::Protocol,
                                            limits.transfer_stall(),
                                            &closer,
                                            owner.take().expect("upload owns ledger completion"),
                                        )
                                        .await;
                                    };
                                    match state.validate_data_at_credit(
                                        offset,
                                        payload.len(),
                                        credit_at_receipt,
                                    ) {
                                        Err(DataFailure::Protocol) => {
                                            terminal_yield = TerminalYield::Preemptive;
                                            Some(UploadFailure::Protocol)
                                        }
                                        Err(DataFailure::TooLarge { observed }) => {
                                            let _ = observed;
                                            terminal_yield = TerminalYield::Preemptive;
                                            Some(UploadFailure::TooLarge)
                                        }
                                        Err(DataFailure::Declared { observed }) => {
                                            terminal_yield = TerminalYield::Preemptive;
                                            Some(UploadFailure::Declared { observed })
                                        }
                                        Ok(candidate) => {
                                            let accepted = await_active_operation(
                                                stage
                                                    .as_mut()
                                                    .expect("active upload owns staging")
                                                    .accept(offset, payload),
                                                &ingress,
                                                absolute_deadline,
                                                stall_deadline,
                                            )
                                            .await;
                                            match accepted {
                                                Err(ActiveInterrupt::TerminalPending) => None,
                                                Err(ActiveInterrupt::Deadline) => {
                                                    terminal_yield = TerminalYield::Any;
                                                    Some(UploadFailure::Deadline { budget_ms })
                                                }
                                                Err(ActiveInterrupt::Stall) => {
                                                    terminal_yield = TerminalYield::Any;
                                                    Some(UploadFailure::Stall)
                                                }
                                                Err(ActiveInterrupt::TransportLost) => {
                                                    drop(stage.take());
                                                    drop(reservation.take());
                                                    drop(cancellation_guard.take());
                                                    binding.retire();
                                                    let _ = owner
                                                        .take()
                                                        .expect("upload owns ledger completion")
                                                        .transport_lost(state.accepted);
                                                    request.release();
                                                    return false;
                                                }
                                                Ok(accepted) => match accepted {
                                                    Ok(accepted) if accepted == candidate => {
                                                        match ingress.commit_accepted(
                                                            candidate,
                                                            absolute_deadline,
                                                            stall_deadline,
                                                        ) {
                                                            Ok(()) => {
                                                                state.data_accepted(candidate);
                                                                progress.record_accepted(candidate);
                                                                stall_deadline = limits
                                                                    .stall_deadline(Instant::now())
                                                                    .expect(
                                                                        "served stall duration was validated",
                                                                    );
                                                                None
                                                            }
                                                            Err(
                                                                ActiveInterrupt::TerminalPending,
                                                            ) => None,
                                                            Err(ActiveInterrupt::Deadline) => {
                                                                terminal_yield = TerminalYield::Any;
                                                                Some(UploadFailure::Deadline {
                                                                    budget_ms,
                                                                })
                                                            }
                                                            Err(ActiveInterrupt::Stall) => {
                                                                terminal_yield = TerminalYield::Any;
                                                                Some(UploadFailure::Stall)
                                                            }
                                                            Err(ActiveInterrupt::TransportLost) => {
                                                                drop(stage.take());
                                                                drop(reservation.take());
                                                                drop(cancellation_guard.take());
                                                                binding.retire();
                                                                let _ = owner
                                                                    .take()
                                                                    .expect(
                                                                        "upload owns ledger completion",
                                                                    )
                                                                    .transport_lost(state.accepted);
                                                                request.release();
                                                                return false;
                                                            }
                                                        }
                                                    }
                                                    Ok(_) | Err(_) => {
                                                        terminal_yield = TerminalYield::Preemptive;
                                                        Some(UploadFailure::Sink)
                                                    }
                                                },
                                            }
                                        }
                                    }
                                }
                                StreamRecordBodyView::End { total } => {
                                    let end_failure = match state.validate_end(total) {
                                        Ok(()) => None,
                                        Err(DataFailure::Protocol) => Some(UploadFailure::Protocol),
                                        Err(DataFailure::Declared { observed }) => {
                                            Some(UploadFailure::Declared { observed })
                                        }
                                        Err(DataFailure::TooLarge { .. }) => unreachable!(
                                            "END validation does not apply aggregate DATA policy"
                                        ),
                                    };
                                    if let Some(failure) = end_failure {
                                        Some(failure)
                                    } else if !ingress.commit_finalizing(state.accepted) {
                                        Some(UploadFailure::Protocol)
                                    } else {
                                        // A follower classified after this END
                                        // but before the actor consumed it may
                                        // already be waiting in the bounded
                                        // ingress lane. END still owns the
                                        // finalization result, while that
                                        // duplicate/late record deterministically
                                        // closes the connection with 4007.
                                        if ingress.post_end_fatal.load(Ordering::Acquire) {
                                            closer.malformed();
                                        }
                                        drop(reservation.take());
                                        drop(cancellation_guard.take());
                                        let stage = match stage.take() {
                                            Some(stage) => stage,
                                            None => return false,
                                        };
                                        let finalization = match stage.finish(total).await {
                                            Ok(stage) => stage,
                                            Err(error) => {
                                                let result = owner
                                                    .take()
                                                    .expect("upload owns ledger completion")
                                                    .complete(Err(final_sink_error(error, &state)));
                                                let sent = send_reply_and_retire(
                                                    &outbound,
                                                    id,
                                                    result.as_ref(),
                                                    &request,
                                                    &binding,
                                                )
                                                .await;
                                                if !sent {
                                                    binding.retire();
                                                    request.release();
                                                }
                                                return sent;
                                            }
                                        };
                                        let outbound = outbound.clone();
                                        let request = request.clone();
                                        let owner =
                                            owner.take().expect("upload owns ledger completion");
                                        tokio::spawn(async move {
                                            finalize_detached(
                                                finalization,
                                                owner,
                                                outbound,
                                                id,
                                                request,
                                                binding,
                                            )
                                            .await;
                                        });
                                        return true;
                                    }
                                }
                                StreamRecordBodyView::Abort {
                                    accepted_through,
                                    reason,
                                } => {
                                    if !state.valid_producer_abort(accepted_through) {
                                        Some(UploadFailure::Protocol)
                                    } else {
                                        if !ingress.try_select_client_abort() {
                                            return false;
                                        }
                                        drop(stage.take());
                                        drop(reservation.take());
                                        drop(cancellation_guard.take());
                                        let reason = match reason {
                                            BinaryAbortReason::Cancelled => {
                                                StreamAbortReason::Cancelled
                                            }
                                            BinaryAbortReason::SourceFailed => {
                                                StreamAbortReason::SourceFailed
                                            }
                                            BinaryAbortReason::ProtocolError => {
                                                StreamAbortReason::ProtocolError
                                            }
                                            BinaryAbortReason::SinkFailed
                                            | BinaryAbortReason::OperationError => {
                                                return finish_daemon_abort(
                                                    &outbound,
                                                    &ingress,
                                                    &mut events,
                                                    &binding,
                                                    identity,
                                                    &state,
                                                    &bounds,
                                                    id,
                                                    &request,
                                                    UploadFailure::Protocol,
                                                    limits.transfer_stall(),
                                                    &closer,
                                                    owner
                                                        .take()
                                                        .expect("upload owns ledger completion"),
                                                )
                                                .await;
                                            }
                                        };
                                        return finish_client_abort(
                                            &outbound,
                                            &ingress,
                                            &mut events,
                                            &binding,
                                            identity,
                                            &state,
                                            &bounds,
                                            id,
                                            &request,
                                            reason,
                                            limits.transfer_stall(),
                                            &closer,
                                            owner.take().expect("upload owns ledger completion"),
                                        )
                                        .await;
                                    }
                                }
                                StreamRecordBodyView::Open { .. }
                                | StreamRecordBodyView::Credit { .. }
                                | StreamRecordBodyView::Ack { .. } => Some(UploadFailure::Protocol),
                            }
                        }
                    }
                }
            }
        };

        if let Some(failure) = failure {
            match ingress.select_daemon_abort(terminal_yield) {
                DaemonAbortSelection::TerminalPending => continue,
                DaemonAbortSelection::Lost => {
                    drop(stage.take());
                    drop(reservation.take());
                    drop(cancellation_guard.take());
                    let _ = owner
                        .take()
                        .expect("upload owns ledger completion")
                        .transport_lost(state.accepted);
                    request.release();
                    return false;
                }
                DaemonAbortSelection::Selected => {}
            }
            drop(stage.take());
            drop(reservation.take());
            // Selection already moved cancellation provenance into the
            // canonical ledger; every terminal drops only this exact live
            // registry guard.
            drop(cancellation_guard.take());
            if let Some(response) = pending_cancel_response.take() {
                let _ = response.send(CancelAttempt::Cancelled {
                    accepted: state.accepted,
                });
            }
            return finish_daemon_abort(
                &outbound,
                &ingress,
                &mut events,
                &binding,
                identity,
                &state,
                &bounds,
                id,
                &request,
                failure,
                limits.transfer_stall(),
                &closer,
                owner.take().expect("upload owns ledger completion"),
            )
            .await;
        }

        // DATA success is handled separately because the borrowed payload
        // cannot outlive its owned message. This fallback indicates a
        // cancel request that lost without selecting a terminal.
        if state.should_credit() && state.credited_accepted != state.accepted {
            let credit = issue_credit(
                &outbound,
                &ingress,
                identity,
                &mut state,
                window,
                &bounds,
                absolute_deadline,
                stall_deadline,
                budget_ms,
            )
            .await;
            if matches!(credit, Ok(CreditOutcome::TransportLost)) {
                drop(stage.take());
                drop(reservation.take());
                drop(cancellation_guard.take());
                binding.retire();
                let _ = owner
                    .take()
                    .expect("upload owns ledger completion")
                    .transport_lost(state.accepted);
                request.release();
                return false;
            }
            if let Err(failure) = credit {
                match ingress.select_daemon_abort(TerminalYield::Any) {
                    DaemonAbortSelection::TerminalPending => continue,
                    DaemonAbortSelection::Lost => {
                        drop(stage.take());
                        drop(reservation.take());
                        drop(cancellation_guard.take());
                        let _ = owner
                            .take()
                            .expect("upload owns ledger completion")
                            .transport_lost(state.accepted);
                        request.release();
                        return false;
                    }
                    DaemonAbortSelection::Selected => {}
                }
                drop(stage.take());
                drop(reservation.take());
                drop(cancellation_guard.take());
                return finish_daemon_abort(
                    &outbound,
                    &ingress,
                    &mut events,
                    &binding,
                    identity,
                    &state,
                    &bounds,
                    id,
                    &request,
                    failure,
                    limits.transfer_stall(),
                    &closer,
                    owner.take().expect("upload owns ledger completion"),
                )
                .await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::future::pending;

    use jeliya_codec::STREAM_HEADER_BYTES;
    use tempfile::TempDir;
    use tokio::time::timeout;

    use super::*;

    const REQUEST_ID: u64 = 41;
    const STREAM_ID: u128 = 0x0102_0304_0506_0708_1112_1314_1516_1718;

    /// Spec-authored `JBS2` record constructor. These tests deliberately do
    /// not obtain expected wire bytes from the implementation encoder.
    fn wire(
        kind: u8,
        request_id: u64,
        stream_id: u128,
        offset: u64,
        value: u64,
        payload: &[u8],
    ) -> Bytes {
        let mut bytes = Vec::with_capacity(STREAM_HEADER_BYTES + payload.len());
        bytes.extend_from_slice(b"JBS2");
        bytes.push(kind);
        bytes.extend_from_slice(&[0, 0, 0]);
        bytes.extend_from_slice(&request_id.to_be_bytes());
        bytes.extend_from_slice(&stream_id.to_be_bytes());
        bytes.extend_from_slice(&offset.to_be_bytes());
        bytes.extend_from_slice(&value.to_be_bytes());
        bytes.extend_from_slice(payload);
        Bytes::from(bytes)
    }

    fn bounds(max_frame_bytes: usize) -> CodecBounds {
        CodecBounds {
            max_frame_bytes,
            ..CodecBounds::default()
        }
    }

    fn budget(bytes: usize) -> UploadIngressBudget {
        UploadIngressBudget {
            bytes: Arc::new(Semaphore::new(bytes)),
            data_messages: UPLOAD_DATA_EVENT_SLOTS,
        }
    }

    fn active_ingress(bytes: usize) -> (Arc<UploadIngress>, UploadEvents) {
        let (ingress, events) = UploadIngress::new(budget(bytes), u64::MAX, u64::MAX);
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(1))
            .expect("test deadline is finite");
        assert_eq!(
            ingress.commit_open_with(deadline, || true),
            ControlCommit::Committed
        );
        ingress
            .served_send_through
            .store(u64::MAX, Ordering::Release);
        (ingress, events)
    }

    async fn next_event(events: &mut UploadEvents, ingress: &UploadIngress) -> UploadEvent {
        timeout(Duration::from_secs(1), events.recv(ingress))
            .await
            .expect("upload event timed out")
            .expect("upload ingress remained open")
    }

    fn cancel_request(value: &str) -> TransferCancel {
        TransferCancel {
            transfer_op_id: OpId::new(value),
        }
    }

    async fn cancellation_ledger(
        op_id: &OpId,
        principal: &str,
        declared_bytes: u64,
    ) -> (
        TempDir,
        Arc<Engine>,
        FileShareLedgerOwner,
        UploadCancellationRegistry,
    ) {
        let dir = TempDir::new().expect("cancellation ledger tempdir");
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
        .expect("cancellation ledger engine");
        engine
            .execute(jeliya_core::typed::TypedCall::SubjectEnsure(
                jeliya_api::SubjectEnsure {},
            ))
            .await
            .reply
            .expect("subject.ensure");
        let request = FileShare {
            room_id: jeliya_api::RoomId::new("room-for-cancellation-ledger"),
            name: "cancelled.bin".into(),
            declared_bytes,
            declared_content_type: "application/octet-stream".into(),
        };
        let FileShareLedgerGate::Owner(owner) = engine
            .gate_file_share(&request, Some(op_id.clone()), principal)
            .await
        else {
            panic!("fresh cancellation ledger key must be owned");
        };
        let registry = UploadCancellationRegistry::with_engine(&engine);
        (dir, engine, owner, registry)
    }

    fn assert_transfer_unknown(result: Result<TransferCancelOut, ApiError>, op_id: &OpId) {
        assert!(matches!(
            result,
            Err(ApiError::TransferUnknown { transfer_op_id })
                if transfer_op_id == *op_id
        ));
    }

    #[test]
    fn cumulative_credit_holds_the_sentinel_until_declared_bytes_are_accepted() {
        let mut zero = ConsumerState::new(0, 8);
        assert_eq!(zero.next_credit(4), Some((0, 1)));
        zero.record_credit(0, 1).unwrap();
        assert_eq!(zero.credit_history.len(), 1);

        let mut one = ConsumerState::new(1, 8);
        assert_eq!(one.next_credit(64), Some((0, 1)));
        one.record_credit(0, 1).unwrap();
        let candidate = one.validate_data(0, 1).unwrap();
        one.data_accepted(candidate);
        assert_eq!(one.next_credit(64), Some((1, 2)));

        let mut ordinary = ConsumerState::new(5, 8);
        assert_eq!(ordinary.next_credit(3), Some((0, 3)));
        ordinary.record_credit(0, 3).unwrap();
        ordinary.record_credit(0, 3).unwrap();
        assert_eq!(ordinary.credit_history.len(), 1, "repeat CREDIT is bounded");
        let candidate = ordinary.validate_data(0, 3).unwrap();
        ordinary.data_accepted(candidate);
        assert!(ordinary.should_credit());
        assert_eq!(ordinary.next_credit(3), Some((3, 5)));
        ordinary.record_credit(3, 5).unwrap();
        let candidate = ordinary.validate_data(3, 2).unwrap();
        ordinary.data_accepted(candidate);
        assert_eq!(ordinary.next_credit(3), Some((5, 6)));

        let mut maximum = ConsumerState::new(8, 8);
        assert_eq!(maximum.next_credit(8), Some((0, 8)));
        maximum.record_credit(0, 8).unwrap();
        let candidate = maximum.validate_data(0, 8).unwrap();
        maximum.data_accepted(candidate);
        assert_eq!(maximum.next_credit(8), Some((8, 9)));
    }

    #[test]
    fn data_refusal_order_is_policy_then_declaration_then_continuity_and_credit() {
        let mut aggregate_first = ConsumerState::new(3, 2);
        assert_eq!(
            aggregate_first.validate_data(0, 4),
            Err(DataFailure::TooLarge { observed: 4 }),
            "aggregate policy precedes declaration and absent credit"
        );
        assert_eq!(aggregate_first.accepted, 0);

        let mut declaration_second = ConsumerState::new(3, 10);
        assert_eq!(
            declaration_second.validate_data(4, 1),
            Err(DataFailure::Declared { observed: 5 }),
            "declaration precedes both wrong continuity and absent credit"
        );
        assert_eq!(declaration_second.accepted, 0);

        let mut exact = ConsumerState::new(4, 4);
        exact.record_credit(0, 4).unwrap();
        let candidate = exact.validate_data(0, 4).unwrap();
        exact.data_accepted(candidate);
        exact.record_credit(4, 5).unwrap();
        assert_eq!(
            exact.validate_data(4, 1),
            Err(DataFailure::TooLarge { observed: 5 })
        );
        assert_eq!(exact.accepted, 4, "one-past rejection is atomic");

        let mut no_credit = ConsumerState::new(10, 10);
        assert_eq!(no_credit.validate_data(1, 1), Err(DataFailure::Protocol));
        no_credit.record_credit(0, 2).unwrap();
        assert_eq!(no_credit.validate_data(0, 3), Err(DataFailure::Protocol));
        assert_eq!(no_credit.accepted, 0);

        let mut overflow = ConsumerState::new(u64::MAX, u64::MAX);
        assert_eq!(
            overflow.validate_data(u64::MAX, 1),
            Err(DataFailure::Protocol)
        );
        assert_eq!(overflow.probe_limit(), None);
    }

    #[test]
    fn stage_stream_too_large_reports_the_original_declaration() {
        let state = ConsumerState::new(2, 1);
        assert_eq!(
            failure_error(&UploadFailure::TooLarge, &state),
            ApiError::FileTooLarge {
                declared_bytes: 2,
                limit_bytes: 1,
                enforced_at: EnforcedAt::StageStream,
            }
        );
    }

    #[test]
    fn finish_time_count_disagreement_is_an_exact_declaration_error() {
        let state = ConsumerState::new(7, 8);
        assert_eq!(
            final_sink_error(
                FileShareSinkError::CountDisagreement {
                    expected: 7,
                    observed: 6,
                },
                &state,
            ),
            ApiError::DeclaredSizeMismatch {
                declared_bytes: 7,
                observed_bytes: 6,
            }
        );
    }

    #[test]
    fn end_distinguishes_exact_below_and_noncontiguous_totals() {
        let mut exact = ConsumerState::new(4, 8);
        exact.data_accepted(4);
        assert_eq!(
            exact.validate_end(4),
            Err(DataFailure::Protocol),
            "END cannot precede writer-committed final accepted/sentinel CREDIT"
        );
        exact.record_credit(4, 5).unwrap();
        assert_eq!(exact.validate_end(4), Ok(()));

        let mut zero = ConsumerState::new(0, 8);
        assert_eq!(zero.validate_end(0), Err(DataFailure::Protocol));
        zero.record_credit(0, 1).unwrap();
        assert_eq!(zero.validate_end(0), Ok(()));

        let mut below = ConsumerState::new(4, 8);
        below.data_accepted(2);
        assert_eq!(
            below.validate_end(2),
            Err(DataFailure::Declared { observed: 2 })
        );
        assert_eq!(
            below.validate_end(4),
            Err(DataFailure::Protocol),
            "END at the declaration is noncontiguous while only two bytes are accepted"
        );
        assert_eq!(below.validate_end(3), Err(DataFailure::Protocol));
    }

    #[tokio::test]
    async fn exact_pair_routing_is_size_first_directional_and_request_local() {
        let limits = bounds(64);
        let registry = StreamRegistry::new();
        let identity = StreamIdentity::new(REQUEST_ID, STREAM_ID).unwrap();
        let other_identity = StreamIdentity::new(REQUEST_ID + 1, STREAM_ID + 1).unwrap();
        let shared_budget = budget(512);
        let (one, mut one_events) = UploadIngress::new(shared_budget.clone(), u64::MAX, u64::MAX);
        let (two, mut two_events) = UploadIngress::new(shared_budget, u64::MAX, u64::MAX);
        let deadline = Instant::now() + Duration::from_secs(1);
        assert_eq!(
            one.commit_open_with(deadline, || true),
            ControlCommit::Committed
        );
        assert_eq!(
            two.commit_open_with(deadline, || true),
            ControlCommit::Committed
        );
        one.served_send_through.store(u64::MAX, Ordering::Release);
        two.served_send_through.store(u64::MAX, Ordering::Release);
        let _one_binding = registry.bind_upload(identity, one.clone()).unwrap();
        let _two_binding = registry.bind_upload(other_identity, two.clone()).unwrap();

        let valid = wire(0x02, REQUEST_ID, STREAM_ID, 0, 0, &[0xaa]);
        assert_eq!(
            registry.route_binary_message(valid, &limits).await,
            BinaryRoute::Delivered
        );
        assert!(matches!(
            next_event(&mut one_events, &one).await.body,
            UploadEventBody::Record {
                kind: StreamRecordKind::Data,
                ..
            }
        ));

        assert_eq!(
            registry
                .route_binary_message(wire(0x02, REQUEST_ID + 1, STREAM_ID, 0, 0, &[1]), &limits,)
                .await,
            BinaryRoute::CloseMalformed
        );
        assert_eq!(
            registry
                .route_binary_message(wire(0x02, REQUEST_ID, STREAM_ID + 1, 0, 0, &[1]), &limits,)
                .await,
            BinaryRoute::CloseMalformed
        );

        let mut bound_bad_reserved = wire(0x02, REQUEST_ID, STREAM_ID, 0, 0, &[1]).to_vec();
        bound_bad_reserved[5] = 1;
        assert_eq!(
            registry
                .route_binary_message(Bytes::from(bound_bad_reserved), &limits)
                .await,
            BinaryRoute::Delivered
        );
        assert!(matches!(
            next_event(&mut one_events, &one).await.body,
            UploadEventBody::Malformed
        ));

        assert_eq!(
            registry
                .route_binary_message(
                    wire(0x02, REQUEST_ID + 1, STREAM_ID + 1, 0, 0, &[2]),
                    &limits,
                )
                .await,
            BinaryRoute::Delivered
        );
        assert!(matches!(
            next_event(&mut two_events, &two).await.body,
            UploadEventBody::Record {
                kind: StreamRecordKind::Data,
                ..
            }
        ));

        assert_eq!(
            registry
                .route_binary_message(
                    wire(0x03, REQUEST_ID + 1, STREAM_ID + 1, 0, 1, &[]),
                    &limits,
                )
                .await,
            BinaryRoute::Delivered
        );
        assert!(matches!(
            next_event(&mut two_events, &two).await.body,
            UploadEventBody::Malformed
        ));

        let mut unbound_bad_reserved = wire(0x02, 99, 99, 0, 0, &[1]).to_vec();
        unbound_bad_reserved[5] = 1;
        assert_eq!(
            registry
                .route_binary_message(Bytes::from(unbound_bad_reserved), &limits)
                .await,
            BinaryRoute::CloseMalformed
        );
        assert_eq!(
            registry
                .route_binary_message(Bytes::from(vec![0; 65]), &limits)
                .await,
            BinaryRoute::CloseTooLarge,
            "complete-message size precedes header and content"
        );
        assert_eq!(
            registry
                .route_binary_message(Bytes::from_static(b"JBS2"), &limits)
                .await,
            BinaryRoute::CloseMalformed
        );
        let mut bad_magic = wire(0x02, REQUEST_ID, STREAM_ID, 0, 0, &[1]).to_vec();
        bad_magic[0] = b'X';
        assert_eq!(
            registry
                .route_binary_message(Bytes::from(bad_magic), &limits)
                .await,
            BinaryRoute::CloseMalformed
        );
    }

    #[tokio::test]
    async fn upload_producer_abort_reasons_are_endpoint_scoped() {
        let limits = bounds(64);
        for reason in [
            BinaryAbortReason::Cancelled,
            BinaryAbortReason::SourceFailed,
            BinaryAbortReason::ProtocolError,
        ] {
            let (ingress, mut events) = active_ingress(128);
            assert_eq!(
                ingress
                    .route_bound(
                        wire(0x05, REQUEST_ID, STREAM_ID, 0, reason as u64, &[]),
                        &limits,
                    )
                    .await,
                BinaryRoute::Delivered
            );
            let UploadEventBody::Record { bytes, .. } =
                next_event(&mut events, &ingress).await.body
            else {
                panic!("valid producer ABORT was not routed as a record");
            };
            assert!(matches!(
                decode_stream_record_view(&bytes, &limits).unwrap().body,
                StreamRecordBodyView::Abort { reason: observed, .. } if observed == reason
            ));
        }

        for daemon_only in [
            BinaryAbortReason::SinkFailed,
            BinaryAbortReason::OperationError,
        ] {
            let (ingress, mut events) = active_ingress(128);
            assert_eq!(
                ingress
                    .route_bound(
                        wire(0x05, REQUEST_ID, STREAM_ID, 0, daemon_only as u64, &[]),
                        &limits,
                    )
                    .await,
                BinaryRoute::Delivered
            );
            assert!(matches!(
                next_event(&mut events, &ingress).await.body,
                UploadEventBody::Malformed
            ));
        }

        let (ingress, mut events) = active_ingress(128);
        assert_eq!(
            ingress
                .route_bound(wire(0x06, REQUEST_ID, STREAM_ID, 0, 5, &[]), &limits)
                .await,
            BinaryRoute::Delivered
        );
        assert!(matches!(
            next_event(&mut events, &ingress).await.body,
            UploadEventBody::Malformed
        ));
    }

    #[tokio::test]
    async fn first_valid_end_wins_finalizing_and_late_records_cannot_rewrite_it() {
        let limits = bounds(64);
        let (ingress, mut events) = active_ingress(128);
        let end = wire(0x04, REQUEST_ID, STREAM_ID, 0, 0, &[]);
        assert_eq!(
            ingress.route_bound(end.clone(), &limits).await,
            BinaryRoute::Delivered
        );
        assert_eq!(
            ingress.route_bound(end, &limits).await,
            BinaryRoute::Delivered
        );
        assert!(matches!(
            next_event(&mut events, &ingress).await.body,
            UploadEventBody::Record {
                kind: StreamRecordKind::End,
                ..
            }
        ));
        assert!(ingress.commit_finalizing(0));
        assert!(matches!(
            next_event(&mut events, &ingress).await.body,
            UploadEventBody::Malformed
        ));
        assert_eq!(ingress.phase(), UploadPhase::Finalizing);
        assert!(!ingress.try_select_daemon_abort());

        let (ingress, mut events) = active_ingress(128);
        assert_eq!(
            ingress
                .route_bound(wire(0x04, REQUEST_ID, STREAM_ID, 0, 0, &[]), &limits,)
                .await,
            BinaryRoute::Delivered
        );
        drop(next_event(&mut events, &ingress).await);
        assert!(ingress.commit_finalizing(0));
        assert!(!ingress.try_select_daemon_abort());
        assert!(!ingress.try_select_client_abort());
        assert_eq!(
            ingress
                .route_bound(wire(0x05, REQUEST_ID, STREAM_ID, 0, 1, &[]), &limits,)
                .await,
            BinaryRoute::Delivered,
            "valid late producer ABORT cannot change accepted END"
        );
        assert_eq!(
            ingress
                .route_bound(wire(0x05, REQUEST_ID, STREAM_ID, 1, 1, &[]), &limits,)
                .await,
            BinaryRoute::CloseMalformed,
            "late FINALIZING ABORT must echo the final accepted count"
        );
        assert_eq!(
            ingress
                .route_bound(wire(0x02, REQUEST_ID, STREAM_ID, 0, 0, &[1]), &limits)
                .await,
            BinaryRoute::CloseMalformed
        );
        assert_eq!(ingress.request_cancel().await, CancelAttempt::Unknown);

        let (invalid_end, mut invalid_events) = active_ingress(128);
        assert_eq!(
            invalid_end
                .route_bound(wire(0x04, REQUEST_ID, STREAM_ID, 0, 0, &[]), &limits)
                .await,
            BinaryRoute::Delivered
        );
        assert_eq!(
            invalid_end
                .route_bound(wire(0x05, REQUEST_ID, STREAM_ID, 0, 1, &[]), &limits)
                .await,
            BinaryRoute::Delivered
        );
        assert!(matches!(
            next_event(&mut invalid_events, &invalid_end).await.body,
            UploadEventBody::Record {
                kind: StreamRecordKind::End,
                ..
            }
        ));
        let invalid_state = ConsumerState::new(1, 8);
        assert_eq!(
            invalid_state.validate_end(0),
            Err(DataFailure::Declared { observed: 0 })
        );
        assert!(invalid_end.try_select_daemon_abort());
        assert!(matches!(
            next_event(&mut invalid_events, &invalid_end).await.body,
            UploadEventBody::Record {
                kind: StreamRecordKind::Abort,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn declared_sentinel_data_is_refused_before_a_queued_exact_end() {
        let limits = bounds(64);
        let (ingress, mut events) = UploadIngress::new(budget(128), 3, 8);
        let deadline = Instant::now() + Duration::from_secs(1);
        assert_eq!(
            ingress.commit_open_with(deadline, || true),
            ControlCommit::Committed
        );
        ingress.served_send_through.store(4, Ordering::Release);
        ingress.received_through.store(3, Ordering::Release);
        ingress.accepted.store(3, Ordering::Release);

        assert_eq!(
            ingress
                .route_bound(wire(0x02, REQUEST_ID, STREAM_ID, 3, 0, &[0xaa]), &limits,)
                .await,
            BinaryRoute::Delivered
        );
        assert_eq!(
            ingress
                .route_bound(wire(0x04, REQUEST_ID, STREAM_ID, 3, 0, &[]), &limits)
                .await,
            BinaryRoute::Delivered
        );

        assert!(matches!(
            next_event(&mut events, &ingress).await.body,
            UploadEventBody::RejectedData(DataFailure::Declared { observed: 4 })
        ));
        assert_eq!(
            ingress.select_daemon_abort(TerminalYield::None),
            DaemonAbortSelection::Selected,
            "a later END cannot erase the earlier DATA refusal"
        );
        assert!(matches!(
            next_event(&mut events, &ingress).await.body,
            UploadEventBody::Record {
                kind: StreamRecordKind::End,
                ..
            }
        ));
        assert!(!ingress.commit_finalizing(3));
    }

    #[tokio::test]
    async fn maximum_sentinel_data_hits_stream_policy_before_a_queued_exact_end() {
        let limits = bounds(64);
        let (ingress, mut events) = UploadIngress::new(budget(128), 3, 3);
        let deadline = Instant::now() + Duration::from_secs(1);
        assert_eq!(
            ingress.commit_open_with(deadline, || true),
            ControlCommit::Committed
        );
        ingress.served_send_through.store(4, Ordering::Release);
        ingress.received_through.store(3, Ordering::Release);
        ingress.accepted.store(3, Ordering::Release);

        assert_eq!(
            ingress
                .route_bound(wire(0x02, REQUEST_ID, STREAM_ID, 3, 0, &[0xbb]), &limits,)
                .await,
            BinaryRoute::Delivered
        );
        assert_eq!(
            ingress
                .route_bound(wire(0x04, REQUEST_ID, STREAM_ID, 3, 0, &[]), &limits)
                .await,
            BinaryRoute::Delivered
        );

        assert!(matches!(
            next_event(&mut events, &ingress).await.body,
            UploadEventBody::RejectedData(DataFailure::TooLarge { observed: 4 })
        ));
        assert_eq!(
            ingress.select_daemon_abort(TerminalYield::None),
            DaemonAbortSelection::Selected,
            "the max+1 stage-stream refusal must win over a later exact END"
        );
        assert!(matches!(
            next_event(&mut events, &ingress).await.body,
            UploadEventBody::Record {
                kind: StreamRecordKind::End,
                ..
            }
        ));
        assert!(!ingress.commit_finalizing(3));
    }

    #[tokio::test]
    async fn a_complete_end_queued_before_disconnect_can_still_commit_finalizing() {
        let limits = bounds(64);
        let registry = StreamRegistry::new();
        let identity = StreamIdentity::new(REQUEST_ID, STREAM_ID).unwrap();
        let (ingress, mut events) = UploadIngress::new(budget(128), u64::MAX, u64::MAX);
        let deadline = Instant::now() + Duration::from_secs(1);
        assert_eq!(
            ingress.commit_open_with(deadline, || true),
            ControlCommit::Committed
        );
        let _binding = registry.bind_upload(identity, ingress.clone()).unwrap();
        assert_eq!(
            registry
                .route_binary_message(wire(0x04, REQUEST_ID, STREAM_ID, 0, 0, &[]), &limits,)
                .await,
            BinaryRoute::Delivered
        );

        registry.invalidate_connection();
        assert!(matches!(
            next_event(&mut events, &ingress).await.body,
            UploadEventBody::Record {
                kind: StreamRecordKind::End,
                ..
            }
        ));
        assert!(ingress.commit_finalizing(0));
        assert_eq!(ingress.phase(), UploadPhase::Finalizing);
    }

    #[tokio::test]
    async fn admitted_end_survives_full_terminal_lane_disconnect_and_preserves_order() {
        let (ingress, mut events) = active_ingress(128);
        let mut cancel_receivers = Vec::new();
        for _ in 0..UPLOAD_TERMINAL_EVENT_SLOTS {
            let (response, result) = oneshot::channel();
            let admission = {
                let _sequencing = ingress
                    .sequencing
                    .lock()
                    .expect("upload sequencing poisoned");
                ingress.begin_terminal_locked(true, true)
            };
            assert_eq!(
                ingress
                    .enqueue(
                        UploadEvent {
                            received_at: Instant::now(),
                            body: UploadEventBody::Cancel { response },
                            _pending_data: None,
                        },
                        true,
                        Some(admission),
                    )
                    .await,
                BinaryRoute::Delivered
            );
            cancel_receivers.push(result);
        }

        let end_admission = {
            let _sequencing = ingress
                .sequencing
                .lock()
                .expect("upload sequencing poisoned");
            ingress
                .producer_terminal
                .store(PRODUCER_TERMINAL_END, Ordering::Release);
            ingress.begin_terminal_locked(false, false)
        };
        let end_ingress = ingress.clone();
        let end = tokio::spawn(async move {
            end_ingress
                .enqueue(
                    UploadEvent {
                        received_at: Instant::now(),
                        body: UploadEventBody::Record {
                            kind: StreamRecordKind::End,
                            bytes: wire(0x04, REQUEST_ID, STREAM_ID, 0, 0, &[]),
                            _bytes: None,
                        },
                        _pending_data: None,
                    },
                    true,
                    Some(end_admission),
                )
                .await
        });
        tokio::task::yield_now().await;
        assert!(
            !end.is_finished(),
            "full terminal lane applies backpressure"
        );

        ingress.invalidate_connection_locked();
        assert_eq!(ingress.phase(), UploadPhase::Active);
        let UploadEventBody::Cancel { response } = next_event(&mut events, &ingress).await.body
        else {
            panic!("terminal FIFO must preserve the first cancellation");
        };
        response.send(CancelAttempt::Unknown).unwrap();
        assert_eq!(
            timeout(Duration::from_secs(1), end).await.unwrap().unwrap(),
            BinaryRoute::Delivered
        );
        for _ in 1..UPLOAD_TERMINAL_EVENT_SLOTS {
            let UploadEventBody::Cancel { response } = next_event(&mut events, &ingress).await.body
            else {
                panic!("queued cancellation order changed");
            };
            response.send(CancelAttempt::Unknown).unwrap();
        }
        assert!(matches!(
            next_event(&mut events, &ingress).await.body,
            UploadEventBody::Record {
                kind: StreamRecordKind::End,
                ..
            }
        ));
        assert!(ingress.commit_finalizing(0));
        for result in cancel_receivers {
            assert_eq!(result.await.unwrap(), CancelAttempt::Unknown);
        }
    }

    #[tokio::test]
    async fn terminal_admission_order_is_preserved_across_end_cancel_task_reordering() {
        let (ingress, mut events) = active_ingress(128);
        let (end_admission, cancel_admission) = {
            let _sequencing = ingress
                .sequencing
                .lock()
                .expect("upload sequencing poisoned");
            (
                ingress.begin_terminal_locked(false, false),
                ingress.begin_terminal_locked(true, true),
            )
        };
        let (cancel_response, cancel_result) = oneshot::channel();
        let cancel_ingress = ingress.clone();
        let cancel = tokio::spawn(async move {
            cancel_ingress
                .enqueue(
                    UploadEvent {
                        received_at: Instant::now(),
                        body: UploadEventBody::Cancel {
                            response: cancel_response,
                        },
                        _pending_data: None,
                    },
                    true,
                    Some(cancel_admission),
                )
                .await
        });
        tokio::task::yield_now().await;
        assert!(!cancel.is_finished(), "later cancel waits for admitted END");

        let end_ingress = ingress.clone();
        let end = tokio::spawn(async move {
            end_ingress
                .enqueue(
                    UploadEvent {
                        received_at: Instant::now(),
                        body: UploadEventBody::Record {
                            kind: StreamRecordKind::End,
                            bytes: wire(0x04, REQUEST_ID, STREAM_ID, 0, 0, &[]),
                            _bytes: None,
                        },
                        _pending_data: None,
                    },
                    true,
                    Some(end_admission),
                )
                .await
        });
        assert_eq!(end.await.unwrap(), BinaryRoute::Delivered);
        assert_eq!(cancel.await.unwrap(), BinaryRoute::Delivered);
        assert!(matches!(
            next_event(&mut events, &ingress).await.body,
            UploadEventBody::Record {
                kind: StreamRecordKind::End,
                ..
            }
        ));
        let UploadEventBody::Cancel { response } = next_event(&mut events, &ingress).await.body
        else {
            panic!("cancel must follow END admission order");
        };
        response.send(CancelAttempt::Unknown).unwrap();
        assert_eq!(cancel_result.await.unwrap(), CancelAttempt::Unknown);
    }

    #[tokio::test]
    async fn client_abort_overtakes_older_uncommitted_data_and_followers_become_protocol_faults() {
        let limits = bounds(64);
        let (ingress, mut events) = active_ingress(256);
        assert_eq!(
            ingress
                .route_bound(wire(0x02, REQUEST_ID, STREAM_ID, 0, 0, &[1]), &limits)
                .await,
            BinaryRoute::Delivered
        );
        assert_eq!(
            ingress
                .route_bound(wire(0x05, REQUEST_ID, STREAM_ID, 0, 1, &[]), &limits,)
                .await,
            BinaryRoute::Delivered
        );

        let deadline = Instant::now() + Duration::from_secs(1);
        assert!(matches!(
            await_active_operation(pending::<()>(), &ingress, deadline, deadline).await,
            Err(ActiveInterrupt::TerminalPending)
        ));
        assert_eq!(ingress.accepted.load(Ordering::Acquire), 0);
        assert!(matches!(
            next_event(&mut events, &ingress).await.body,
            UploadEventBody::Record {
                kind: StreamRecordKind::Abort,
                ..
            }
        ));
        events.drain_data(&ingress);
        assert_eq!(ingress.queued_events.load(Ordering::Acquire), 0);

        assert_eq!(
            ingress
                .route_bound(wire(0x05, REQUEST_ID, STREAM_ID, 0, 1, &[]), &limits,)
                .await,
            BinaryRoute::Delivered
        );
        assert!(ingress.try_select_client_abort());
        assert!(matches!(
            next_event(&mut events, &ingress).await.body,
            UploadEventBody::Malformed
        ));
        assert!(ingress.promote_client_abort_protocol());
        assert_eq!(ingress.phase(), UploadPhase::DaemonAbortQueued);
    }

    #[tokio::test(start_paused = true)]
    async fn queued_open_expires_without_committing_and_abort_ack_waits_are_bounded() {
        let (outbound, _queues) = Outbound::new(8, 1, 4096, 4096, 4096);
        let codec_bounds = bounds(64);
        let identity = StreamIdentity::new(REQUEST_ID, STREAM_ID).unwrap();

        let (opening, _events) = UploadIngress::new(budget(128), u64::MAX, u64::MAX);
        let open_deadline = Instant::now() + Duration::from_millis(10);
        let open_outbound = outbound.clone();
        let open_ingress = opening.clone();
        let open = tokio::spawn(async move {
            send_open(
                &open_outbound,
                &open_ingress,
                identity,
                0,
                &codec_bounds,
                open_deadline,
                || true,
            )
            .await
        });
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(10)).await;
        assert_eq!(open.await.unwrap(), BoundedControlOutcome::Deadline);
        assert_eq!(opening.phase(), UploadPhase::Opening);

        let (aborting, _events) = active_ingress(128);
        assert!(aborting.try_select_daemon_abort());
        let abort_deadline = Instant::now() + Duration::from_millis(10);
        let abort_outbound = outbound.clone();
        let abort_ingress = aborting.clone();
        let abort = tokio::spawn(async move {
            send_abort(
                &abort_outbound,
                &abort_ingress,
                identity,
                0,
                BinaryAbortReason::ProtocolError,
                &codec_bounds,
                abort_deadline,
            )
            .await
        });
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(10)).await;
        assert_eq!(abort.await.unwrap(), BoundedControlOutcome::Deadline);
        assert_eq!(aborting.phase(), UploadPhase::DaemonAbortQueued);

        let ack_deadline = Instant::now() + Duration::from_millis(10);
        let ack_outbound = outbound.clone();
        let ack = tokio::spawn(async move {
            send_ack_until(&ack_outbound, identity, 0, &codec_bounds, ack_deadline).await
        });
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(10)).await;
        assert_eq!(ack.await.unwrap(), BoundedControlOutcome::Deadline);
    }

    #[tokio::test]
    async fn inbound_queue_is_bounded_by_both_complete_record_bytes_and_count() {
        let limits = bounds(64);
        let record = wire(0x02, REQUEST_ID, STREAM_ID, 0, 0, &[1]);
        let record_bytes = record.len();

        let shared = budget(record_bytes);
        let (one, mut one_events) = UploadIngress::new(shared.clone(), u64::MAX, u64::MAX);
        let (two, mut two_events) = UploadIngress::new(shared, u64::MAX, u64::MAX);
        let deadline = Instant::now() + Duration::from_secs(1);
        assert_eq!(
            one.commit_open_with(deadline, || true),
            ControlCommit::Committed
        );
        assert_eq!(
            two.commit_open_with(deadline, || true),
            ControlCommit::Committed
        );
        one.served_send_through.store(u64::MAX, Ordering::Release);
        two.served_send_through.store(u64::MAX, Ordering::Release);
        assert_eq!(
            one.route_bound(record.clone(), &limits).await,
            BinaryRoute::Delivered
        );
        let blocked_ingress = two.clone();
        let blocked_record = record.clone();
        let byte_waiter =
            tokio::spawn(async move { blocked_ingress.route_bound(blocked_record, &limits).await });
        tokio::task::yield_now().await;
        assert!(!byte_waiter.is_finished());
        drop(next_event(&mut one_events, &one).await);
        assert_eq!(
            timeout(Duration::from_secs(1), byte_waiter)
                .await
                .unwrap()
                .unwrap(),
            BinaryRoute::Delivered
        );
        drop(next_event(&mut two_events, &two).await);

        let (ingress, mut events) = active_ingress(record_bytes * (UPLOAD_DATA_EVENT_SLOTS + 1));
        for offset in 0..UPLOAD_DATA_EVENT_SLOTS {
            assert_eq!(
                ingress
                    .route_bound(
                        wire(
                            0x02,
                            REQUEST_ID,
                            STREAM_ID,
                            u64::try_from(offset).unwrap(),
                            0,
                            &[1],
                        ),
                        &limits,
                    )
                    .await,
                BinaryRoute::Delivered
            );
        }
        let blocked_ingress = ingress.clone();
        let count_waiter = tokio::spawn(async move {
            blocked_ingress
                .route_bound(
                    wire(
                        0x02,
                        REQUEST_ID,
                        STREAM_ID,
                        u64::try_from(UPLOAD_DATA_EVENT_SLOTS).unwrap(),
                        0,
                        &[1],
                    ),
                    &limits,
                )
                .await
        });
        tokio::task::yield_now().await;
        assert!(!count_waiter.is_finished());
        drop(next_event(&mut events, &ingress).await);
        assert_eq!(
            timeout(Duration::from_secs(1), count_waiter)
                .await
                .unwrap()
                .unwrap(),
            BinaryRoute::Delivered
        );
        for _ in 0..UPLOAD_DATA_EVENT_SLOTS {
            drop(next_event(&mut events, &ingress).await);
        }
        assert_eq!(ingress.queued_events.load(Ordering::Acquire), 0);

        let (terminal_ingress, mut terminal_events) = active_ingress(128);
        for _ in 0..UPLOAD_TERMINAL_EVENT_SLOTS {
            assert_eq!(
                terminal_ingress.malformed(Instant::now()).await,
                BinaryRoute::Delivered
            );
        }
        let blocked_terminal = terminal_ingress.clone();
        let terminal_waiter =
            tokio::spawn(async move { blocked_terminal.malformed(Instant::now()).await });
        tokio::task::yield_now().await;
        assert!(!terminal_waiter.is_finished());
        drop(next_event(&mut terminal_events, &terminal_ingress).await);
        assert_eq!(
            timeout(Duration::from_secs(1), terminal_waiter)
                .await
                .unwrap()
                .unwrap(),
            BinaryRoute::Delivered
        );
        for _ in 0..UPLOAD_TERMINAL_EVENT_SLOTS {
            drop(next_event(&mut terminal_events, &terminal_ingress).await);
        }
        assert_eq!(terminal_ingress.queued_events.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn complete_legal_tiny_record_credit_window_cannot_hide_following_end() {
        let mut served = jeliya_core::typed::limits();
        served.max_frame_bytes = 128;
        served.max_concurrent_transfers = 1;
        let runtime = RuntimeLimits::from_served(&served).unwrap();
        let window = runtime.max_data_payload_bytes();
        assert!(window > UPLOAD_DATA_EVENT_SLOTS);

        let (ingress, mut events) = UploadIngress::new(
            UploadIngressBudget::new(runtime),
            u64::try_from(window).unwrap(),
            u64::try_from(window).unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(1);
        assert_eq!(
            ingress.commit_open_with(deadline, || true),
            ControlCommit::Committed
        );
        let credit_attempt = AtomicU8::new(CONTROL_ATTEMPT_QUEUED);
        assert!(ingress.try_start_active_credit(
            &credit_attempt,
            deadline,
            deadline,
            u64::try_from(window).unwrap(),
        ));

        let codec_bounds = bounds(runtime.max_frame_bytes());
        for offset in 0..window {
            assert_eq!(
                timeout(
                    Duration::from_secs(1),
                    ingress.route_bound(
                        wire(
                            0x02,
                            REQUEST_ID,
                            STREAM_ID,
                            u64::try_from(offset).unwrap(),
                            0,
                            &[0x5a],
                        ),
                        &codec_bounds,
                    ),
                )
                .await
                .expect("every record inside one CREDIT window is immediately admissible"),
                BinaryRoute::Delivered
            );
        }
        assert_eq!(
            timeout(
                Duration::from_secs(1),
                ingress.route_bound(
                    wire(
                        0x04,
                        REQUEST_ID,
                        STREAM_ID,
                        u64::try_from(window).unwrap(),
                        0,
                        &[],
                    ),
                    &codec_bounds,
                ),
            )
            .await
            .expect("the terminal lane remains reachable after a full legal DATA window"),
            BinaryRoute::Delivered
        );

        for expected in 0..window {
            let event = next_event(&mut events, &ingress).await;
            let UploadEventBody::Record {
                kind: StreamRecordKind::Data,
                bytes,
                ..
            } = event.body
            else {
                panic!("END overtook DATA admitted earlier on the wire");
            };
            let view = decode_stream_record_view(&bytes, &codec_bounds).unwrap();
            assert!(matches!(
                view.body,
                StreamRecordBodyView::Data { offset, payload }
                    if offset == u64::try_from(expected).unwrap() && payload == [0x5a]
            ));
        }
        assert!(matches!(
            next_event(&mut events, &ingress).await.body,
            UploadEventBody::Record {
                kind: StreamRecordKind::End,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn invalid_data_bypasses_a_full_data_lane_without_borrowing_later_credit() {
        let codec_bounds = bounds(64);
        let record_bytes = STREAM_HEADER_BYTES + 1;
        let budget = UploadIngressBudget {
            bytes: Arc::new(Semaphore::new(record_bytes)),
            data_messages: 1,
        };
        let (ingress, mut events) = UploadIngress::new(budget, 2, 2);
        let deadline = Instant::now() + Duration::from_secs(1);
        assert_eq!(
            ingress.commit_open_with(deadline, || true),
            ControlCommit::Committed
        );
        let first_credit = AtomicU8::new(CONTROL_ATTEMPT_QUEUED);
        assert!(ingress.try_start_active_credit(&first_credit, deadline, deadline, 1));

        assert_eq!(
            ingress
                .route_bound(wire(0x02, REQUEST_ID, STREAM_ID, 0, 0, &[1]), &codec_bounds)
                .await,
            BinaryRoute::Delivered
        );
        assert_eq!(ingress.budget.bytes.available_permits(), 0);

        // This duplicate is both discontinuous with the receive frontier and
        // beyond no new CREDIT. It must reach the terminal lane without
        // waiting for the full DATA byte/message lane.
        assert_eq!(
            timeout(
                Duration::from_secs(1),
                ingress.route_bound(wire(0x02, REQUEST_ID, STREAM_ID, 0, 0, &[2]), &codec_bounds,),
            )
            .await
            .expect("invalid DATA bypasses DATA capacity"),
            BinaryRoute::Delivered
        );

        // A later writer commit cannot retroactively turn that already
        // received refusal into accepted DATA.
        let later_credit = AtomicU8::new(CONTROL_ATTEMPT_QUEUED);
        assert!(!ingress.try_start_active_credit(&later_credit, deadline, deadline, 2));
        assert!(matches!(
            next_event(&mut events, &ingress).await.body,
            UploadEventBody::RejectedData(DataFailure::Protocol)
        ));
        assert_eq!(ingress.received_through.load(Ordering::Acquire), 1);
        assert_eq!(ingress.budget.bytes.available_permits(), 0);
        drop(next_event(&mut events, &ingress).await);
        assert_eq!(ingress.budget.bytes.available_permits(), record_bytes);
    }

    #[tokio::test(start_paused = true)]
    async fn full_data_lane_cannot_starve_end_abort_cancel_or_ack_behind_timers() {
        async fn fill_data(ingress: &Arc<UploadIngress>, bounds: &CodecBounds) {
            for offset in 0..UPLOAD_DATA_EVENT_SLOTS {
                assert_eq!(
                    ingress
                        .route_bound(
                            wire(
                                0x02,
                                REQUEST_ID,
                                STREAM_ID,
                                u64::try_from(offset).unwrap(),
                                0,
                                &[1],
                            ),
                            bounds,
                        )
                        .await,
                    BinaryRoute::Delivered
                );
            }
        }

        let codec_bounds = bounds(64);
        let data_bytes = STREAM_HEADER_BYTES + 1;

        let (ending, mut end_events) = active_ingress(data_bytes * UPLOAD_DATA_EVENT_SLOTS);
        fill_data(&ending, &codec_bounds).await;
        let end_boundary = Instant::now() + Duration::from_millis(10);
        assert_eq!(
            ending
                .route_bound(
                    wire(
                        0x04,
                        REQUEST_ID,
                        STREAM_ID,
                        u64::try_from(UPLOAD_DATA_EVENT_SLOTS).unwrap(),
                        0,
                        &[],
                    ),
                    &codec_bounds,
                )
                .await,
            BinaryRoute::Delivered
        );
        tokio::time::advance(Duration::from_millis(10)).await;
        for _ in 0..UPLOAD_DATA_EVENT_SLOTS {
            assert!(matches!(
                next_active(&mut end_events, &ending, end_boundary, end_boundary).await,
                ActiveInput::Event(UploadEvent {
                    body: UploadEventBody::Record {
                        kind: StreamRecordKind::Data,
                        ..
                    },
                    ..
                })
            ));
        }
        assert!(
            matches!(
                next_active(&mut end_events, &ending, end_boundary, end_boundary).await,
                ActiveInput::Event(UploadEvent {
                    body: UploadEventBody::Record {
                        kind: StreamRecordKind::End,
                        ..
                    },
                    ..
                })
            ),
            "END remains visible ahead of the timer but cannot overtake DATA admitted before it"
        );

        let (aborting, mut abort_events) = active_ingress(data_bytes * UPLOAD_DATA_EVENT_SLOTS);
        fill_data(&aborting, &codec_bounds).await;
        let stall_boundary = Instant::now() + Duration::from_millis(10);
        assert_eq!(
            aborting
                .route_bound(wire(0x05, REQUEST_ID, STREAM_ID, 0, 1, &[]), &codec_bounds)
                .await,
            BinaryRoute::Delivered
        );
        tokio::time::advance(Duration::from_millis(10)).await;
        assert!(matches!(
            next_active(
                &mut abort_events,
                &aborting,
                stall_boundary + Duration::from_secs(1),
                stall_boundary,
            )
            .await,
            ActiveInput::Event(UploadEvent {
                body: UploadEventBody::Record {
                    kind: StreamRecordKind::Abort,
                    ..
                },
                ..
            })
        ));

        let (cancelling, mut cancel_events) = active_ingress(data_bytes * UPLOAD_DATA_EVENT_SLOTS);
        fill_data(&cancelling, &codec_bounds).await;
        let cancel_boundary = Instant::now() + Duration::from_millis(10);
        let cancelling_ingress = cancelling.clone();
        let cancel = tokio::spawn(async move { cancelling_ingress.request_cancel().await });
        while cancelling.queued_cancels.load(Ordering::Acquire) == 0 {
            tokio::task::yield_now().await;
        }
        tokio::time::advance(Duration::from_millis(10)).await;
        let ActiveInput::Event(UploadEvent {
            body: UploadEventBody::Cancel { response },
            ..
        }) = next_active(
            &mut cancel_events,
            &cancelling,
            cancel_boundary,
            cancel_boundary,
        )
        .await
        else {
            panic!("cancel must own the terminal lane ahead of full DATA and timers");
        };
        response.send(CancelAttempt::Unknown).unwrap();
        assert_eq!(cancel.await.unwrap(), CancelAttempt::Unknown);

        let (acking, mut ack_events) = active_ingress(data_bytes * UPLOAD_DATA_EVENT_SLOTS);
        fill_data(&acking, &codec_bounds).await;
        assert!(acking.try_select_daemon_abort());
        let ack_deadline = Instant::now() + Duration::from_secs(1);
        assert_eq!(
            acking.commit_daemon_abort(ack_deadline),
            ControlCommit::Committed
        );
        assert_eq!(
            acking
                .route_bound(wire(0x06, REQUEST_ID, STREAM_ID, 0, 5, &[]), &codec_bounds)
                .await,
            BinaryRoute::Delivered
        );
        assert!(matches!(
            next_event(&mut ack_events, &acking).await.body,
            UploadEventBody::Record {
                kind: StreamRecordKind::Ack,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn zero_logical_byte_pool_still_routes_one_sentinel_probe_for_typed_refusal() {
        let mut served = jeliya_core::typed::limits();
        served.max_concurrent_transfers = 1;
        served.max_transfer_bytes_inflight = 0;
        let runtime = RuntimeLimits::from_served(&served).unwrap();
        assert_eq!(runtime.data_queue_capacity_bytes(), 0);
        assert!(runtime.upload_ingress_capacity_bytes() > STREAM_HEADER_BYTES);

        let (ingress, mut events) =
            UploadIngress::new(UploadIngressBudget::new(runtime), 0, u64::MAX);
        let deadline = Instant::now() + Duration::from_secs(1);
        assert_eq!(
            ingress.commit_open_with(deadline, || true),
            ControlCommit::Committed
        );
        ingress.served_send_through.store(1, Ordering::Release);
        assert_eq!(
            timeout(
                Duration::from_secs(1),
                ingress.route_bound(
                    wire(0x02, REQUEST_ID, STREAM_ID, 0, 0, &[0xff]),
                    &bounds(runtime.max_frame_bytes()),
                ),
            )
            .await
            .expect("sentinel probe must not deadlock behind a zero-byte semaphore"),
            BinaryRoute::Delivered
        );
        assert!(matches!(
            next_event(&mut events, &ingress).await.body,
            UploadEventBody::RejectedData(DataFailure::Declared { observed: 1 })
        ));
        assert_eq!(ingress.received_through.load(Ordering::Acquire), 0);
        assert_eq!(
            ingress.budget.bytes.available_permits(),
            runtime.upload_ingress_capacity_bytes(),
            "a policy-refused sentinel must consume no DATA storage"
        );
    }

    #[tokio::test]
    async fn cancellation_is_principal_scoped_and_concurrent_repeats_use_the_ledger() {
        let request = cancel_request("transfer-op");
        let (_dir, engine, mut owner, registry) =
            cancellation_ledger(&request.transfer_op_id, "principal-a", 9).await;
        let (ingress, mut events) = active_ingress(256);
        let guard = registry
            .register(
                "principal-a".into(),
                request.transfer_op_id.clone(),
                ingress.clone(),
                9,
            )
            .unwrap();

        assert_transfer_unknown(
            registry.cancel("principal-b", &request).await,
            &request.transfer_op_id,
        );

        let first_registry = registry.clone();
        let first_request = request.clone();
        let first =
            tokio::spawn(async move { first_registry.cancel("principal-a", &first_request).await });
        let second_registry = registry.clone();
        let second_request = request.clone();
        let second =
            tokio::spawn(
                async move { second_registry.cancel("principal-a", &second_request).await },
            );
        timeout(Duration::from_secs(1), async {
            while ingress.queued_events.load(Ordering::Acquire) < 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("both concurrent cancellation requests reached the actor");

        let UploadEventBody::Cancel { response } = next_event(&mut events, &ingress).await.body
        else {
            panic!("expected first cancellation event");
        };
        assert!(guard.select_cancelled(&mut owner, 3, 9));
        response
            .send(CancelAttempt::Cancelled { accepted: 3 })
            .unwrap();

        let UploadEventBody::Cancel { response } = next_event(&mut events, &ingress).await.body
        else {
            panic!("expected concurrent cancellation event");
        };
        response.send(CancelAttempt::Unknown).unwrap();

        let first = first.await.unwrap().unwrap();
        let second = second.await.unwrap().unwrap();
        assert_eq!(
            usize::from(matches!(&first.outcome, CancelOutcome::Cancelled))
                + usize::from(matches!(&second.outcome, CancelOutcome::Cancelled)),
            1
        );
        assert_eq!(
            usize::from(matches!(&first.outcome, CancelOutcome::AlreadyCancelled))
                + usize::from(matches!(&second.outcome, CancelOutcome::AlreadyCancelled)),
            1
        );
        for result in [&first, &second] {
            assert_eq!(result.transferred_bytes, 3);
            assert_eq!(result.total, ByteTotal::Known { bytes: 9 });
        }

        let repeated = registry.cancel("principal-a", &request).await.unwrap();
        assert_eq!(repeated.outcome, CancelOutcome::AlreadyCancelled);
        assert_eq!(repeated.transferred_bytes, 3);
        drop(guard);
        let published = owner.complete(Err(ApiError::NotReady));
        assert!(matches!(
            published,
            Err(ApiError::StreamAborted {
                transferred_bytes: 3,
                total: ByteTotal::Known { bytes: 9 },
                reason: StreamAbortReason::Cancelled,
            })
        ));
        assert_eq!(
            engine.recorded_file_share_cancellation("principal-a", &request.transfer_op_id),
            Some((3, 9))
        );
        assert_eq!(
            registry
                .cancel("principal-a", &request)
                .await
                .unwrap()
                .outcome,
            CancelOutcome::AlreadyCancelled,
            "ledger publication preserves cancellation replay without a tombstone"
        );

        let (other_ingress, _other_events) = active_ingress(128);
        let other_guard = registry
            .register(
                "principal-b".into(),
                request.transfer_op_id.clone(),
                other_ingress,
                9,
            )
            .expect("same op_id is isolated by principal");
        drop(other_guard);
        assert_transfer_unknown(
            registry.cancel("principal-b", &request).await,
            &request.transfer_op_id,
        );
    }

    #[tokio::test]
    async fn cancellation_refuses_finalizing_and_unknown_uploads() {
        let registry = UploadCancellationRegistry::default();
        let request = cancel_request("finalizing-op");
        assert_transfer_unknown(
            registry.cancel("principal", &request).await,
            &request.transfer_op_id,
        );

        let (ingress, _events) = active_ingress(128);
        let guard = registry
            .register(
                "principal".into(),
                request.transfer_op_id.clone(),
                ingress.clone(),
                0,
            )
            .unwrap();
        assert!(ingress.commit_finalizing(0));
        assert_transfer_unknown(
            registry.cancel("principal", &request).await,
            &request.transfer_op_id,
        );
        drop(guard);
    }

    #[tokio::test]
    async fn cancellation_and_end_races_preserve_the_first_active_terminal() {
        let codec_bounds = bounds(64);

        // END is the first terminal admitted. Once the actor validates it and
        // enters FINALIZING, a principal-scoped cancel cannot replace that
        // result or record daemon-selected cancellation provenance.
        let end_registry = UploadCancellationRegistry::default();
        let end_request = cancel_request("end-first-op");
        let (ending, mut end_events) = active_ingress(128);
        let end_guard = end_registry
            .register(
                "principal".into(),
                end_request.transfer_op_id.clone(),
                ending.clone(),
                0,
            )
            .unwrap();
        assert_eq!(
            ending
                .route_bound(wire(0x04, REQUEST_ID, STREAM_ID, 0, 0, &[]), &codec_bounds,)
                .await,
            BinaryRoute::Delivered
        );
        let UploadEventBody::Record { bytes, .. } = next_event(&mut end_events, &ending).await.body
        else {
            panic!("END must own the first terminal admission");
        };
        assert!(matches!(
            decode_stream_record_view(&bytes, &codec_bounds)
                .unwrap()
                .body,
            StreamRecordBodyView::End { total: 0 }
        ));
        let mut state = ConsumerState::new(0, 8);
        state.record_credit(0, 1).unwrap();
        assert_eq!(state.validate_end(0), Ok(()));
        assert!(ending.commit_finalizing(0));
        assert_transfer_unknown(
            end_registry.cancel("principal", &end_request).await,
            &end_request.transfer_op_id,
        );
        drop(end_guard);

        // The reverse order selects cancellation while ACTIVE. A later END is
        // drained under the daemon-selected ABORT and cannot enter FINALIZING.
        let cancel_request = cancel_request("cancel-first-end-op");
        let (_cancel_dir, _cancel_engine, mut cancel_owner, cancel_registry) =
            cancellation_ledger(&cancel_request.transfer_op_id, "principal", 0).await;
        let (cancelling, mut cancel_events) = active_ingress(128);
        let cancel_guard = cancel_registry
            .register(
                "principal".into(),
                cancel_request.transfer_op_id.clone(),
                cancelling.clone(),
                0,
            )
            .unwrap();
        let cancel_registry_task = cancel_registry.clone();
        let cancel_request_task = cancel_request.clone();
        let cancel = tokio::spawn(async move {
            cancel_registry_task
                .cancel("principal", &cancel_request_task)
                .await
        });
        while cancelling.queued_cancels.load(Ordering::Acquire) == 0 {
            tokio::task::yield_now().await;
        }
        let UploadEventBody::Cancel { response } =
            next_event(&mut cancel_events, &cancelling).await.body
        else {
            panic!("cancel must own the first terminal admission");
        };
        assert!(cancel_guard.select_cancelled(&mut cancel_owner, 0, 0));
        response
            .send(CancelAttempt::Cancelled { accepted: 0 })
            .unwrap();
        let outcome = cancel.await.unwrap().unwrap();
        assert_eq!(outcome.outcome, CancelOutcome::Cancelled);
        assert_eq!(outcome.transferred_bytes, 0);

        assert_eq!(
            cancelling
                .route_bound(wire(0x04, REQUEST_ID, STREAM_ID, 0, 0, &[]), &codec_bounds,)
                .await,
            BinaryRoute::Delivered
        );
        assert!(matches!(
            next_event(&mut cancel_events, &cancelling).await.body,
            UploadEventBody::Record {
                kind: StreamRecordKind::End,
                ..
            }
        ));
        assert!(!cancelling.commit_finalizing(0));
        assert_eq!(
            cancel_registry
                .cancel("principal", &cancel_request)
                .await
                .unwrap()
                .outcome,
            CancelOutcome::AlreadyCancelled
        );
        drop(cancel_guard);
    }

    #[tokio::test]
    async fn cancellation_and_client_abort_races_preserve_the_first_terminal_owner() {
        let codec_bounds = bounds(64);

        // A producer ABORT admitted first owns the client-abort handshake;
        // the later cancellation request is indistinguishable from unknown.
        let abort_registry = UploadCancellationRegistry::default();
        let abort_request = cancel_request("abort-first-op");
        let (aborting, mut abort_events) = active_ingress(128);
        let abort_guard = abort_registry
            .register(
                "principal".into(),
                abort_request.transfer_op_id.clone(),
                aborting.clone(),
                0,
            )
            .unwrap();
        assert_eq!(
            aborting
                .route_bound(wire(0x05, REQUEST_ID, STREAM_ID, 0, 2, &[]), &codec_bounds,)
                .await,
            BinaryRoute::Delivered
        );
        let abort_registry_task = abort_registry.clone();
        let abort_request_task = abort_request.clone();
        let cancel = tokio::spawn(async move {
            abort_registry_task
                .cancel("principal", &abort_request_task)
                .await
        });
        while aborting.queued_cancels.load(Ordering::Acquire) == 0 {
            tokio::task::yield_now().await;
        }
        assert!(matches!(
            next_event(&mut abort_events, &aborting).await.body,
            UploadEventBody::Record {
                kind: StreamRecordKind::Abort,
                ..
            }
        ));
        assert!(aborting.try_select_client_abort());
        let UploadEventBody::Cancel { response } =
            next_event(&mut abort_events, &aborting).await.body
        else {
            panic!("later cancel must remain queued behind producer ABORT");
        };
        response.send(CancelAttempt::Unknown).unwrap();
        assert_transfer_unknown(cancel.await.unwrap(), &abort_request.transfer_op_id);
        assert_eq!(aborting.phase(), UploadPhase::ClientAbortPending);
        drop(abort_guard);

        // A cancel admitted first selects the daemon's authoritative ABORT.
        // The crossed producer ABORT remains an ACK obligation but cannot
        // replace the cancellation result.
        let cancel_request = cancel_request("cancel-first-abort-op");
        let (_cancel_dir, _cancel_engine, mut cancel_owner, cancel_registry) =
            cancellation_ledger(&cancel_request.transfer_op_id, "principal", 0).await;
        let (cancelling, mut cancel_events) = active_ingress(128);
        let cancel_guard = cancel_registry
            .register(
                "principal".into(),
                cancel_request.transfer_op_id.clone(),
                cancelling.clone(),
                0,
            )
            .unwrap();
        let cancel_registry_task = cancel_registry.clone();
        let cancel_request_task = cancel_request.clone();
        let cancel = tokio::spawn(async move {
            cancel_registry_task
                .cancel("principal", &cancel_request_task)
                .await
        });
        while cancelling.queued_cancels.load(Ordering::Acquire) == 0 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            cancelling
                .route_bound(wire(0x05, REQUEST_ID, STREAM_ID, 0, 2, &[]), &codec_bounds,)
                .await,
            BinaryRoute::Delivered
        );
        let UploadEventBody::Cancel { response } =
            next_event(&mut cancel_events, &cancelling).await.body
        else {
            panic!("cancel must own the first terminal admission");
        };
        assert!(cancel_guard.select_cancelled(&mut cancel_owner, 0, 0));
        response
            .send(CancelAttempt::Cancelled { accepted: 0 })
            .unwrap();
        assert_eq!(
            cancel.await.unwrap().unwrap().outcome,
            CancelOutcome::Cancelled
        );
        assert!(matches!(
            next_event(&mut cancel_events, &cancelling).await.body,
            UploadEventBody::Record {
                kind: StreamRecordKind::Abort,
                ..
            }
        ));
        assert!(!cancelling.try_select_client_abort());
        assert_eq!(cancelling.phase(), UploadPhase::DaemonAbortQueued);
        assert_eq!(
            cancel_registry
                .cancel("principal", &cancel_request)
                .await
                .unwrap()
                .outcome,
            CancelOutcome::AlreadyCancelled
        );
        drop(cancel_guard);
    }

    #[tokio::test]
    async fn an_already_selected_deadline_makes_cancellation_unknown() {
        let registry = UploadCancellationRegistry::default();
        let request = cancel_request("deadline-first-op");
        let (ingress, _events) = active_ingress(128);
        let guard = registry
            .register(
                "principal".into(),
                request.transfer_op_id.clone(),
                ingress.clone(),
                1,
            )
            .unwrap();
        let now = Instant::now();
        assert!(matches!(
            ingress.commit_accepted(0, now, now + Duration::from_secs(1)),
            Err(ActiveInterrupt::Deadline)
        ));
        assert_eq!(ingress.phase(), UploadPhase::DaemonAbortQueued);
        assert_transfer_unknown(
            registry.cancel("principal", &request).await,
            &request.transfer_op_id,
        );
        drop(guard);
    }

    #[tokio::test(start_paused = true)]
    async fn explicit_terminals_win_timer_equality_but_local_acceptance_does_not() {
        let (ingress, mut events) = active_ingress(128);
        let boundary = Instant::now() + Duration::from_millis(10);
        tokio::time::advance(Duration::from_millis(10)).await;

        let (response, _result) = oneshot::channel();
        let admission = {
            let _sequencing = ingress
                .sequencing
                .lock()
                .expect("upload sequencing poisoned");
            ingress.begin_terminal_locked(true, true)
        };
        assert_eq!(
            ingress
                .enqueue(
                    UploadEvent {
                        received_at: boundary,
                        body: UploadEventBody::Cancel { response },
                        _pending_data: None,
                    },
                    true,
                    Some(admission),
                )
                .await,
            BinaryRoute::Delivered
        );
        assert!(matches!(
            await_active_operation(pending::<()>(), &ingress, boundary, boundary).await,
            Err(ActiveInterrupt::TerminalPending)
        ));
        let UploadEventBody::Cancel { response } = next_event(&mut events, &ingress).await.body
        else {
            panic!("expected cancellation event");
        };
        let cancel_event = UploadEvent {
            received_at: boundary,
            body: UploadEventBody::Cancel { response },
            _pending_data: None,
        };
        let mut state = ConsumerState::new(0, 8);
        state.record_credit(0, 1).unwrap();
        assert!(active_event_timer_failure(
            &cancel_event,
            &state,
            &bounds(64),
            boundary,
            boundary,
            10,
        )
        .is_none());
        let UploadEventBody::Cancel { response } = cancel_event.body else {
            unreachable!()
        };
        response.send(CancelAttempt::Unknown).unwrap();

        let valid_abort = UploadEvent {
            received_at: boundary,
            body: UploadEventBody::Record {
                kind: StreamRecordKind::Abort,
                bytes: wire(0x05, REQUEST_ID, STREAM_ID, 0, 1, &[]),
                _bytes: None,
            },
            _pending_data: None,
        };
        assert!(active_event_timer_failure(
            &valid_abort,
            &state,
            &bounds(64),
            boundary,
            boundary,
            10,
        )
        .is_none());
        assert!(active_event_timer_failure(
            &valid_abort,
            &state,
            &bounds(64),
            boundary + Duration::from_millis(1),
            boundary,
            10,
        )
        .is_none());

        let invalid_abort = UploadEvent {
            received_at: boundary,
            body: UploadEventBody::Record {
                kind: StreamRecordKind::Abort,
                bytes: wire(0x05, REQUEST_ID, STREAM_ID, 1, 1, &[]),
                _bytes: None,
            },
            _pending_data: None,
        };
        assert!(matches!(
            active_event_timer_failure(&invalid_abort, &state, &bounds(64), boundary, boundary, 10,),
            Some(UploadFailure::Deadline { .. })
        ));
        assert!(matches!(
            active_event_timer_failure(
                &invalid_abort,
                &state,
                &bounds(64),
                boundary + Duration::from_millis(1),
                boundary,
                10,
            ),
            Some(UploadFailure::Stall)
        ));

        assert!(matches!(
            ingress.commit_accepted(1, boundary, boundary),
            Err(ActiveInterrupt::TerminalPending)
        ));
        ingress.clear_processing_terminal();
        assert!(matches!(
            ingress.commit_accepted(1, boundary, boundary),
            Err(ActiveInterrupt::Deadline)
        ));
        assert_eq!(ingress.phase(), UploadPhase::DaemonAbortQueued);
        assert_eq!(ingress.accepted.load(Ordering::Acquire), 0);
    }
}
