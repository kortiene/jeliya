//! Per-room reconciliation state and convergence (#169 §R2, §R6, §R7, §R8).
//!
//! A room is always in exactly one phase: [`Phase::Converged`] (an
//! authoritative, gap-free timeline up to `watermark`, extended in place by
//! live pushes), [`Phase::Reconciling`] (a baseline read is outstanding; live
//! pushes are buffered), or [`Phase::NeedsReconcile`] (a read failed; the next
//! liveness trigger relaunches). Convergence keys ordering on the dense position
//! `pos`, dedup identity on `event_id`, and treats the signed `at`/`author` as
//! the evidence a record carries — the reconciler never fabricates a position or
//! a timestamp.

use std::collections::{HashMap, HashSet, VecDeque};

use jeliya_api::{
    ApiError, Cursor, DeviceId, Direction, Event, EventId, GapReason, GapTo, Link, MemberRow, Page,
    PeerRow, Reachability, RoomId, RoomMembers, RoomPeers, RoomTimeline, Standing, StreamResync,
    SubjectId, Truncated,
};

use crate::error::{CallError, LocalError};
use crate::reconcile::buffer::{estimated_event_bytes, event_identifiers_within, ReconcileBuffer};
use crate::reconcile::reason::ResyncReason;
use crate::reconcile::view::RoomView;
use crate::reconcile::ReconcileLimits;

/// One typed baseline read the driver issues through [`crate::ClientHandle::call`].
pub(crate) enum ReadRequest {
    /// A `room.timeline` page (bootstrap history).
    Timeline(RoomTimeline),
    /// A `stream.resync` page (authoritative recovery).
    Resync(StreamResync),
    /// A `room.members` roster read (authoritative membership).
    Members(RoomMembers),
    /// A `room.peers` presence read (authoritative presence).
    Peers(RoomPeers),
}

/// The decoded reply the driver feeds back for one [`ReadRequest`].
pub(crate) enum ReadReply {
    /// A `room.timeline` page, or its classified failure.
    Timeline(Result<jeliya_api::RoomTimelineOut, CallError>),
    /// A `stream.resync` page, or its classified failure (a `resync_required`
    /// arrives as `Err(CallError::Wire(ApiError::ResyncRequired { .. }))`).
    Resync(Result<jeliya_api::StreamResyncOut, CallError>),
    /// A `room.members` roster, or its classified failure.
    Members(Result<jeliya_api::RoomMembersOut, CallError>),
    /// A `room.peers` snapshot, or its classified failure.
    Peers(Result<jeliya_api::RoomPeersOut, CallError>),
}

/// The next read to issue in a reconciliation's plan.
#[derive(Clone)]
enum ReadStep {
    /// Read a `room.timeline` page starting at `cursor`.
    Timeline {
        cursor: Cursor,
        /// Bootstrap stops once a returned page reaches this subscription
        /// anchor; `None` is retained for future non-bootstrap timeline reads.
        anchor: Option<u64>,
    },
    /// Read a `stream.resync` page from `from_pos`.
    Resync { from_pos: u64 },
    /// Read the `room.members` roster.
    Members,
    /// Read the `room.peers` snapshot.
    Peers,
}

/// What the core does after feeding a reply to the room.
pub(crate) enum ReplyOutcome {
    /// Relaunch after a protocol-inconsistent baseline page.
    Retry { reason: ResyncReason },
    /// Issue the next read of this reconciliation (same `read_id`, same epoch).
    NextRead(ReadRequest),
    /// The reconciliation converged; the commit is done, broadcast this view.
    /// `rerun` carries a coalesced re-trigger to relaunch immediately (§R9).
    Converged {
        /// The converged view to broadcast.
        view: RoomView,
        /// A coalesced re-trigger, if one accrued.
        rerun: Option<ResyncReason>,
        /// Local loss recovered by a stronger cause, surfaced before `view`.
        dropped: u64,
    },
    /// The daemon answered `resync_required`; discard back to `from_pos` and
    /// relaunch an incremental read from there.
    Restart {
        /// The observable cause.
        reason: ResyncReason,
        /// The position to re-read from.
        from_pos: u64,
    },
    /// The read failed (disconnect/timeout/other); the room parks in
    /// [`Phase::NeedsReconcile`] carrying the coalesced cause.
    ///
    /// A failed settle **is** a settle (§R9), so any re-trigger that accrued
    /// while the read was outstanding is returned here rather than dropped with
    /// the reconciliation. This matters beyond liveness: a coalesced
    /// [`ResyncReason::ResyncRequiredByDaemon`] carries the daemon's discard
    /// position, and **no watermark encodes it** — losing it leaves repudiated
    /// positions in the timeline forever while the room still looks converged.
    Failed {
        /// The classified read failure.
        err: CallError,
        /// The coalesced re-trigger to relaunch once, if one accrued.
        rerun: Option<ResyncReason>,
    },
}

/// The outcome of offering a live event push to a converged room.
pub(crate) enum LiveOutcome {
    /// The push was a duplicate or already held; nothing changed.
    Ignored,
    /// The push extended the timeline in place; broadcast this view.
    Applied(RoomView),
    /// A position discontinuity opened; relaunch with this cause.
    NeedResync(ResyncReason),
}

/// The outcome of applying a live peer push to a converged room.
pub(crate) enum PeerOutcome {
    /// The generation fence rejected the frame.
    Ignored,
    /// The peer set changed; broadcast this view.
    Applied(RoomView),
    /// The configured peer bound was reached; force an authoritative presence read.
    Overflow,
}

/// Maximum strictly-progressing `resync_required` redirects honoured between
/// two successful commits. A legitimate daemon names one discard cursor and
/// serves it; an endless descending chain is a malfunction and parks.
const MAX_RESTART_CHAIN: u32 = 8;

/// One peer push held until an in-flight reconciliation settles.
struct BufferedPeerPush {
    subject_id: SubjectId,
    device_id: DeviceId,
    link: Link,
    generation: u64,
}

/// The next action to take after mutating the outstanding reconciliation.
enum Next {
    /// Re-issue the (updated) outstanding read (paging continuation).
    Issue,
    /// Advance the plan (or converge if exhausted).
    Advance,
    /// The daemon named a `resync_required` position.
    Restart(u64),
    /// The read failed.
    Fail(CallError),
    /// The daemon returned a structurally inconsistent page; force a fresh
    /// authoritative recovery without publishing a partial baseline.
    Gap(ResyncReason),
}

/// An in-flight reconciliation's transient state.
struct Reconciling {
    /// The epoch this reconciliation is fenced by (§R4).
    epoch: u64,
    /// The observable cause.
    reason: ResyncReason,
    /// The read identity all this reconciliation's reads carry.
    read_id: u64,
    /// The read currently outstanding.
    outstanding: ReadStep,
    /// The reads still to issue after `outstanding`.
    plan: VecDeque<ReadStep>,
    /// The bounded tail of baseline events accumulated across pages, in ascending
    /// `pos`. Older validated pages advance the scalar baseline cursor but are
    /// not retained because the rendered timeline is a finite window.
    events: Vec<Event>,
    /// A bounded exact id window used for recent duplicate identities.
    baseline_ids: DedupRing,
    /// Exact, count- and byte-bounded id evidence covering every page scanned
    /// in this reconciliation, including pages older than the retained tail.
    /// Exhausting the explicit total-history ceiling fails closed.
    baseline_seen_ids: BaselineSeenIds,
    /// Whether any raw baseline event has been seen yet.
    baseline_seen: bool,
    /// Whether at least one raw event was retained in the baseline (genesis at
    /// position zero is a real event, so this is distinct from `baseline_last`).
    baseline_has_event: bool,
    /// Last raw position validated across baseline pages.
    scan_pos: u64,
    /// Last position included in the baseline (events beyond a bootstrap anchor
    /// are validated but deliberately excluded).
    baseline_last: u64,
    /// Position from which this baseline was requested.
    baseline_start: u64,
    /// Whether this reconciliation replaces the durable timeline rather than
    /// extending the existing watermark. Full `room.timeline` bootstraps use
    /// replacement semantics, including reactivation of a tracked room.
    replace_timeline: bool,
    /// Whether this reconciliation must replace presence before it can settle.
    presence: bool,
    /// A loss was already known before this baseline began. The validated
    /// baseline may advance durable state, but no view is published until the
    /// queued LocalOverflow recovery also settles.
    suppress_view: bool,
    /// Maximum baseline tail retained while pages are collected.
    event_cap: usize,
    /// Estimated bytes currently retained in `events`.
    event_bytes: u64,
    /// Maximum estimated bytes retained in the baseline tail.
    event_bytes_cap: u64,
    /// A membership event appeared anywhere in the validated baseline. This
    /// matters even when it fell out of the bounded event tail.
    baseline_membership: bool,
    /// Live event pushes held while the baseline read is outstanding.
    buffer: ReconcileBuffer,
    /// Live peer pushes held until the authoritative baseline and its snapshot
    /// have settled. This prevents events-only reconciliation from dropping
    /// presence changes.
    peer_buffer: VecDeque<BufferedPeerPush>,
    /// Estimated identifier bytes retained by `peer_buffer`; together with the
    /// event buffer this stays within `buffer_bytes`.
    peer_buffer_bytes: u64,
    /// Peer pushes lost because the bounded peer buffer was full.
    peer_dropped: u64,
    /// A coalesced re-trigger to run once this reconciliation settles (§R9).
    rerun: Option<ResyncReason>,
    /// The authoritative roster, once read.
    members: Option<Vec<MemberRow>>,
    /// The authoritative presence snapshot, once read.
    peers: Option<(Reachability, Vec<PeerRow>)>,
}

/// A room's phase.
enum Phase {
    /// An authoritative, gap-free timeline up to `watermark`.
    Converged,
    /// A baseline read is outstanding; live pushes are buffered. Boxed so the
    /// large reconciliation state does not inflate every idle room's `Phase`.
    Reconciling(Box<Reconciling>),
    /// A read failed; the next liveness trigger relaunches.
    NeedsReconcile {
        /// The cause to carry into the relaunch.
        reason: ResyncReason,
    },
}

/// A fixed-size FIFO of recently-applied `event_id`s (§R7). It catches exact-
/// identity duplicates in the narrow window where `pos` is ambiguous across a
/// convergence boundary; beyond the window the `watermark` already suffices. It
/// is a constant-size ring, **not** an unbounded set keyed by external input.
#[derive(Clone)]
struct DedupRing {
    cap: usize,
    order: VecDeque<EventId>,
    set: HashSet<EventId>,
}

impl DedupRing {
    fn new(cap: u32) -> Self {
        Self {
            cap: cap as usize,
            order: VecDeque::new(),
            set: HashSet::new(),
        }
    }

    fn contains(&self, id: &EventId) -> bool {
        self.set.contains(id)
    }

    fn insert(&mut self, id: EventId) {
        if self.cap == 0 {
            return;
        }
        if self.set.insert(id.clone()) {
            self.order.push_back(id);
            if self.order.len() > self.cap {
                if let Some(evicted) = self.order.pop_front() {
                    self.set.remove(&evicted);
                }
            }
        }
    }
}

/// Exact, count- and byte-bounded identity/position evidence for every event
/// scanned by one reconciliation or committed within the supported room
/// history. Positions let daemon-directed truncation discard only the
/// repudiated suffix without forgetting older identities.
struct BaselineSeenIds {
    ids: HashMap<EventId, u64>,
    bytes: u64,
    count_cap: u32,
    bytes_cap: u64,
}

impl BaselineSeenIds {
    fn new(count_cap: u32, bytes_cap: u64) -> Self {
        Self {
            ids: HashMap::new(),
            bytes: 0,
            count_cap,
            bytes_cap,
        }
    }

    fn contains(&self, id: &EventId) -> bool {
        self.ids.contains_key(id)
    }

    fn position(&self, id: &EventId) -> Option<u64> {
        self.ids.get(id).copied()
    }

    fn can_merge(&self, other: &Self) -> bool {
        self.ids.len().saturating_add(other.ids.len()) <= self.count_cap as usize
            && self.bytes.saturating_add(other.bytes) <= self.bytes_cap
            && other.ids.keys().all(|id| !self.ids.contains_key(id))
    }

    fn can_merge_through(&self, other: &Self, pos: u64) -> bool {
        let mut count = 0_usize;
        let mut bytes = 0_u64;
        for (id, event_pos) in &other.ids {
            if *event_pos > pos {
                continue;
            }
            if self.ids.contains_key(id) {
                return false;
            }
            count = count.saturating_add(1);
            bytes = bytes.saturating_add(64_u64.saturating_add(id.as_str().len() as u64));
        }
        self.ids.len().saturating_add(count) <= self.count_cap as usize
            && self.bytes.saturating_add(bytes) <= self.bytes_cap
    }

    fn merge(&mut self, other: Self) {
        debug_assert!(self.can_merge(&other));
        self.bytes = self.bytes.saturating_add(other.bytes);
        self.ids.extend(other.ids);
    }

    fn retain_through(&mut self, pos: u64) {
        self.ids.retain(|_, event_pos| *event_pos <= pos);
        self.bytes = self.ids.keys().fold(0_u64, |bytes, id| {
            bytes.saturating_add(64_u64.saturating_add(id.as_str().len() as u64))
        });
    }

    /// Insert `id`, returning false for a duplicate or when retaining exact
    /// evidence would exceed the explicit total-history resource ceiling.
    fn insert_new(&mut self, id: &EventId, pos: u64) -> bool {
        if self.ids.contains_key(id) {
            return false;
        }
        let size = 64_u64.saturating_add(id.as_str().len() as u64);
        if self.ids.len() as u64 >= u64::from(self.count_cap)
            || self.bytes.saturating_add(size) > self.bytes_cap
        {
            return false;
        }
        self.bytes = self.bytes.saturating_add(size);
        self.ids.insert(id.clone(), pos).is_none()
    }
}

/// Per-room durable state plus the transient reconciliation.
pub(crate) struct RoomState {
    room_id: RoomId,
    limits: ReconcileLimits,
    /// The last applied position, or `None` before the first event.
    watermark: Option<u64>,
    /// The converged, position-ordered timeline.
    timeline: Vec<Event>,
    /// The recent-id dedup ring.
    recent_ids: DedupRing,
    /// Exact identity evidence for the supported room history, bounded by the
    /// same total-scan count/byte ceilings as authoritative replacement.
    all_event_ids: BaselineSeenIds,
    /// The authoritative roster (replaced wholesale).
    members: Vec<MemberRow>,
    /// The authoritative presence snapshot (replaced wholesale).
    peers: Vec<PeerRow>,
    /// The last authoritative reachability.
    reachability: Reachability,
    /// Last-seen connection generation per peer in the current authoritative
    /// snapshot. `None` means the snapshot named the peer before any live
    /// generation-bearing push. This map is bounded by `peer_capacity`.
    peer_generations: HashMap<(SubjectId, DeviceId), Option<u64>>,
    /// Per-device fences retained for peers an authoritative snapshot removed.
    /// A room-global maximum is invalid because connection generations are
    /// scoped to each peer connection. This map is separately bounded by
    /// `peer_capacity`; overflow switches unknown peers to fail-closed refresh.
    peer_tombstones: HashMap<(SubjectId, DeviceId), Option<u64>>,
    peer_tombstone_overflowed: bool,
    /// A live membership event could not be folded safely into the bounded
    /// roster; the next reconciliation must include an authoritative members
    /// replacement before publishing a settled view.
    presence_refresh_pending: bool,
    /// The subscription anchor from the most recent activation.
    bootstrap_anchor: u64,
    /// The epoch the room last reconciled under, used to stamp in-place views.
    last_epoch: u64,
    /// Whether a reconciliation has ever completed for this room.
    converged_once: bool,
    /// An anchor change requested a replacement bootstrap while another read
    /// was in flight or while the room was parked.
    pending_bootstrap: bool,
    /// A bounded retry for a structurally inconsistent baseline has been spent.
    /// has already been spent. A permanently malformed daemon must park rather
    /// than spin forever.
    protocol_retry_used: bool,
    /// Highest committed position proven by trigger evidence: a daemon-named
    /// cursor or a bounded gap end above the watermark. Authority must reach it
    /// before any view publishes; the clamped read start never encodes it.
    required_frontier: Option<u64>,
    /// The one bounded follow-up read chasing an unreached frontier has run.
    frontier_retry_used: bool,
    /// Strictly-progressing `resync_required` redirects consumed since the
    /// last successful commit (bounded by [`MAX_RESTART_CHAIN`]).
    restart_chain: u32,
    /// The one retry allowed for a non-progressing `resync_required` has run.
    restart_retry_used: bool,
    /// Lowest explicit gap cursor observed while a reconciliation was in flight.
    pending_gap_from: Option<u64>,
    /// Peer/control pushes lost while an in-flight read was superseded. Event
    /// loss is tracked by `parked_buffer`; this scalar covers non-event evidence
    /// that cannot be retained safely across an authoritative snapshot.
    pending_local_dropped: u64,
    /// Quantitative loss already recovered by a stronger authoritative cause,
    /// to surface as a Lagged boundary before its converged view without a
    /// redundant second reconciliation.
    pending_loss_notice: u64,
    /// Events received while a failed reconciliation is parked. This keeps
    /// them bounded and allows the next authoritative baseline to apply them
    /// after its validated prefix instead of silently losing them.
    parked_buffer: ReconcileBuffer,
    phase: Phase,
}

impl RoomState {
    /// A freshly activated room, not yet reconciled.
    pub(crate) fn new(room_id: RoomId, anchor: u64, limits: ReconcileLimits) -> Self {
        Self {
            room_id,
            limits,
            watermark: None,
            timeline: Vec::new(),
            recent_ids: DedupRing::new(limits.dedup_window.max(limits.timeline_depth).max(1)),
            all_event_ids: BaselineSeenIds::new(
                limits.max_baseline_events,
                limits.baseline_dedup_bytes,
            ),
            members: Vec::new(),
            peers: Vec::new(),
            reachability: Reachability::Offline,
            peer_generations: HashMap::new(),
            peer_tombstones: HashMap::new(),
            peer_tombstone_overflowed: false,
            presence_refresh_pending: false,
            bootstrap_anchor: anchor,
            last_epoch: 0,
            converged_once: false,
            pending_bootstrap: false,
            protocol_retry_used: false,
            required_frontier: None,
            frontier_retry_used: false,
            restart_chain: 0,
            restart_retry_used: false,
            pending_gap_from: None,
            pending_local_dropped: 0,
            pending_loss_notice: 0,
            parked_buffer: ReconcileBuffer::new(limits.buffer_depth, limits.buffer_bytes),
            phase: Phase::NeedsReconcile {
                reason: ResyncReason::Bootstrap,
            },
        }
    }

    /// Record trigger evidence of committed history. A daemon-named cursor
    /// (gap `from_pos`, `resync_required` `from_pos`) or a bounded gap end
    /// proves positions at least that high exist, even when the clamped read
    /// start cannot encode it. Fresh, higher evidence re-arms the bounded
    /// frontier chase.
    pub(crate) fn note_trigger_evidence(&mut self, reason: &ResyncReason, from_pos: Option<u64>) {
        if let Some(pos) = from_pos {
            self.note_frontier(pos);
        }
        match reason {
            ResyncReason::Gap {
                to: GapTo::Bounded { pos },
                ..
            } => self.note_frontier(*pos),
            ResyncReason::ResyncRequiredByDaemon { from_pos } => self.note_frontier(*from_pos),
            _ => {}
        }
    }

    fn note_frontier(&mut self, pos: u64) {
        if self.required_frontier.is_none_or(|current| pos > current) {
            self.required_frontier = Some(pos);
            self.frontier_retry_used = false;
        }
    }

    /// Keep the stronger of two causes without erasing quantitative loss:
    /// when a `LocalOverflow` count is outranked in a coalesce, its dropped
    /// count is banked as a pending loss notice and surfaces as an attributed
    /// `Lagged` boundary immediately before the covering converged view.
    /// Cause priority selects the recovery; it never erases how much was lost.
    pub(crate) fn coalesce_banking_loss(
        &mut self,
        existing: Option<ResyncReason>,
        new: ResyncReason,
    ) -> ResyncReason {
        let before =
            overflow_dropped(existing.as_ref()).saturating_add(overflow_dropped(Some(&new)));
        let merged = coalesce(existing, new);
        let after = overflow_dropped(Some(&merged));
        if before > after {
            self.pending_loss_notice = self.pending_loss_notice.saturating_add(before - after);
        }
        merged
    }

    /// Record a fresh activation anchor (a re-`stream.subscribe`'s `from_pos`).
    pub(crate) fn set_anchor(&mut self, anchor: u64) -> bool {
        let changed = self.bootstrap_anchor != anchor;
        self.bootstrap_anchor = anchor;
        changed
    }

    /// Record that the next launch must build a full timeline replacement.
    pub(crate) fn mark_bootstrap_pending(&mut self) {
        self.pending_bootstrap = true;
    }

    pub(crate) fn take_bootstrap_pending(&mut self) -> bool {
        let pending = self.pending_bootstrap;
        self.pending_bootstrap = false;
        pending
    }

    /// The epoch of the outstanding reconciliation, if any.
    pub(crate) fn reconciling_epoch(&self) -> Option<u64> {
        match &self.phase {
            Phase::Reconciling(recon) => Some(recon.epoch),
            _ => None,
        }
    }

    /// The `read_id` of the outstanding read, if any.
    pub(crate) fn outstanding_read_id(&self) -> Option<u64> {
        match &self.phase {
            Phase::Reconciling(recon) => Some(recon.read_id),
            _ => None,
        }
    }

    /// Whether the room is currently reconciling.
    pub(crate) fn is_reconciling(&self) -> bool {
        matches!(self.phase, Phase::Reconciling(_))
    }

    /// Whether this room may publish in-place updates.
    pub(crate) fn is_converged(&self) -> bool {
        matches!(self.phase, Phase::Converged)
    }

    /// Whether this room is parked awaiting a liveness trigger.
    pub(crate) fn is_parked(&self) -> bool {
        matches!(self.phase, Phase::NeedsReconcile { .. })
    }

    /// Whether the room holds a converged timeline (has a watermark).
    pub(crate) fn has_baseline(&self) -> bool {
        self.watermark.is_some()
    }

    /// Preserve live evidence before an in-flight reconciliation is replaced
    /// or parked. Event pushes move into the same bounded parked buffer; peer
    /// pushes cannot be ordered across the next snapshot and are counted as
    /// local loss so presence is authoritatively refreshed.
    fn stash_reconciling_pushes(&mut self) {
        let (events, local_loss, presence, peer_pushes) = match &mut self.phase {
            Phase::Reconciling(recon) => {
                // `drain` resets the loss latch, so capture it first. A stronger
                // coalesced cause may outrank LocalOverflow, but must never erase
                // the fact that a pushed event was dropped.
                let event_loss = recon.buffer.dropped();
                let events = recon.buffer.drain();
                let peer_pushes: Vec<_> = recon.peer_buffer.drain(..).collect();
                let peer_loss = recon.peer_dropped.saturating_add(peer_pushes.len() as u64);
                recon.peer_buffer_bytes = 0;
                recon.peer_dropped = 0;
                (
                    events,
                    event_loss.saturating_add(peer_loss),
                    recon.presence,
                    peer_pushes,
                )
            }
            _ => return,
        };
        for event in events {
            let _ = self.parked_buffer.push(event);
        }
        for push in peer_pushes {
            self.fence_discarded_peer(push);
        }
        self.pending_local_dropped = self.pending_local_dropped.saturating_add(local_loss);
        if presence || local_loss > 0 || self.parked_buffer.dropped() > 0 {
            self.presence_refresh_pending = true;
        }
    }

    /// Enter a new kernel transport epoch. Preserve event/cause evidence from
    /// an old in-flight read, then forget peer payload counters: K7 already
    /// fences old transport frames and a restarted daemon may restart counters.
    pub(crate) fn reset_peer_transport_epoch(&mut self) {
        self.stash_reconciling_pushes();
        self.peer_generations.clear();
        self.peer_tombstones.clear();
        self.peer_tombstone_overflowed = false;
    }

    /// Park the outstanding reconciliation without consuming its reply. This is
    /// used when old I/O settles after lifecycle left `Ready`: no follow-up read
    /// may be issued into the dead transport, but its cause and buffered pushes
    /// must remain available for the next liveness-triggered launch.
    pub(crate) fn park_outstanding(&mut self) {
        self.stash_reconciling_pushes();
        let taken = match &mut self.phase {
            Phase::Reconciling(recon) => (recon.rerun.take(), recon.reason.clone()),
            // Already parked (the cause is retained in place) or converged
            // (nothing outstanding): nothing to fold.
            Phase::NeedsReconcile { .. } | Phase::Converged => return,
        };
        let reason = self.coalesce_banking_loss(taken.0, taken.1);
        self.phase = Phase::NeedsReconcile { reason };
    }

    /// Begin a reconciliation under `epoch`/`read_id` with the given cause. An
    /// incremental reconciliation resyncs from the current `watermark`; a
    /// bootstrap builds from `room.timeline`. Bootstrap always reads presence;
    /// an incremental reads presence only when the cause implicates it (§R2).
    /// Returns the first read to issue.
    pub(crate) fn begin_reconcile(
        &mut self,
        epoch: u64,
        read_id: u64,
        reason: ResyncReason,
        from_pos_override: Option<u64>,
    ) -> ReadRequest {
        // A superseding launch must not destroy pushes held by the old read.
        self.stash_reconciling_pushes();
        let preexisting_loss = self
            .parked_buffer
            .dropped()
            .saturating_add(self.pending_local_dropped);
        self.pending_local_dropped = 0;
        let parked_events = self.parked_buffer.drain();

        // A daemon-named `from_pos` means "discard back to `from_pos` and
        // re-read". A cursor above our watermark cannot prove that we hold the
        // intervening prefix, so clamp it; before a first baseline it is
        // ignored and the full genesis bootstrap is used instead.
        let from_pos_override = from_pos_override
            .and_then(|from_pos| self.watermark.map(|watermark| from_pos.min(watermark)));
        if let Some(from_pos) = from_pos_override {
            self.truncate_to(from_pos);
        }
        let replace_timeline = self.watermark.is_none()
            || (matches!(reason, ResyncReason::Bootstrap) && from_pos_override.is_none())
            || self.pending_bootstrap;
        let incremental = !replace_timeline
            && self.watermark.is_some()
            && (from_pos_override.is_some() || !matches!(reason, ResyncReason::Bootstrap));
        if replace_timeline {
            self.pending_bootstrap = false;
        }
        let presence =
            reason.implicates_presence() || self.presence_refresh_pending || preexisting_loss > 0;
        self.presence_refresh_pending = false;
        let outstanding = if incremental {
            let from_pos = from_pos_override.unwrap_or_else(|| self.watermark.unwrap_or(0));
            ReadStep::Resync { from_pos }
        } else {
            ReadStep::Timeline {
                cursor: Cursor::Start,
                anchor: Some(self.bootstrap_anchor),
            }
        };
        let baseline_start = match &outstanding {
            ReadStep::Timeline { .. } => 0,
            ReadStep::Resync { from_pos } => *from_pos,
            ReadStep::Members | ReadStep::Peers => 0,
        };
        let mut plan = VecDeque::new();
        if !incremental || presence {
            plan.push_back(ReadStep::Members);
            plan.push_back(ReadStep::Peers);
        }
        let request = self.build_request(&outstanding);
        let mut buffer = ReconcileBuffer::new(self.limits.buffer_depth, self.limits.buffer_bytes);
        for event in parked_events {
            // The parked buffer used the same limits, so every retained event
            // fits when moved into this fresh reconciliation buffer.
            let _ = buffer.push(event);
        }
        let mut baseline_ids = if incremental {
            self.recent_ids.clone()
        } else {
            DedupRing::new(
                self.limits
                    .dedup_window
                    .max(self.limits.timeline_depth)
                    .max(1),
            )
        };
        baseline_ids.cap = baseline_ids
            .cap
            .max(self.limits.timeline_depth as usize)
            .max(1);
        self.last_epoch = epoch;
        self.phase = Phase::Reconciling(Box::new(Reconciling {
            epoch,
            reason,
            read_id,
            outstanding,
            plan,
            // Do not reserve from an untrusted/public u32 limit up front;
            // the tail grows only as validated events are admitted.
            events: Vec::new(),
            baseline_ids,
            baseline_seen_ids: BaselineSeenIds::new(
                self.limits.max_baseline_events,
                self.limits.baseline_dedup_bytes,
            ),
            baseline_seen: false,
            baseline_has_event: false,
            scan_pos: baseline_start,
            baseline_last: baseline_start,
            baseline_start,
            replace_timeline,
            presence,
            suppress_view: preexisting_loss > 0,
            event_cap: self.limits.timeline_depth as usize,
            event_bytes: 0,
            event_bytes_cap: self.limits.timeline_bytes,
            baseline_membership: false,
            buffer,
            peer_buffer: VecDeque::new(),
            peer_buffer_bytes: 0,
            peer_dropped: 0,
            rerun: (preexisting_loss > 0).then_some(ResyncReason::LocalOverflow {
                dropped: preexisting_loss,
            }),
            members: None,
            peers: None,
        }));
        request
    }

    /// Build the typed request for one read step.
    fn build_request(&self, step: &ReadStep) -> ReadRequest {
        match step {
            ReadStep::Timeline { cursor, .. } => ReadRequest::Timeline(RoomTimeline {
                room_id: self.room_id.clone(),
                page: Page {
                    cursor: cursor.clone(),
                    direction: Direction::Forward,
                    limit: self
                        .limits
                        .read_page_size
                        .min(u64::from(self.limits.max_read_page_events)),
                },
            }),
            ReadStep::Resync { from_pos } => ReadRequest::Resync(StreamResync {
                room_id: self.room_id.clone(),
                from_pos: *from_pos,
            }),
            ReadStep::Members => ReadRequest::Members(RoomMembers {
                room_id: self.room_id.clone(),
            }),
            ReadStep::Peers => ReadRequest::Peers(RoomPeers {
                room_id: self.room_id.clone(),
            }),
        }
    }

    /// The request for the currently-outstanding step (immutable).
    fn issue_outstanding(&self) -> ReadRequest {
        match &self.phase {
            Phase::Reconciling(recon) => self.build_request(&recon.outstanding),
            // Only ever called while reconciling; a benign fallback keeps this
            // total without an `unwrap`.
            _ => ReadRequest::Members(RoomMembers {
                room_id: self.room_id.clone(),
            }),
        }
    }

    /// Feed a reply for the outstanding read. The caller has already fenced by
    /// epoch and `read_id`, so this reply is the current reconciliation's.
    pub(crate) fn on_read_reply(&mut self, reply: ReadReply) -> ReplyOutcome {
        if !self.is_reconciling() {
            return ReplyOutcome::Failed {
                err: CallError::Local(LocalError::Backend),
                rerun: None,
            };
        }
        // Mutate the outstanding reconciliation in a scoped borrow, deciding
        // what to do next; the borrow ends before any `&mut self` method call.
        let next = {
            let Phase::Reconciling(recon) = &mut self.phase else {
                return ReplyOutcome::Failed {
                    err: CallError::Local(LocalError::Backend),
                    rerun: None,
                };
            };
            match reply {
                ReadReply::Timeline(Ok(out))
                    if out.room_id != self.room_id
                        || !reply_events_valid(
                            &out.events,
                            self.limits
                                .read_page_size
                                .min(u64::from(self.limits.max_read_page_events)),
                            self.limits,
                        ) =>
                {
                    Next::Gap(protocol_gap())
                }
                ReadReply::Timeline(Ok(out)) => {
                    let anchor = match &recon.outstanding {
                        ReadStep::Timeline { anchor, .. } => *anchor,
                        _ => None,
                    };
                    // The `stream.subscribe.from_pos` anchor is a concrete,
                    // inclusive completeness floor, including zero (the
                    // genesis-only room head). The baseline reads to
                    // `Complete` and retains every validated event — an event
                    // committed after the anchor but before this read (for
                    // example a push that raced the room's activation and was
                    // dropped while the room was untracked) has no other
                    // recovery path. Termination is bounded by the total-scan
                    // ceiling and the cursor checks, never by daemon trust.
                    let mut valid = true;
                    let page_empty = out.events.is_empty();
                    for event in out.events {
                        let is_room_created = matches!(
                            &event.kind,
                            jeliya_api::EventKindContent::RoomCreated { .. }
                        );
                        // A room has exactly one origin. A later RoomCreated is
                        // structurally invalid even when its position is dense.
                        if recon.baseline_seen && is_room_created {
                            valid = false;
                            break;
                        }
                        if recon.baseline_ids.contains(&event.event_id)
                            || !recon
                                .baseline_seen_ids
                                .insert_new(&event.event_id, event.pos)
                        {
                            valid = false;
                            break;
                        }
                        recon.baseline_ids.insert(event.event_id.clone());
                        let first_timeline_event = recon.replace_timeline && !recon.baseline_seen;
                        let valid_position = if first_timeline_event {
                            event.pos == 0 && is_room_created
                        } else {
                            recon.scan_pos.checked_add(1) == Some(event.pos)
                        };
                        if !valid_position {
                            valid = false;
                            break;
                        }
                        recon.baseline_seen = true;
                        recon.scan_pos = event.pos;
                        if is_membership_event(&event) {
                            recon.baseline_membership = true;
                        }
                        recon.baseline_has_event = true;
                        recon.baseline_last = recon.scan_pos;
                        retain_baseline_event(recon, event);
                    }
                    let continuation = more_cursor(&out.truncated);
                    let invalid_cursor = continuation
                        .as_ref()
                        .is_some_and(|cursor| !timeline_cursor_is_next(cursor, recon.scan_pos));
                    if (!valid)
                        || invalid_cursor
                        || (page_empty && matches!(out.truncated, Truncated::More { .. }))
                        || (page_empty && recon.replace_timeline)
                    {
                        Next::Gap(protocol_gap())
                    } else {
                        match continuation {
                            Some(cursor) => {
                                recon.outstanding = ReadStep::Timeline { cursor, anchor };
                                Next::Issue
                            }
                            // A complete history that never reached the anchor
                            // is missing a committed event (density from the
                            // genesis guarantees the anchor position itself was
                            // served iff the scan reached it); never publish
                            // the partial prefix.
                            None if anchor.is_some_and(|anchor| recon.scan_pos < anchor) => {
                                Next::Gap(protocol_gap())
                            }
                            None => Next::Advance,
                        }
                    }
                }
                ReadReply::Resync(Ok(out))
                    if out.room_id != self.room_id
                        || !reply_events_valid(
                            &out.events,
                            u64::from(self.limits.max_read_page_events),
                            self.limits,
                        ) =>
                {
                    Next::Gap(protocol_gap())
                }
                ReadReply::Resync(Ok(out)) => {
                    let page_start = recon.scan_pos;
                    let mut valid = true;
                    for event in out.events {
                        // RoomCreated is the unique position-zero origin and
                        // can never appear in an incremental recovery suffix.
                        if matches!(
                            &event.kind,
                            jeliya_api::EventKindContent::RoomCreated { .. }
                        ) {
                            valid = false;
                            break;
                        }
                        if self.all_event_ids.contains(&event.event_id)
                            || recon.baseline_ids.contains(&event.event_id)
                            || !recon
                                .baseline_seen_ids
                                .insert_new(&event.event_id, event.pos)
                        {
                            valid = false;
                            break;
                        }
                        recon.baseline_ids.insert(event.event_id.clone());
                        let Some(expected) = recon.scan_pos.checked_add(1) else {
                            valid = false;
                            break;
                        };
                        if event.pos == 0 || event.pos != expected {
                            valid = false;
                            break;
                        }
                        recon.scan_pos = event.pos;
                        if is_membership_event(&event) {
                            recon.baseline_membership = true;
                        }
                        recon.baseline_has_event = true;
                        recon.baseline_last = recon.scan_pos;
                        retain_baseline_event(recon, event);
                    }
                    if !valid || out.next_pos != recon.scan_pos {
                        Next::Gap(protocol_gap())
                    } else if matches!(out.truncated, Truncated::More { .. }) {
                        // `next_pos` is the exclusive recovery cursor. An empty
                        // page or contradictory cursor must not spin or skip.
                        let valid_cursor = matches!(
                            out.truncated,
                            Truncated::More {
                                cursor: Cursor::At { pos }
                            } if pos == out.next_pos
                        );
                        if out.next_pos == page_start || !valid_cursor {
                            Next::Gap(protocol_gap())
                        } else {
                            recon.outstanding = ReadStep::Resync {
                                from_pos: out.next_pos,
                            };
                            Next::Issue
                        }
                    } else {
                        Next::Advance
                    }
                }
                ReadReply::Resync(Err(err)) => match resync_required_from(&err, &self.room_id) {
                    Ok(Some(from_pos)) => Next::Restart(from_pos),
                    Ok(None) => Next::Fail(err),
                    Err(()) => Next::Gap(protocol_gap()),
                },
                ReadReply::Members(Ok(out)) if out.room_id != self.room_id => {
                    Next::Gap(protocol_gap())
                }
                ReadReply::Members(Ok(out)) => {
                    if !members_snapshot_valid(
                        &out.members,
                        self.limits.member_capacity as usize,
                        self.limits,
                    ) {
                        // Refuse an oversized authoritative snapshot rather
                        // than publish partial truth. Park until a later
                        // liveness trigger; unconditional retry would spin.
                        Next::Fail(CallError::Local(LocalError::Backend))
                    } else {
                        recon.members = Some(out.members);
                        Next::Advance
                    }
                }
                ReadReply::Peers(Ok(out)) if out.room_id != self.room_id => {
                    Next::Gap(protocol_gap())
                }
                ReadReply::Peers(Ok(out)) => {
                    if !peers_snapshot_valid(
                        &out.peers,
                        self.limits.peer_capacity as usize,
                        self.limits,
                    ) {
                        // Refuse an oversized authoritative snapshot rather
                        // than publish partial truth. Park until a later
                        // liveness trigger; unconditional retry would spin.
                        Next::Fail(CallError::Local(LocalError::Backend))
                    } else {
                        recon.peers = Some((out.reachability, out.peers));
                        Next::Advance
                    }
                }
                ReadReply::Timeline(Err(err))
                | ReadReply::Members(Err(err))
                | ReadReply::Peers(Err(err)) => Next::Fail(err),
            }
        };
        match next {
            Next::Issue => ReplyOutcome::NextRead(self.issue_outstanding()),
            Next::Advance => self.advance(),
            Next::Restart(from_pos) => self.bounded_restart(from_pos),
            Next::Fail(err) => self.fail(err),
            Next::Gap(_reason) => self.structural_retry(),
        }
    }

    /// Honour a daemon `resync_required` redirect only while the chain makes
    /// strict progress within a bounded length. The effective re-read cursor is
    /// the clamped `from_pos`; when it equals the failed read's own start the
    /// daemon is redirecting in place, which gets exactly one retry before the
    /// room parks — repeated identical or nondecreasing cursors must not issue
    /// reads forever, and even a strictly-descending chain is bounded.
    fn bounded_restart(&mut self, from_pos: u64) -> ReplyOutcome {
        let baseline_start = match &self.phase {
            Phase::Reconciling(recon) => recon.baseline_start,
            _ => 0,
        };
        let effective = self
            .watermark
            .map_or(from_pos, |watermark| from_pos.min(watermark));
        let exhausted = if effective < baseline_start {
            self.restart_chain = self.restart_chain.saturating_add(1);
            self.restart_chain > MAX_RESTART_CHAIN
        } else if self.restart_retry_used {
            true
        } else {
            self.restart_retry_used = true;
            false
        };
        if exhausted {
            // Preserve the daemon's discard cursor in the parked cause; no
            // watermark encodes it, and losing it would leave repudiated
            // positions in the timeline forever (§R9).
            self.coalesce_rerun(
                ResyncReason::ResyncRequiredByDaemon { from_pos },
                Some(from_pos),
            );
            self.fail_inner(CallError::Local(LocalError::Backend), false)
        } else {
            ReplyOutcome::Restart {
                reason: ResyncReason::ResyncRequiredByDaemon { from_pos },
                from_pos,
            }
        }
    }

    /// Pop the next planned read (or converge when the plan is exhausted).
    fn advance(&mut self) -> ReplyOutcome {
        let popped = match &mut self.phase {
            Phase::Reconciling(recon) => recon.plan.pop_front(),
            _ => None,
        };
        match popped {
            Some(next) => {
                if let Phase::Reconciling(recon) = &mut self.phase {
                    recon.outstanding = next;
                }
                ReplyOutcome::NextRead(self.issue_outstanding())
            }
            None => self.converge(),
        }
    }

    /// Park the room carrying the coalesced cause and report the failure.
    ///
    /// The outstanding reconciliation's own cause and any re-trigger coalesced
    /// into it while the read was in flight are folded into the parked reason,
    /// so neither is lost with the dropped `Reconciling` box. The stronger cause
    /// wins (§R9), which keeps a daemon-named discard position addressable.
    fn fail(&mut self, err: CallError) -> ReplyOutcome {
        self.fail_inner(err, true)
    }

    /// Park after a failed settle. `relaunch_accrued` is false for the exhausted
    /// structural-retry budget: accrued causes remain in the parked state but
    /// cannot turn a permanently malformed reply into an immediate hot loop.
    fn fail_inner(&mut self, err: CallError, relaunch_accrued: bool) -> ReplyOutcome {
        self.stash_reconciling_pushes();
        let local_loss = self
            .parked_buffer
            .dropped()
            .saturating_add(self.pending_local_dropped);
        let (reason, rerun) = match &mut self.phase {
            Phase::Reconciling(recon) => {
                if recon.presence {
                    self.presence_refresh_pending = true;
                }
                (recon.reason.clone(), recon.rerun.take())
            }
            _ => (ResyncReason::Reconnect, None),
        };
        let had_rerun = rerun.is_some();
        let mut parked = match rerun {
            // The rerun's count exists nowhere else, so bank it if outranked.
            Some(rerun) => self.coalesce_banking_loss(Some(reason), rerun),
            None => reason,
        };
        if local_loss > 0 {
            // Deliberately unbanked: `local_loss` is re-derived from the
            // still-latched parked-buffer and pending-drop counters at the
            // next launch, so banking it here would double-count.
            parked = parked.coalesce(ResyncReason::LocalOverflow {
                dropped: local_loss,
            });
        }
        self.phase = Phase::NeedsReconcile {
            reason: parked.clone(),
        };
        // Relaunch only when a re-trigger actually accrued: an unprovoked retry
        // on every failure would auto-spin against a failing daemon.
        ReplyOutcome::Failed {
            err,
            rerun: if relaunch_accrued && had_rerun {
                Some(parked)
            } else {
                None
            },
        }
    }

    /// Take the cause that is pending but not yet acted on, if any.
    ///
    /// Two cases carry evidence a fresh launch would otherwise destroy (§R9):
    /// - **parked** — a previous read failed, so its cause was never satisfied;
    /// - **superseded** — a reconciliation is in flight and has a coalesced
    ///   `rerun` that has not run yet; replacing the phase drops it.
    ///
    /// The in-flight reconciliation's own `reason` is deliberately *not*
    /// returned: it is already being acted on, and re-folding it would re-apply
    /// a discard position the outstanding read had already honoured.
    pub(crate) fn take_pending_cause(&mut self) -> Option<ResyncReason> {
        match &mut self.phase {
            Phase::NeedsReconcile { reason } => Some(reason.clone()),
            Phase::Reconciling(recon) => recon.rerun.take(),
            Phase::Converged => None,
        }
    }

    /// Commit the accumulated baseline and buffered pushes into the durable
    /// timeline, replace presence/membership, and produce the converged view.
    fn converge(&mut self) -> ReplyOutcome {
        // A changed subscription anchor supersedes this entire baseline. Its
        // replacement mode is independent of the observable cause (a queued
        // Gap/Reconnect may outrank Bootstrap), so do not publish or commit the
        // old-anchor view. Preserve buffered evidence and immediately launch
        // the one coalesced replacement instead.
        if self.pending_bootstrap {
            return self.retry_pending_bootstrap();
        }

        // A non-zero resync cursor cannot establish a first local baseline: the
        // prefix before it has never been read by this client. Park instead of
        // fabricating a watermark or publishing a partial view.
        let invalid_first_cursor = matches!(
            &self.phase,
            Phase::Reconciling(recon)
                if !self.converged_once && recon.baseline_start != 0
        );
        if invalid_first_cursor {
            return self.structural_retry();
        }
        // Validate every condition that can structurally retry before taking
        // buffered pushes, peer evidence, causes, snapshots, or scan ids out of
        // the reconciliation. A retry must be able to stash all of them.
        let invalid_commit = match &self.phase {
            Phase::Reconciling(recon) if !recon.baseline_has_event => recon.replace_timeline,
            Phase::Reconciling(recon) => {
                let start_matches = recon.replace_timeline
                    || self
                        .watermark
                        .map_or(recon.baseline_start == 0, |watermark| {
                            watermark == recon.baseline_start
                        });
                !start_matches
                    || (!recon.replace_timeline
                        && !self
                            .all_event_ids
                            .can_merge_through(&recon.baseline_seen_ids, recon.baseline_last))
            }
            _ => true,
        };
        if invalid_commit {
            return self.structural_retry();
        }

        let in_flight_event_loss = match &self.phase {
            Phase::Reconciling(recon) => recon.buffer.dropped(),
            _ => 0,
        };
        let scan_count_cap = self.limits.max_baseline_events;
        let scan_bytes_cap = self.limits.baseline_dedup_bytes;
        let (
            mut rerun,
            baseline,
            mut baseline_seen_ids,
            baseline_has_event,
            baseline_last,
            baseline_membership,
            mut buffered,
            members,
            peers,
            mut pending_peers,
            peer_dropped,
            epoch,
            replace_timeline,
            mut suppress_view,
        ) = match &mut self.phase {
            Phase::Reconciling(recon) => (
                recon.rerun.take(),
                std::mem::take(&mut recon.events),
                std::mem::replace(
                    &mut recon.baseline_seen_ids,
                    BaselineSeenIds::new(scan_count_cap, scan_bytes_cap),
                ),
                recon.baseline_has_event,
                recon.baseline_last,
                recon.baseline_membership,
                recon.buffer.drain(),
                recon.members.take(),
                recon.peers.take(),
                std::mem::take(&mut recon.peer_buffer),
                std::mem::take(&mut recon.peer_dropped),
                recon.epoch,
                recon.replace_timeline,
                recon.suppress_view,
            ),
            _ => {
                return ReplyOutcome::Failed {
                    err: CallError::Local(LocalError::Backend),
                    rerun: None,
                }
            }
        };

        if in_flight_event_loss > 0 {
            self.presence_refresh_pending = true;
            rerun = Some(self.coalesce_banking_loss(
                rerun,
                ResyncReason::LocalOverflow {
                    dropped: in_flight_event_loss,
                },
            ));
        }

        // 1. Apply the authoritative baseline first (§R6.1). The page scanner
        // already proved density from `baseline_start`; only its bounded tail is
        // retained, while `baseline_last` advances across every page.
        baseline_seen_ids.retain_through(baseline_last);
        if baseline_has_event {
            if replace_timeline {
                self.all_event_ids = baseline_seen_ids;
            } else {
                debug_assert!(self.all_event_ids.can_merge(&baseline_seen_ids));
                self.all_event_ids.merge(baseline_seen_ids);
            }
            if replace_timeline {
                self.timeline.clear();
                self.watermark = None;
                self.recent_ids = DedupRing::new(
                    self.limits
                        .dedup_window
                        .max(self.limits.timeline_depth)
                        .max(1),
                );
            }
            for event in &baseline {
                self.apply_membership_event(event);
                self.recent_ids.insert(event.event_id.clone());
            }
            self.timeline.extend(baseline);
            self.trim_timeline();
            self.watermark = Some(baseline_last);
        }
        // 2. Replace presence/membership wholesale (§R8). Oversized snapshots
        // are rejected before this method, never silently truncated. If a
        // membership event appeared in an events-only baseline, its older
        // pages may have fallen out of the render tail; require an authoritative
        // roster before publishing that state.
        if baseline_membership {
            self.presence_refresh_pending = true;
        }
        let has_peer_snapshot = peers.is_some();
        if let Some(members) = members {
            self.members = members;
            self.presence_refresh_pending = false;
        }
        if let Some((reachability, peers)) = peers {
            self.reachability = reachability;
            self.replace_peer_snapshot(peers);
        }

        // Peer pushes buffered before an authoritative peers snapshot have no
        // snapshot-relative order. Applying them could overwrite newer
        // authority, so discard them into bounded per-device fences and schedule
        // a follow-up presence read. For an events-only reconciliation there is
        // no new snapshot boundary; apply them using the durable device fence.
        if has_peer_snapshot && !pending_peers.is_empty() {
            let dropped = pending_peers.len() as u64;
            for push in pending_peers.drain(..) {
                self.fence_discarded_peer(push);
            }
            self.presence_refresh_pending = true;
            rerun =
                Some(self.coalesce_banking_loss(rerun, ResyncReason::LocalOverflow { dropped }));
        } else if !has_peer_snapshot {
            for push in pending_peers.drain(..) {
                if matches!(
                    self.apply_peer_push_now(
                        push.subject_id,
                        push.device_id,
                        push.link,
                        push.generation,
                    ),
                    PeerOutcome::Overflow
                ) {
                    self.presence_refresh_pending = true;
                    rerun =
                        Some(self.coalesce_banking_loss(
                            rerun,
                            ResyncReason::LocalOverflow { dropped: 1 },
                        ));
                }
            }
        }
        if peer_dropped > 0 {
            self.presence_refresh_pending = true;
            rerun = Some(self.coalesce_banking_loss(
                rerun,
                ResyncReason::LocalOverflow {
                    dropped: peer_dropped,
                },
            ));
        }

        // 3. Drain live pushes after the authoritative baseline. Ordinarily
        // parked pushes were moved into `recon.buffer` at launch; capture any
        // late loss defensively before drain resets its accounting.
        let late_parked_loss = self.parked_buffer.dropped();
        buffered.extend(self.parked_buffer.drain());
        if late_parked_loss > 0 {
            suppress_view = true;
            self.presence_refresh_pending = true;
            rerun = Some(self.coalesce_banking_loss(
                rerun,
                ResyncReason::LocalOverflow {
                    dropped: late_parked_loss,
                },
            ));
        }
        buffered.sort_by_key(|event| event.pos);
        if let Some(gap) = self.apply_ordered(buffered) {
            // The gap may outrank an accumulated LocalOverflow; banking keeps
            // its count observable at the eventual publication boundary.
            rerun = Some(self.coalesce_banking_loss(rerun, gap));
        }
        let needs_presence_refresh = self.presence_refresh_pending;
        if needs_presence_refresh {
            // Preserve a more specific loss cause already coalesced (notably
            // LocalOverflow); the pending presence flag itself forces the next
            // launch to include Members/Peers.
            rerun = Some(rerun.unwrap_or_else(protocol_gap));
        }

        self.converged_once = true;
        self.protocol_retry_used = false;
        self.restart_chain = 0;
        self.restart_retry_used = false;
        self.pending_bootstrap = false;
        self.last_epoch = epoch;
        self.phase = Phase::Converged;
        // Any accrued trigger is a publication barrier. The just-validated
        // prefix may be useful durable progress, but a Gap/daemon cursor may
        // repudiate it and LocalOverflow may mean it is incomplete; never emit
        // that intermediate state before the authoritative rerun settles.
        // Quantitative loss outranked in any of the coalesces above was banked
        // into `pending_loss_notice` at that site, so nothing is erased here.
        if let Some(reason) = rerun {
            return ReplyOutcome::Retry { reason };
        }
        if needs_presence_refresh || suppress_view {
            return ReplyOutcome::Retry {
                reason: protocol_gap(),
            };
        }
        // A trigger proved committed history beyond what authority served.
        // The validated prefix is durable progress, but publishing it would
        // present a converged view below a frontier the daemon itself named:
        // chase the frontier once, then fail closed rather than publish short.
        if let Some(frontier) = self.required_frontier {
            if self.watermark.is_none_or(|watermark| watermark < frontier) {
                let reason = ResyncReason::Gap {
                    reason: GapReason::SubscriptionLapse,
                    to: GapTo::Bounded { pos: frontier },
                };
                if !self.frontier_retry_used {
                    self.frontier_retry_used = true;
                    return ReplyOutcome::Retry { reason };
                }
                self.phase = Phase::NeedsReconcile { reason };
                return ReplyOutcome::Failed {
                    err: CallError::Local(LocalError::Backend),
                    rerun: None,
                };
            }
            self.required_frontier = None;
            self.frontier_retry_used = false;
        }
        // A discard below the render window with a short authoritative suffix
        // can leave nothing renderable while the room still has history.
        // Publishing an empty timeline would misreport the room as empty;
        // rebuild the window with a full timeline replacement instead. A
        // replacement is exempt: its window is policy-conformant by
        // construction, so this cannot loop.
        if !replace_timeline && self.timeline.is_empty() && self.watermark.is_some() {
            self.pending_bootstrap = true;
            return ReplyOutcome::Retry {
                reason: protocol_gap(),
            };
        }
        ReplyOutcome::Converged {
            view: self.view(epoch),
            rerun: None,
            dropped: std::mem::take(&mut self.pending_loss_notice),
        }
    }

    /// Apply a `pos`-ascending run of events to the timeline, deduplicating by
    /// watermark and by recent id, requiring density. Returns a gap cause when a
    /// hole is found (the contiguous prefix is applied; the rest forces a
    /// resync).
    fn apply_ordered(&mut self, events: Vec<Event>) -> Option<ResyncReason> {
        let mut gap: Option<ResyncReason> = None;
        for event in events {
            if !event_identifiers_within(&event, self.limits.max_identifier_bytes)
                || estimated_event_bytes(&event) > self.limits.timeline_bytes
            {
                gap = Some(protocol_gap());
                break;
            }
            if event.pos != 0
                && matches!(
                    &event.kind,
                    jeliya_api::EventKindContent::RoomCreated { .. }
                )
            {
                gap = Some(protocol_gap());
                break;
            }
            match self.watermark {
                Some(watermark) if event.pos <= watermark => {
                    // An exact identity/position replay is harmless. Unknown or
                    // position-conflicting evidence below the watermark
                    // contradicts the committed history even after render-tail
                    // eviction. The committed claimant is itself unproven, so
                    // recovery must re-read the disputed position, not merely
                    // the suffix after it.
                    if self.all_event_ids.position(&event.event_id) != Some(event.pos)
                        || self.retained_position_conflicts(&event)
                    {
                        self.dispute_position(event.pos);
                        gap = Some(protocol_gap());
                        break;
                    }
                    continue;
                }
                Some(watermark)
                    if event.pos != watermark.saturating_add(1)
                        || self.recent_ids.contains(&event.event_id)
                        || self.all_event_ids.contains(&event.event_id) =>
                {
                    // A duplicate identity above the watermark is not harmless:
                    // it cannot advance the watermark, so accepting it would
                    // leave the next position unexplained. Treat it like any
                    // other density violation.
                    gap = Some(ResyncReason::Gap {
                        reason: GapReason::SubscriptionLapse,
                        to: GapTo::Open,
                    });
                    break;
                }
                None if event.pos == 1 => {
                    if !self.all_event_ids.insert_new(&event.event_id, event.pos) {
                        gap = Some(protocol_gap());
                        break;
                    }
                    self.append(event);
                }
                None => {
                    // A baseline cannot begin at a later position: the prefix
                    // has never been read and must be recovered authoritatively.
                    gap = Some(ResyncReason::Gap {
                        reason: GapReason::SubscriptionLapse,
                        to: GapTo::Open,
                    });
                    break;
                }
                Some(_) => {
                    if !self.all_event_ids.insert_new(&event.event_id, event.pos) {
                        gap = Some(protocol_gap());
                        break;
                    }
                    self.append(event);
                }
            }
        }
        gap
    }

    fn retained_position_conflicts(&self, event: &Event) -> bool {
        self.timeline
            .iter()
            .find(|known| known.pos == event.pos)
            .is_some_and(|known| known != event)
    }

    /// Require the next launch to discard back to before `pos`: contradictory
    /// evidence at a committed position means the committed claimant is itself
    /// unproven, and only a re-read covering `pos` can settle which event
    /// authority actually holds there. A disputed genesis can only be re-proven
    /// by a full timeline replacement.
    fn dispute_position(&mut self, pos: u64) {
        match pos.checked_sub(1) {
            Some(before) => {
                self.pending_gap_from = Some(
                    self.pending_gap_from
                        .map_or(before, |current| current.min(before)),
                );
            }
            None => self.pending_bootstrap = true,
        }
    }

    /// Discard the timeline back to `from_pos` (drop every event with a greater
    /// position) and rebuild the watermark and the dedup ring from the retained
    /// tail. This is the "discard back to `from_pos`" a `resync_required`
    /// requires; without rebuilding the dedup ring, the re-read events could be
    /// wrongly skipped as duplicates, re-opening the hole.
    fn truncate_to(&mut self, from_pos: u64) {
        let Some(watermark) = self.watermark else {
            return;
        };
        if from_pos >= watermark {
            return;
        }
        // The discarded suffix may have carried membership events already
        // folded into the derived roster. Rolling back the timeline cannot
        // reverse that fold, so only an authoritative `room.members`
        // replacement can repair it before the next published view (§R8).
        self.presence_refresh_pending = true;
        self.timeline.retain(|event| event.pos <= from_pos);
        self.all_event_ids.retain_through(from_pos);
        // `from_pos` is the daemon's authoritative discard cursor even when
        // the finite render window no longer retains the event at that exact
        // position. Preserve the cursor so the next page at `from_pos + 1`
        // remains contiguous instead of looking like a new, impossible genesis.
        self.watermark = Some(from_pos);
        // Rebuild the recent-id ring from the retained tail so a duplicate of a
        // retained event is still caught, while a re-read event beyond `from_pos`
        // is not.
        let window = self
            .limits
            .dedup_window
            .max(self.limits.timeline_depth)
            .max(1) as usize;
        let start = self.timeline.len().saturating_sub(window);
        let retained_ids: Vec<EventId> = self.timeline[start..]
            .iter()
            .map(|event| event.event_id.clone())
            .collect();
        self.recent_ids = DedupRing::new(
            self.limits
                .dedup_window
                .max(self.limits.timeline_depth)
                .max(1),
        );
        for id in retained_ids {
            self.recent_ids.insert(id);
        }
    }

    /// Append one authoritative event, advancing the watermark and recording its
    /// id. Only ever called on an event that passed dedup and density.
    fn append(&mut self, event: Event) {
        self.apply_membership_event(&event);
        self.watermark = Some(event.pos);
        self.recent_ids.insert(event.event_id.clone());
        self.timeline.push(event);
        self.trim_timeline();
    }

    /// Fold signed membership events into the live roster between authoritative
    /// `room.members` reads. Join carries signed role/timestamp evidence;
    /// leave/remove only change an already-known row. An unknown target or a
    /// full bounded roster requests an authoritative replacement instead of
    /// silently leaving stale UI state.
    fn apply_membership_event(&mut self, event: &Event) {
        match &event.kind {
            jeliya_api::EventKindContent::MemberJoined { subject_id, role } => {
                if let Some(member) = self
                    .members
                    .iter_mut()
                    .find(|member| member.subject_id == *subject_id)
                {
                    let was_terminal =
                        matches!(member.standing, Standing::Left | Standing::Removed);
                    member.role = *role;
                    member.standing = Standing::Active;
                    if was_terminal {
                        // A re-join is a new signed membership fact; preserve
                        // the original date only for an already-active row.
                        member.joined_at = event.at;
                    }
                } else if self.members.len() < self.limits.member_capacity as usize {
                    self.members.push(MemberRow {
                        subject_id: subject_id.clone(),
                        role: *role,
                        standing: Standing::Active,
                        joined_at: event.at,
                    });
                } else {
                    self.presence_refresh_pending = true;
                }
            }
            jeliya_api::EventKindContent::MemberLeft { subject_id } => {
                if let Some(member) = self
                    .members
                    .iter_mut()
                    .find(|member| member.subject_id == *subject_id)
                {
                    member.standing = Standing::Left;
                } else {
                    self.presence_refresh_pending = true;
                }
            }
            jeliya_api::EventKindContent::MemberRemoved { subject_id, .. } => {
                if let Some(member) = self
                    .members
                    .iter_mut()
                    .find(|member| member.subject_id == *subject_id)
                {
                    member.standing = Standing::Removed;
                } else {
                    self.presence_refresh_pending = true;
                }
            }
            _ => {}
        }
    }

    /// Retain only the newest configured view window. The watermark and
    /// recent-id ring intentionally survive eviction: ordering and dedup do not
    /// depend on the UI's finite render history.
    fn trim_timeline(&mut self) {
        let count_cap = self.limits.timeline_depth as usize;
        let byte_cap = self.limits.timeline_bytes;
        let mut bytes = self
            .timeline
            .iter()
            .map(estimated_event_bytes)
            .fold(0_u64, u64::saturating_add);
        while !self.timeline.is_empty() && (self.timeline.len() > count_cap || bytes > byte_cap) {
            let removed = self.timeline.remove(0);
            bytes = bytes.saturating_sub(estimated_event_bytes(&removed));
        }
    }

    /// Offer a live event push while reconciling: buffer it, or on overflow
    /// record the loss and coalesce a `LocalOverflow` re-trigger (§R5).
    pub(crate) fn buffer_live_event(&mut self, event: Event) {
        if !event_identifiers_within(&event, self.limits.max_identifier_bytes)
            || estimated_event_bytes(&event) > self.limits.timeline_bytes
        {
            if let Phase::Reconciling(recon) = &mut self.phase {
                recon.rerun = Some(coalesce(recon.rerun.take(), protocol_gap()));
            }
            return;
        }
        if let Phase::Reconciling(recon) = &mut self.phase {
            // Event and peer pushes share one transient room budget; otherwise
            // two individually bounded collections could retain twice the
            // advertised count and bytes.
            let event_bytes = estimated_event_bytes(&event);
            let combined_count = recon.buffer.len().saturating_add(recon.peer_buffer.len());
            let combined_bytes = recon
                .buffer
                .bytes()
                .saturating_add(recon.peer_buffer_bytes)
                .saturating_add(event_bytes);
            if combined_count >= self.limits.buffer_depth as usize
                || combined_bytes > self.limits.buffer_bytes
            {
                recon.buffer.note_drop();
            } else {
                // The buffer's loss latch is the single source of quantitative
                // truth. Convergence or stashing translates it exactly once.
                let _ = recon.buffer.push(event);
            }
        }
    }

    /// Hold a push received while parked. The bounded buffer is drained on the
    /// next authoritative baseline; overflow remains observable as loss.
    /// Supersede an old-anchor reconciliation without committing or
    /// publishing it. Event pushes are moved into the bounded parked buffer;
    /// peer pushes cannot be ordered across the future authoritative snapshot,
    /// so their loss forces a presence replacement.
    fn retry_pending_bootstrap(&mut self) -> ReplyOutcome {
        let reason = match &mut self.phase {
            Phase::Reconciling(recon) => {
                if recon.presence {
                    self.presence_refresh_pending = true;
                }
                recon.rerun.take().unwrap_or(ResyncReason::Bootstrap)
            }
            _ => ResyncReason::Bootstrap,
        };
        // `begin_reconcile` stashes the old read's buffered pushes before it
        // installs the replacement state.
        ReplyOutcome::Retry { reason }
    }

    pub(crate) fn buffer_parked_event(&mut self, event: Event) {
        if !event_identifiers_within(&event, self.limits.max_identifier_bytes)
            || estimated_event_bytes(&event) > self.limits.timeline_bytes
        {
            self.pending_local_dropped = self.pending_local_dropped.saturating_add(1);
            self.presence_refresh_pending = true;
            return;
        }
        let _ = self.parked_buffer.push(event);
    }

    /// A peer push arrived while no authoritative baseline was publishable.
    /// Count the lost evidence, force a presence snapshot on the next liveness
    /// trigger, and fence the key's generation so a stale replay of the same
    /// connection cannot later masquerade as an unknown fresh peer. Oversized
    /// identifiers are never retained; their unknown-key path fails closed on
    /// the identifier bound itself.
    pub(crate) fn note_parked_peer_loss(
        &mut self,
        subject_id: SubjectId,
        device_id: DeviceId,
        generation: u64,
    ) {
        self.pending_local_dropped = self.pending_local_dropped.saturating_add(1);
        self.presence_refresh_pending = true;
        if identifier_within(subject_id.as_str(), self.limits.max_identifier_bytes)
            && identifier_within(device_id.as_str(), self.limits.max_identifier_bytes)
        {
            self.fence_discarded_key((subject_id, device_id), generation);
        }
    }

    /// Whether parked push buffering has lost data.
    pub(crate) fn parked_dropped(&self) -> u64 {
        self.parked_buffer.dropped()
    }

    /// Mark a structural retry. Only one immediate retry is allowed; a second
    /// malformed page parks the room instead of hot-looping against the daemon.
    fn structural_retry(&mut self) -> ReplyOutcome {
        if self.protocol_retry_used {
            // A permanently malformed daemon must park even if another trigger
            // accrued during the retry; that trigger is folded into the parked
            // cause and may run only after a future external liveness input.
            self.fail_inner(CallError::Local(LocalError::Backend), false)
        } else {
            // The relaunching `begin_reconcile` stashes the old read's buffers.
            // Fold an accrued cause into this retry before that phase is
            // replaced; otherwise taking the recon apart would erase it.
            self.protocol_retry_used = true;
            let mut accrued = None;
            if let Phase::Reconciling(recon) = &mut self.phase {
                if recon.replace_timeline {
                    self.pending_bootstrap = true;
                }
                if recon.presence {
                    self.presence_refresh_pending = true;
                }
                accrued = recon.rerun.take();
            }
            let reason = self.coalesce_banking_loss(accrued, protocol_gap());
            ReplyOutcome::Retry { reason }
        }
    }

    /// Coalesce a new trigger into the outstanding reconciliation's pending
    /// re-run (§R9). Only valid while reconciling.
    pub(crate) fn coalesce_rerun(&mut self, reason: ResyncReason, from_pos: Option<u64>) {
        if let Some(from_pos) = from_pos {
            self.pending_gap_from = Some(
                self.pending_gap_from
                    .map_or(from_pos, |current| current.min(from_pos)),
            );
        }
        let existing = match &mut self.phase {
            Phase::Reconciling(recon) => recon.rerun.take(),
            _ => return,
        };
        let merged = self.coalesce_banking_loss(existing, reason);
        if let Phase::Reconciling(recon) = &mut self.phase {
            recon.rerun = Some(merged);
        }
    }

    pub(crate) fn take_pending_gap_from(&mut self) -> Option<u64> {
        self.pending_gap_from.take()
    }

    /// Apply a live event push to a converged room (§R6). A duplicate or
    /// already-held push is ignored; a contiguous push extends in place; a jump
    /// forces a resync.
    pub(crate) fn apply_live_event(&mut self, event: Event) -> LiveOutcome {
        if !event_identifiers_within(&event, self.limits.max_identifier_bytes)
            || estimated_event_bytes(&event) > self.limits.timeline_bytes
        {
            return LiveOutcome::NeedResync(protocol_gap());
        }
        if event.pos != 0
            && matches!(
                &event.kind,
                jeliya_api::EventKindContent::RoomCreated { .. }
            )
        {
            return LiveOutcome::NeedResync(protocol_gap());
        }
        if let Some(watermark) = self.watermark {
            if event.pos <= watermark {
                if self.all_event_ids.position(&event.event_id) != Some(event.pos)
                    || self.retained_position_conflicts(&event)
                {
                    // The disputed position must be re-proven by authority; a
                    // resync from the watermark would never re-read it and
                    // would leave the committed claimant standing unverified.
                    self.dispute_position(event.pos);
                    return LiveOutcome::NeedResync(protocol_gap());
                }
                return LiveOutcome::Ignored;
            }
        }
        if self.recent_ids.contains(&event.event_id) || self.all_event_ids.contains(&event.event_id)
        {
            if self
                .watermark
                .is_some_and(|watermark| event.pos > watermark)
            {
                return LiveOutcome::NeedResync(ResyncReason::Gap {
                    reason: GapReason::SubscriptionLapse,
                    to: GapTo::Open,
                });
            }
            return LiveOutcome::Ignored;
        }
        match self.watermark {
            Some(watermark) if event.pos == watermark.saturating_add(1) => {
                if !self.all_event_ids.insert_new(&event.event_id, event.pos) {
                    return LiveOutcome::NeedResync(protocol_gap());
                }
                self.append(event);
                if self.presence_refresh_pending {
                    return LiveOutcome::NeedResync(ResyncReason::Gap {
                        reason: GapReason::SubscriptionLapse,
                        to: GapTo::Open,
                    });
                }
                let epoch = self.last_epoch;
                LiveOutcome::Applied(self.view(epoch))
            }
            _ => LiveOutcome::NeedResync(ResyncReason::Gap {
                reason: GapReason::SubscriptionLapse,
                to: GapTo::Open,
            }),
        }
    }

    /// Hold a live `peer` push while a baseline is outstanding. Repeated
    /// generations for one device coalesce in place; a genuinely new key uses
    /// the same finite depth bound as event buffering. Overflow forces a
    /// presence-refreshing `LocalOverflow` rerun rather than silent loss.
    pub(crate) fn buffer_peer_push(
        &mut self,
        subject_id: SubjectId,
        device_id: DeviceId,
        link: Link,
        generation: u64,
    ) {
        let dropped_key = {
            let Phase::Reconciling(recon) = &mut self.phase else {
                return;
            };
            if !identifier_within(subject_id.as_str(), self.limits.max_identifier_bytes)
                || !identifier_within(device_id.as_str(), self.limits.max_identifier_bytes)
            {
                // Attacker-sized identifiers are never retained, not even as a
                // fence; the unknown-key path already fails closed for them
                // because `apply_peer_push_now` re-checks the same bound.
                recon.peer_dropped = recon.peer_dropped.saturating_add(1);
                return;
            }
            if let Some(existing) = recon
                .peer_buffer
                .iter_mut()
                .find(|push| push.subject_id == subject_id && push.device_id == device_id)
            {
                if generation >= existing.generation {
                    // Generation is a connection fence, not an update sequence;
                    // later arrival order within the same connection wins.
                    existing.link = link;
                    existing.generation = generation;
                }
                return;
            }
            let size = 64_u64
                .saturating_add(subject_id.as_str().len() as u64)
                .saturating_add(device_id.as_str().len() as u64);
            let combined_count = recon.buffer.len().saturating_add(recon.peer_buffer.len());
            let combined_bytes = recon
                .buffer
                .bytes()
                .saturating_add(recon.peer_buffer_bytes)
                .saturating_add(size);
            if combined_count >= self.limits.buffer_depth as usize
                || combined_bytes > self.limits.buffer_bytes
            {
                // Like event-buffer loss, this counter is translated once when
                // the reconciliation settles or is stashed.
                recon.peer_dropped = recon.peer_dropped.saturating_add(1);
                Some((subject_id, device_id))
            } else {
                recon.peer_buffer_bytes = recon.peer_buffer_bytes.saturating_add(size);
                recon.peer_buffer.push_back(BufferedPeerPush {
                    subject_id,
                    device_id,
                    link,
                    generation,
                });
                None
            }
        };
        // A dropped push's generation must remain fenced: after an omitting
        // authoritative snapshot, a stale same-generation replay would
        // otherwise look like an unknown fresh peer and resurrect it.
        if let Some(key) = dropped_key {
            self.fence_discarded_key(key, generation);
        }
    }

    /// Apply a live `peer` push to a converged room (§R8). A stale-generation
    /// teardown is discarded and cannot resurrect a peer an authoritative read
    /// removed. The peer set is capped so wire-supplied device keys cannot grow
    /// memory without bound between snapshots.
    pub(crate) fn apply_peer_push(
        &mut self,
        subject_id: SubjectId,
        device_id: DeviceId,
        link: Link,
        generation: u64,
    ) -> PeerOutcome {
        if !self.is_converged() {
            return PeerOutcome::Ignored;
        }
        self.apply_peer_push_now(subject_id, device_id, link, generation)
    }

    fn apply_peer_push_now(
        &mut self,
        subject_id: SubjectId,
        device_id: DeviceId,
        link: Link,
        generation: u64,
    ) -> PeerOutcome {
        if !identifier_within(subject_id.as_str(), self.limits.max_identifier_bytes)
            || !identifier_within(device_id.as_str(), self.limits.max_identifier_bytes)
        {
            return PeerOutcome::Overflow;
        }
        let key = (subject_id.clone(), device_id.clone());
        if let Some(last) = self.peer_generations.get(&key).copied().flatten() {
            if generation < last {
                return PeerOutcome::Ignored;
            }
        } else if let Some(tombstone) = self.peer_tombstones.get(&key).copied() {
            match tombstone {
                Some(last) if generation < last => return PeerOutcome::Ignored,
                Some(last) if generation == last => return PeerOutcome::Overflow,
                None => return PeerOutcome::Overflow,
                Some(_) => {
                    self.peer_tombstones.remove(&key);
                }
            }
        } else if self.peer_tombstone_overflowed {
            // A prior tombstone could not be retained. Unknown keys are now
            // ambiguous and must be confirmed by authoritative presence.
            return PeerOutcome::Overflow;
        }

        let known = self.peer_generations.contains_key(&key);
        if !known && self.peers.len() >= self.limits.peer_capacity as usize {
            return PeerOutcome::Overflow;
        }
        self.peer_generations.insert(key, Some(generation));
        if let Some(row) = self
            .peers
            .iter_mut()
            .find(|row| row.subject_id == subject_id && row.device_id == device_id)
        {
            // Generation fences connections, not updates; arrival order wins
            // for multiple changes within one generation.
            row.link = link;
        } else {
            self.peers.push(PeerRow {
                subject_id,
                device_id,
                link,
            });
        }
        // The whole-room aggregate is derived from the links, so it must move
        // with them; leaving it at the last snapshot's value reports `Alone`
        // for a freshly connected peer and `Connected` after the last one drops.
        self.recompute_reachability();
        let epoch = self.last_epoch;
        PeerOutcome::Applied(self.view(epoch))
    }

    fn replace_peer_snapshot(&mut self, peers: Vec<PeerRow>) {
        let mut previous = std::mem::take(&mut self.peer_generations);
        let mut next = HashMap::new();
        for row in &peers {
            let key = (row.subject_id.clone(), row.device_id.clone());
            let live = previous.remove(&key).flatten();
            let tombstone = self.peer_tombstones.remove(&key).flatten();
            next.insert(key, merge_peer_generation(live, tombstone));
        }
        // Sort the removed keys before fencing them: when the bounded
        // tombstone map saturates, WHICH keys stay fenced must be a
        // deterministic function of the inputs, not of hash-map iteration
        // order — the sans-IO core promises identical behavior per §R1.
        let mut removed: Vec<_> = previous.into_iter().collect();
        removed.sort_by(|left, right| {
            (left.0 .0.as_str(), left.0 .1.as_str())
                .cmp(&(right.0 .0.as_str(), right.0 .1.as_str()))
        });
        for (key, generation) in removed {
            self.note_peer_tombstone(key, generation);
        }
        self.peers = peers;
        self.peer_generations = next;
    }

    fn fence_discarded_peer(&mut self, push: BufferedPeerPush) {
        self.fence_discarded_key((push.subject_id, push.device_id), push.generation);
    }

    fn fence_discarded_key(&mut self, key: (SubjectId, DeviceId), generation: u64) {
        if let Some(current) = self.peer_generations.get_mut(&key) {
            *current = merge_peer_generation(*current, Some(generation));
        } else {
            self.note_peer_tombstone(key, Some(generation));
        }
    }

    fn note_peer_tombstone(&mut self, key: (SubjectId, DeviceId), generation: Option<u64>) {
        if let Some(current) = self.peer_tombstones.get_mut(&key) {
            *current = merge_peer_generation(*current, generation);
        } else if self.peer_tombstones.len() < self.limits.peer_capacity as usize {
            self.peer_tombstones.insert(key, generation);
        } else {
            // Forgetting a key is safe only if all later unknown peers take the
            // fail-closed authoritative-refresh path.
            self.peer_tombstone_overflowed = true;
        }
    }

    /// Recompute whole-room reachability from the per-device links.
    ///
    /// Only the `Connected`/`Alone` distinction is derivable from links.
    /// `Connecting`/`Offline` are transport-level answers the daemon owns, so a
    /// peer push must never fabricate liveness the client does not have.
    fn recompute_reachability(&mut self) {
        if !matches!(
            self.reachability,
            Reachability::Connected | Reachability::Alone
        ) {
            return;
        }
        let linked = self
            .peers
            .iter()
            .any(|row| matches!(row.link, Link::Direct { .. } | Link::Relay { .. }));
        self.reachability = if linked {
            Reachability::Connected
        } else {
            Reachability::Alone
        };
    }

    /// Build the current view stamped with `generation`.
    fn view(&self, generation: u64) -> RoomView {
        RoomView {
            room_id: self.room_id.clone(),
            generation,
            timeline: self.timeline.clone(),
            members: self.members.clone(),
            peers: self.peers.clone(),
            reachability: self.reachability,
        }
    }
}

/// Retain only the newest baseline events while allowing the scalar scan cursor
/// to advance across arbitrarily many authoritative pages.
fn retain_baseline_event(recon: &mut Reconciling, event: Event) {
    let size = estimated_event_bytes(&event);
    if recon.event_cap == 0 || size > recon.event_bytes_cap {
        return;
    }
    while !recon.events.is_empty()
        && (recon.events.len() >= recon.event_cap
            || recon.event_bytes.saturating_add(size) > recon.event_bytes_cap)
    {
        let removed = recon.events.remove(0);
        recon.event_bytes = recon
            .event_bytes
            .saturating_sub(estimated_event_bytes(&removed));
    }
    if recon.event_bytes.saturating_add(size) <= recon.event_bytes_cap {
        recon.event_bytes = recon.event_bytes.saturating_add(size);
        recon.events.push(event);
    }
}

/// A forward timeline continuation must name the next unread position. This
/// rejects a repeated cursor and prevents a malformed daemon page from making
/// the sans-IO core spin forever.
fn timeline_cursor_is_next(cursor: &Cursor, last_pos: u64) -> bool {
    matches!(cursor, Cursor::At { pos } if last_pos.checked_add(1) == Some(*pos))
}

fn identifier_within(value: &str, max: u32) -> bool {
    value.len() as u64 <= u64::from(max)
}

fn reply_events_valid(events: &[Event], count_cap: u64, limits: ReconcileLimits) -> bool {
    if events.len() as u64 > count_cap {
        return false;
    }
    // Fixed reply/vector overhead keeps a zero-byte configuration closed even
    // for an otherwise empty typed fixture that bypassed raw decoding.
    let mut bytes = 64_u64;
    if bytes > limits.max_read_reply_bytes {
        return false;
    }
    for event in events {
        let event_bytes = estimated_event_bytes(event);
        if event_bytes > limits.timeline_bytes
            || !event_identifiers_within(event, limits.max_identifier_bytes)
        {
            return false;
        }
        bytes = bytes.saturating_add(event_bytes);
        if bytes > limits.max_read_reply_bytes {
            return false;
        }
    }
    true
}

fn members_snapshot_valid(rows: &[MemberRow], cap: usize, limits: ReconcileLimits) -> bool {
    if rows.len() > cap {
        return false;
    }
    let mut bytes = 64_u64;
    let mut ids: HashSet<&SubjectId> = HashSet::with_capacity(rows.len());
    rows.iter().all(|row| {
        bytes = bytes
            .saturating_add(64)
            .saturating_add(row.subject_id.as_str().len() as u64);
        identifier_within(row.subject_id.as_str(), limits.max_identifier_bytes)
            && bytes <= limits.max_read_reply_bytes
            && ids.insert(&row.subject_id)
    })
}

fn peers_snapshot_valid(rows: &[PeerRow], cap: usize, limits: ReconcileLimits) -> bool {
    if rows.len() > cap {
        return false;
    }
    let mut bytes = 64_u64;
    let mut keys: HashSet<(&SubjectId, &DeviceId)> = HashSet::with_capacity(rows.len());
    rows.iter().all(|row| {
        bytes = bytes
            .saturating_add(64)
            .saturating_add(row.subject_id.as_str().len() as u64)
            .saturating_add(row.device_id.as_str().len() as u64);
        identifier_within(row.subject_id.as_str(), limits.max_identifier_bytes)
            && identifier_within(row.device_id.as_str(), limits.max_identifier_bytes)
            && bytes <= limits.max_read_reply_bytes
            && keys.insert((&row.subject_id, &row.device_id))
    })
}

fn is_membership_event(event: &Event) -> bool {
    matches!(
        &event.kind,
        jeliya_api::EventKindContent::MemberJoined { .. }
            | jeliya_api::EventKindContent::MemberLeft { .. }
            | jeliya_api::EventKindContent::MemberRemoved { .. }
    )
}

/// Classify a malformed/inconsistent authoritative page without inventing a
/// daemon error code. The caller immediately relaunches a bounded gap recovery.
fn merge_peer_generation(current: Option<u64>, next: Option<u64>) -> Option<u64> {
    match (current, next) {
        (Some(current), Some(next)) => Some(current.max(next)),
        (Some(current), None) => Some(current),
        (None, Some(next)) => Some(next),
        (None, None) => None,
    }
}

fn protocol_gap() -> ResyncReason {
    ResyncReason::Gap {
        reason: GapReason::SubscriptionLapse,
        to: GapTo::Open,
    }
}

/// Keep the stronger of an optional accrued cause and a new one.
fn coalesce(existing: Option<ResyncReason>, new: ResyncReason) -> ResyncReason {
    match existing {
        Some(existing) => existing.coalesce(new),
        None => new,
    }
}

/// The quantitative loss a cause carries, if it is a `LocalOverflow`.
fn overflow_dropped(reason: Option<&ResyncReason>) -> u64 {
    match reason {
        Some(ResyncReason::LocalOverflow { dropped }) => *dropped,
        _ => 0,
    }
}

/// The `more` continuation cursor of a page, if any.
fn more_cursor(truncated: &Truncated) -> Option<Cursor> {
    match truncated {
        Truncated::More { cursor } => Some(cursor.clone()),
        Truncated::Complete => None,
    }
}

/// The `from_pos` of a `resync_required` wire error, if that is what `err` is.
fn resync_required_from(err: &CallError, expected_room: &RoomId) -> Result<Option<u64>, ()> {
    match err.as_wire() {
        Some(ApiError::ResyncRequired { room_id, from_pos }) if room_id == expected_room => {
            Ok(Some(*from_pos))
        }
        Some(ApiError::ResyncRequired { .. }) => Err(()),
        _ => Ok(None),
    }
}
