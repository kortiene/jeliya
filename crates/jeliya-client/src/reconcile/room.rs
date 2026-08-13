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
    PeerRow, Reachability, RoomId, RoomMembers, RoomPeers, RoomTimeline, StreamResync, SubjectId,
    Truncated,
};

use crate::error::{CallError, LocalError};
use crate::reconcile::buffer::{PushOutcome, ReconcileBuffer};
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
    Timeline { cursor: Cursor },
    /// Read a `stream.resync` page from `from_pos`.
    Resync { from_pos: u64 },
    /// Read the `room.members` roster.
    Members,
    /// Read the `room.peers` snapshot.
    Peers,
}

/// What the core does after feeding a reply to the room.
pub(crate) enum ReplyOutcome {
    /// Issue the next read of this reconciliation (same `read_id`, same epoch).
    NextRead(ReadRequest),
    /// The reconciliation converged; the commit is done, broadcast this view.
    /// `rerun` carries a coalesced re-trigger to relaunch immediately (§R9).
    Converged {
        /// The converged view to broadcast.
        view: RoomView,
        /// A coalesced re-trigger, if one accrued.
        rerun: Option<ResyncReason>,
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
    /// Baseline events accumulated across pages, in ascending `pos`.
    events: Vec<Event>,
    /// Live pushes held while the baseline read is outstanding.
    buffer: ReconcileBuffer,
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
struct DedupRing {
    cap: usize,
    order: VecDeque<EventId>,
    set: HashSet<EventId>,
}

impl DedupRing {
    fn new(cap: u32) -> Self {
        Self {
            cap: (cap as usize).max(1),
            order: VecDeque::new(),
            set: HashSet::new(),
        }
    }

    fn contains(&self, id: &EventId) -> bool {
        self.set.contains(id)
    }

    fn insert(&mut self, id: EventId) {
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
    /// The authoritative roster (replaced wholesale).
    members: Vec<MemberRow>,
    /// The authoritative presence snapshot (replaced wholesale).
    peers: Vec<PeerRow>,
    /// The last authoritative reachability.
    reachability: Reachability,
    /// The last-seen connection generation per `(subject, device)` for live
    /// `peer` pushes, so a stale-generation teardown is discarded (§R8).
    peer_generations: HashMap<(SubjectId, DeviceId), u64>,
    /// The subscription anchor from the most recent activation.
    bootstrap_anchor: u64,
    /// The epoch the room last reconciled under, used to stamp in-place views.
    last_epoch: u64,
    /// Whether a reconciliation has ever completed for this room.
    converged_once: bool,
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
            recent_ids: DedupRing::new(limits.dedup_window),
            members: Vec::new(),
            peers: Vec::new(),
            reachability: Reachability::Offline,
            peer_generations: HashMap::new(),
            bootstrap_anchor: anchor,
            last_epoch: 0,
            converged_once: false,
            phase: Phase::NeedsReconcile {
                reason: ResyncReason::Bootstrap,
            },
        }
    }

    /// Record a fresh activation anchor (a re-`stream.subscribe`'s `from_pos`).
    pub(crate) fn set_anchor(&mut self, anchor: u64) {
        self.bootstrap_anchor = anchor;
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

    /// Whether the room holds a converged timeline (has a watermark).
    pub(crate) fn has_baseline(&self) -> bool {
        self.watermark.is_some()
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
        // A daemon-named `from_pos` (a `resync_required` / `ResyncRequired`) means
        // "discard back to `from_pos` and re-read". Truncate the timeline and
        // rebuild dedup state so the re-read events are not wrongly skipped.
        if let Some(from_pos) = from_pos_override {
            self.truncate_to(from_pos);
        }
        let incremental = from_pos_override.is_some()
            || (self.watermark.is_some() && !matches!(reason, ResyncReason::Bootstrap));
        let presence = reason.implicates_presence();
        let outstanding = if incremental {
            let from_pos = from_pos_override.unwrap_or_else(|| self.watermark.unwrap_or(0));
            ReadStep::Resync { from_pos }
        } else {
            ReadStep::Timeline {
                cursor: Cursor::Start,
            }
        };
        let mut plan = VecDeque::new();
        if !incremental || presence {
            plan.push_back(ReadStep::Members);
            plan.push_back(ReadStep::Peers);
        }
        let request = self.build_request(&outstanding);
        let buffer = ReconcileBuffer::new(self.limits.buffer_depth, self.limits.buffer_bytes);
        self.last_epoch = epoch;
        self.phase = Phase::Reconciling(Box::new(Reconciling {
            epoch,
            reason,
            read_id,
            outstanding,
            plan,
            events: Vec::new(),
            buffer,
            rerun: None,
            members: None,
            peers: None,
        }));
        request
    }

    /// Build the typed request for one read step.
    fn build_request(&self, step: &ReadStep) -> ReadRequest {
        match step {
            ReadStep::Timeline { cursor } => ReadRequest::Timeline(RoomTimeline {
                room_id: self.room_id.clone(),
                page: Page {
                    cursor: cursor.clone(),
                    direction: Direction::Forward,
                    limit: self.limits.read_page_size,
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
                ReadReply::Timeline(Ok(out)) => {
                    recon.events.extend(out.events);
                    match more_cursor(&out.truncated) {
                        Some(cursor) => {
                            recon.outstanding = ReadStep::Timeline { cursor };
                            Next::Issue
                        }
                        None => Next::Advance,
                    }
                }
                ReadReply::Resync(Ok(out)) => {
                    recon.events.extend(out.events);
                    if matches!(out.truncated, Truncated::More { .. }) {
                        recon.outstanding = ReadStep::Resync {
                            from_pos: out.next_pos,
                        };
                        Next::Issue
                    } else {
                        Next::Advance
                    }
                }
                ReadReply::Resync(Err(err)) => match resync_required_from(&err) {
                    Some(from_pos) => Next::Restart(from_pos),
                    None => Next::Fail(err),
                },
                ReadReply::Members(Ok(out)) => {
                    recon.members = Some(out.members);
                    Next::Advance
                }
                ReadReply::Peers(Ok(out)) => {
                    recon.peers = Some((out.reachability, out.peers));
                    Next::Advance
                }
                ReadReply::Timeline(Err(err))
                | ReadReply::Members(Err(err))
                | ReadReply::Peers(Err(err)) => Next::Fail(err),
            }
        };
        match next {
            Next::Issue => ReplyOutcome::NextRead(self.issue_outstanding()),
            Next::Advance => self.advance(),
            Next::Restart(from_pos) => ReplyOutcome::Restart {
                reason: ResyncReason::ResyncRequiredByDaemon { from_pos },
                from_pos,
            },
            Next::Fail(err) => self.fail(err),
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
        let (reason, rerun) = match &mut self.phase {
            Phase::Reconciling(recon) => (recon.reason.clone(), recon.rerun.take()),
            _ => (ResyncReason::Reconnect, None),
        };
        let parked = match rerun.clone() {
            Some(rerun) => reason.coalesce(rerun),
            None => reason,
        };
        self.phase = Phase::NeedsReconcile {
            reason: parked.clone(),
        };
        // Relaunch only when a re-trigger actually accrued: an unprovoked retry
        // on every failure would auto-spin against a failing daemon.
        ReplyOutcome::Failed {
            err,
            rerun: rerun.map(|_| parked),
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
        let (mut rerun, baseline, mut buffered, members, peers, epoch) = match &mut self.phase {
            Phase::Reconciling(recon) => (
                recon.rerun.take(),
                std::mem::take(&mut recon.events),
                recon.buffer.drain(),
                recon.members.take(),
                recon.peers.take(),
                recon.epoch,
            ),
            _ => {
                return ReplyOutcome::Failed {
                    err: CallError::Local(LocalError::Backend),
                    rerun: None,
                }
            }
        };

        // 1. Apply the authoritative baseline first (§R6.1). A hole inside the
        //    applied range forces a fresh resync for the remainder.
        if let Some(gap) = self.apply_ordered(baseline) {
            rerun = Some(coalesce(rerun, gap));
        }

        // 2. Replace presence/membership wholesale (§R8).
        if let Some(members) = members {
            self.members = members;
        }
        if let Some((reachability, peers)) = peers {
            self.reachability = reachability;
            self.peers = peers;
            // Live-push generations no longer apply to a fresh snapshot.
            self.peer_generations.clear();
        }

        // 3. Drain the buffer, converging by evidence (§R6.2).
        buffered.sort_by_key(|event| event.pos);
        if let Some(gap) = self.apply_ordered(buffered) {
            rerun = Some(coalesce(rerun, gap));
        }

        self.converged_once = true;
        self.last_epoch = epoch;
        self.phase = Phase::Converged;
        ReplyOutcome::Converged {
            view: self.view(epoch),
            rerun,
        }
    }

    /// Apply a `pos`-ascending run of events to the timeline, deduplicating by
    /// watermark and by recent id, requiring density. Returns a gap cause when a
    /// hole is found (the contiguous prefix is applied; the rest forces a
    /// resync).
    fn apply_ordered(&mut self, events: Vec<Event>) -> Option<ResyncReason> {
        let mut gap: Option<ResyncReason> = None;
        for event in events {
            if let Some(watermark) = self.watermark {
                if event.pos <= watermark {
                    // Already held: the primary O(1) dedup.
                    continue;
                }
            }
            if self.recent_ids.contains(&event.event_id) {
                // Exact-identity dedup across the convergence boundary.
                continue;
            }
            match self.watermark {
                None => self.append(event),
                Some(watermark) if event.pos == watermark + 1 => self.append(event),
                Some(_) => {
                    // A hole: everything past the watermark is suspect. Stop and
                    // force a fresh resync for the remainder — never insert an
                    // event out of order.
                    gap = Some(ResyncReason::Gap {
                        reason: GapReason::SubscriptionLapse,
                        to: GapTo::Open,
                    });
                    break;
                }
            }
        }
        gap
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
        self.timeline.retain(|event| event.pos <= from_pos);
        self.watermark = self.timeline.last().map(|event| event.pos);
        // Rebuild the recent-id ring from the retained tail so a duplicate of a
        // retained event is still caught, while a re-read event beyond `from_pos`
        // is not.
        let window = self.limits.dedup_window as usize;
        let start = self.timeline.len().saturating_sub(window);
        let retained_ids: Vec<EventId> = self.timeline[start..]
            .iter()
            .map(|event| event.event_id.clone())
            .collect();
        self.recent_ids = DedupRing::new(self.limits.dedup_window);
        for id in retained_ids {
            self.recent_ids.insert(id);
        }
        // Presence generations belong to the discarded connection window.
        self.peer_generations.clear();
    }

    /// Append one authoritative event, advancing the watermark and recording its
    /// id. Only ever called on an event that passed dedup and density.
    fn append(&mut self, event: Event) {
        self.watermark = Some(event.pos);
        self.recent_ids.insert(event.event_id.clone());
        self.timeline.push(event);
    }

    /// Offer a live event push while reconciling: buffer it, or on overflow
    /// record the loss and coalesce a `LocalOverflow` re-trigger (§R5).
    pub(crate) fn buffer_live_event(&mut self, event: Event) {
        if let Phase::Reconciling(recon) = &mut self.phase {
            if let PushOutcome::Overflow = recon.buffer.push(event) {
                let dropped = recon.buffer.dropped();
                recon.rerun = Some(coalesce(
                    recon.rerun.take(),
                    ResyncReason::LocalOverflow { dropped },
                ));
            }
        }
    }

    /// Coalesce a new trigger into the outstanding reconciliation's pending
    /// re-run (§R9). Only valid while reconciling.
    pub(crate) fn coalesce_rerun(&mut self, reason: ResyncReason) {
        if let Phase::Reconciling(recon) = &mut self.phase {
            recon.rerun = Some(coalesce(recon.rerun.take(), reason));
        }
    }

    /// Apply a live event push to a converged room (§R6). A duplicate or
    /// already-held push is ignored; a contiguous push extends in place; a jump
    /// forces a resync.
    pub(crate) fn apply_live_event(&mut self, event: Event) -> LiveOutcome {
        if let Some(watermark) = self.watermark {
            if event.pos <= watermark {
                return LiveOutcome::Ignored;
            }
        }
        if self.recent_ids.contains(&event.event_id) {
            return LiveOutcome::Ignored;
        }
        match self.watermark {
            Some(watermark) if event.pos == watermark + 1 => {
                self.append(event);
                let epoch = self.last_epoch;
                LiveOutcome::Applied(self.view(epoch))
            }
            _ => LiveOutcome::NeedResync(ResyncReason::Gap {
                reason: GapReason::SubscriptionLapse,
                to: GapTo::Open,
            }),
        }
    }

    /// Apply a live `peer` push (§R8). While reconciling, live peer pushes are
    /// dropped — the reconciliation re-reads `room.peers` authoritatively. While
    /// converged, the push is fenced by the last-seen connection generation per
    /// `(subject, device)`: a stale-generation teardown is discarded and cannot
    /// resurrect a peer an authoritative read removed. Returns a view when the
    /// snapshot changed.
    pub(crate) fn apply_peer_push(
        &mut self,
        subject_id: SubjectId,
        device_id: DeviceId,
        link: Link,
        generation: u64,
    ) -> Option<RoomView> {
        if self.is_reconciling() {
            return None;
        }
        let key = (subject_id.clone(), device_id.clone());
        if let Some(last) = self.peer_generations.get(&key) {
            if generation <= *last {
                return None;
            }
        }
        self.peer_generations.insert(key, generation);
        if let Some(row) = self
            .peers
            .iter_mut()
            .find(|row| row.subject_id == subject_id && row.device_id == device_id)
        {
            row.link = link;
        } else {
            self.peers.push(PeerRow {
                subject_id,
                device_id,
                link,
            });
        }
        let epoch = self.last_epoch;
        Some(self.view(epoch))
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

/// Keep the stronger of an optional accrued cause and a new one.
fn coalesce(existing: Option<ResyncReason>, new: ResyncReason) -> ResyncReason {
    match existing {
        Some(existing) => existing.coalesce(new),
        None => new,
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
fn resync_required_from(err: &CallError) -> Option<u64> {
    match err.as_wire() {
        Some(ApiError::ResyncRequired { from_pos, .. }) => Some(*from_pos),
        _ => None,
    }
}
