//! The sans-IO reconciliation state machine (#169 §R1–§R14).
//!
//! All convergence, fencing, dedup, single-flight, and coalescing logic lives in
//! this pure, synchronous core: [`Core::step`] consumes one [`Input`] and
//! returns a `Vec<Action>` for the driver to perform. It **never blocks, never
//! awaits, reads no clock, and makes no RNG call** — ordering and dedup are
//! driven by the `pos`/`event_id`/`at` carried on inputs. Every fault the
//! Verification section names (push during bootstrap, reconnect during open,
//! repeated gaps, overflow, cancellation, resume, stale generations) is an
//! ordinary deterministic sequence of `step` calls, identical on wasm and
//! native.

use std::collections::HashMap;

use jeliya_api::RoomId;

use crate::event::{ClientEvent, RoomPush, State};
use crate::reconcile::reason::ResyncReason;
use crate::reconcile::room::{
    LiveOutcome, PeerOutcome, ReadReply, ReadRequest, ReplyOutcome, RoomState,
};
use crate::reconcile::view::RoomView;
use crate::reconcile::ReconcileLimits;

/// One input to the core. Every non-determinism a real transport would
/// introduce (a push, a lifecycle transition, a settled read) enters here as an
/// ordinary value, which is what makes the fault suite exhaustive (§R1).
pub(crate) enum Input {
    /// The caller opened a room; `from_pos` is the `stream.subscribe` anchor.
    ActivateRoom {
        /// The room to track.
        room_id: RoomId,
        /// The subscription anchor.
        from_pos: u64,
    },
    /// The caller closed a room.
    DeactivateRoom {
        /// The room to forget.
        room_id: RoomId,
    },
    /// A lifecycle transition lifted off the subscription.
    Lifecycle {
        /// The state entered.
        to: State,
        /// Whether a coalesced window passed through a problem state (§R12).
        coalesced_through_problem: bool,
    },
    /// A lifecycle transition carrying the observed source state. The driver
    /// uses this form to fence a transition that was queued around its initial
    /// state snapshot; a stale duplicate cannot cancel a bootstrap.
    LifecycleObserved {
        /// The state left.
        from: State,
        /// The state entered.
        to: State,
        /// Whether a coalesced window passed through a problem state (§R12).
        coalesced_through_problem: bool,
    },
    /// A push/gap/resync/lagged event lifted off the subscription. A
    /// [`ClientEvent::StateChanged`] is routed to [`Input::Lifecycle`] by the
    /// driver and never arrives here.
    Event(ClientEvent),
    /// A push for `room_id` could not be decoded to a typed event (unknown kind
    /// or malformed content) — a forced gap, never a silent hole (§R14).
    DecodeFailed {
        /// The room the undecodable push was for.
        room_id: RoomId,
    },
    /// A baseline read settled; tagged with the `read_id`/`epoch` it was issued
    /// under so a stale reply is fenced (§R4).
    ReadReply {
        /// The room the read was for.
        room_id: RoomId,
        /// The read identity.
        read_id: u64,
        /// The epoch the read was issued under.
        epoch: u64,
        /// The decoded reply.
        reply: ReadReply,
    },
    /// An adapter resume (§R11) — the same outcome as a reconnect, with no
    /// fabricated lifecycle transition.
    Resume,
    /// Cancel a room's outstanding read and forget it (§R13).
    Cancel {
        /// The room to cancel.
        room_id: RoomId,
    },
    /// Total stop (§R13): cancel every read, forget every room.
    Stop,
}

/// One action the driver performs after a [`Core::step`]. The core decides; the
/// driver does the I/O.
pub(crate) enum Action {
    /// Issue one baseline read through [`crate::ClientHandle::call`], feeding the
    /// settled result back as [`Input::ReadReply`] tagged with this
    /// `read_id`/`epoch`.
    IssueRead {
        /// The room the read is for.
        room_id: RoomId,
        /// The read identity to echo back.
        read_id: u64,
        /// The epoch to echo back.
        epoch: u64,
        /// The typed request.
        request: ReadRequest,
    },
    /// Abandon a room's outstanding read (drop its call future — a local cancel,
    /// never a fabricated remote cancel, §R13).
    CancelRead {
        /// The room whose read is abandoned.
        room_id: RoomId,
        /// The read identity to drop.
        read_id: u64,
    },
    /// Broadcast that a reconciliation started (so its cause is observable
    /// before its outcome, AC-1).
    EmitResyncRequired {
        /// The room being reconciled.
        room_id: RoomId,
        /// The epoch the reconciliation is fenced by.
        generation: u64,
        /// The observable cause.
        reason: ResyncReason,
    },
    /// Broadcast quantitative local loss already covered by the following
    /// authoritative view; this is a boundary, not another read request.
    EmitLagged { room_id: RoomId, dropped: u64 },
    /// Broadcast a converged (or in-place extended) view.
    EmitView(RoomView),
    /// A settled read was stale (its epoch/read_id is no longer current) and was
    /// discarded — never applied over newer state (§R4).
    DropStale {
        /// The room the stale read was for.
        room_id: RoomId,
        /// The stale read identity.
        read_id: u64,
    },
}

/// The sans-IO reconciler core.
pub(crate) struct Core {
    limits: ReconcileLimits,
    /// The last observed lifecycle state.
    state: State,
    /// The reconciler-local monotonic epoch (§R4): the Nth live connection this
    /// reconciler has observed. It fences stale baselines.
    epoch: u64,
    /// Whether a `Ready` has ever been observed, distinguishing the first
    /// connect (bootstrap) from a reconnect.
    seen_ready: bool,
    /// Monotonic read-id source.
    next_read_id: u64,
    /// The tracked rooms.
    rooms: HashMap<RoomId, RoomState>,
    /// Set once [`Input::Stop`] has run; further inputs are inert.
    stopped: bool,
}

impl Core {
    /// A fresh core with no tracked rooms, epoch 0, state `Idle`.
    pub(crate) fn new(limits: ReconcileLimits) -> Self {
        Self {
            limits,
            state: State::Idle,
            epoch: 0,
            seen_ready: false,
            next_read_id: 0,
            rooms: HashMap::new(),
            stopped: false,
        }
    }

    /// The current epoch (diagnostics/tests).
    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
    }

    /// How many rooms are tracked (diagnostics/tests; a bound is asserted).
    pub(crate) fn tracked_rooms(&self) -> usize {
        self.rooms.len()
    }

    /// Consume one input, returning the actions the driver must perform.
    pub(crate) fn step(&mut self, input: Input) -> Vec<Action> {
        let mut actions = Vec::new();
        if self.stopped {
            return actions;
        }
        match input {
            Input::ActivateRoom { room_id, from_pos } => {
                self.activate(room_id, from_pos, &mut actions)
            }
            Input::DeactivateRoom { room_id } | Input::Cancel { room_id } => {
                self.forget(&room_id, &mut actions)
            }
            Input::Lifecycle {
                to,
                coalesced_through_problem,
            } => self.on_lifecycle(None, to, coalesced_through_problem, &mut actions),
            Input::LifecycleObserved {
                from,
                to,
                coalesced_through_problem,
            } => self.on_lifecycle(Some(from), to, coalesced_through_problem, &mut actions),
            Input::Event(event) => self.on_event(event, &mut actions),
            Input::DecodeFailed { room_id } => self.trigger(
                &room_id,
                ResyncReason::Gap {
                    reason: jeliya_api::GapReason::SubscriptionLapse,
                    to: jeliya_api::GapTo::Open,
                },
                None,
                &mut actions,
            ),
            Input::ReadReply {
                room_id,
                read_id,
                epoch,
                reply,
            } => self.on_read_reply(room_id, read_id, epoch, reply, &mut actions),
            Input::Resume => self.on_resume(&mut actions),
            Input::Stop => self.on_stop(&mut actions),
        }
        actions
    }

    /// Allocate a fresh read identity.
    fn alloc_read_id(&mut self) -> u64 {
        let id = self.next_read_id;
        self.next_read_id += 1;
        id
    }

    /// Track a newly opened room. Refused (silently, the handle enforces the
    /// typed error) if it would exceed `max_active_rooms`. Bootstraps
    /// immediately when already live; otherwise waits for the first `Ready`.
    fn activate(&mut self, room_id: RoomId, from_pos: u64, actions: &mut Vec<Action>) {
        if room_id.as_str().len() as u64 > u64::from(self.limits.max_identifier_bytes) {
            return;
        }
        if self.rooms.contains_key(&room_id) {
            let changed = {
                let room = self.rooms.get_mut(&room_id).expect("room exists");
                let changed = room.set_anchor(from_pos);
                if changed {
                    // Replacement mode is independent of cause priority. Mark
                    // it before a queued Gap/Reconnect can outrank Bootstrap.
                    room.mark_bootstrap_pending();
                }
                changed
            };
            if changed && self.state == State::Ready {
                // A changed subscription anchor supersedes even a stalled read;
                // cancel/relaunch now. `begin_reconcile` preserves its buffered
                // pushes before installing the replacement state.
                self.launch(&room_id, ResyncReason::Bootstrap, None, actions);
            }
            return;
        }
        if self.rooms.len() >= self.limits.max_active_rooms as usize {
            // Capacity is enforced with a typed error at the handle; the core
            // refuses to track beyond the bound rather than grow unbounded.
            return;
        }
        self.rooms.insert(
            room_id.clone(),
            RoomState::new(room_id.clone(), from_pos, self.limits),
        );
        if self.state == State::Ready {
            self.launch(&room_id, ResyncReason::Bootstrap, None, actions);
        }
    }

    /// Cancel a room's outstanding read and forget it.
    fn forget(&mut self, room_id: &RoomId, actions: &mut Vec<Action>) {
        if let Some(room) = self.rooms.remove(room_id) {
            if let Some(read_id) = room.outstanding_read_id() {
                actions.push(Action::CancelRead {
                    room_id: room_id.clone(),
                    read_id,
                });
            }
        }
    }

    /// A lifecycle transition. Entry into `Ready` (or a coalesced flap through a
    /// problem state) bumps the epoch and re-baselines every active room (§R12).
    fn on_lifecycle(
        &mut self,
        observed_from: Option<State>,
        to: State,
        coalesced: bool,
        actions: &mut Vec<Action>,
    ) {
        // A transition observed on the subscription may have been queued just
        // before the initial state snapshot. If its source no longer matches
        // the core's state, it is stale; do not let it cancel a newer bootstrap.
        // Coalesced Ready→Ready transitions are the honest exception: their
        // endpoints intentionally hide an Interrupted window.
        if let Some(from) = observed_from {
            if !coalesced && from != self.state {
                return;
            }
            if !coalesced && from == to {
                self.state = to;
                return;
            }
        }
        let was_ready = self.state == State::Ready;
        self.state = to;
        let reconnect = to == State::Ready && (observed_from.is_none() || !was_ready || coalesced);
        if !reconnect {
            return;
        }
        let first = !self.seen_ready;
        self.seen_ready = true;
        self.epoch += 1;
        let reason = if first {
            ResyncReason::Bootstrap
        } else {
            ResyncReason::Reconnect
        };
        for room_id in self.room_ids() {
            if let Some(room) = self.rooms.get_mut(&room_id) {
                room.reset_peer_transport_epoch();
            }
            self.launch(&room_id, reason.clone(), None, actions);
        }
    }

    /// A push/gap/resync/lagged event.
    fn on_event(&mut self, event: ClientEvent, actions: &mut Vec<Action>) {
        match event {
            ClientEvent::StateChanged {
                from,
                to,
                coalesced_through_problem,
            } => self.on_lifecycle(Some(from), to, coalesced_through_problem, actions),
            ClientEvent::Push(RoomPush::Event { room_id, event }) => {
                self.on_live_event(room_id, event, actions)
            }
            ClientEvent::Push(RoomPush::Peer {
                room_id,
                subject_id,
                device_id,
                link,
                generation,
            }) => {
                if let Some(room) = self.rooms.get_mut(&room_id) {
                    if room.is_reconciling() {
                        room.buffer_peer_push(subject_id, device_id, link, generation);
                    } else if room.is_converged() {
                        match room.apply_peer_push(subject_id, device_id, link, generation) {
                            PeerOutcome::Applied(view) => actions.push(Action::EmitView(view)),
                            PeerOutcome::Ignored => {}
                            PeerOutcome::Overflow => self.trigger(
                                &room_id,
                                ResyncReason::LocalOverflow { dropped: 1 },
                                None,
                                actions,
                            ),
                        }
                    } else {
                        room.note_parked_peer_loss(subject_id, device_id, generation);
                    }
                }
            }
            // Transfers are not a room-timeline concern (§11 non-goal).
            ClientEvent::Push(RoomPush::Transfer { .. }) => {}
            // The gap names the position it starts *after*. Anything this room
            // already applied above `from_pos` is inside the discontinuity and
            // must be discarded and re-read; dropping `from_pos` here would
            // resync from the room's own (too-high) watermark and silently keep
            // a suffix the daemon just repudiated. `truncate_to` is a no-op when
            // the watermark is already at or below `from_pos`.
            ClientEvent::Gap {
                room_id,
                from_pos,
                to,
                reason,
            } => self.trigger(
                &room_id,
                ResyncReason::Gap { reason, to },
                Some(from_pos),
                actions,
            ),
            ClientEvent::ResyncRequired { room_id, from_pos } => self.trigger(
                &room_id,
                ResyncReason::ResyncRequiredByDaemon { from_pos },
                Some(from_pos),
                actions,
            ),
            ClientEvent::Lagged { room_id, dropped } => match room_id {
                Some(room_id) => self.trigger(
                    &room_id,
                    ResyncReason::LocalOverflow { dropped },
                    None,
                    actions,
                ),
                None => {
                    for room_id in self.room_ids() {
                        self.trigger(
                            &room_id,
                            ResyncReason::LocalOverflow { dropped },
                            None,
                            actions,
                        );
                    }
                }
            },
        }
    }

    /// One live `event` push for a room.
    fn on_live_event(
        &mut self,
        room_id: RoomId,
        event: jeliya_api::Event,
        actions: &mut Vec<Action>,
    ) {
        let Some(room) = self.rooms.get_mut(&room_id) else {
            return;
        };
        if room.is_reconciling() {
            room.buffer_live_event(event);
            return;
        }
        if !room.is_converged() {
            room.buffer_parked_event(event);
            return;
        }
        if room.has_baseline() {
            match room.apply_live_event(event) {
                LiveOutcome::Applied(view) => actions.push(Action::EmitView(view)),
                LiveOutcome::Ignored => {}
                LiveOutcome::NeedResync(reason) => self.launch(&room_id, reason, None, actions),
            }
        } else {
            // A live event before any baseline (a failed/never-run bootstrap): a
            // detected discontinuity → force a baseline.
            self.launch(
                &room_id,
                ResyncReason::Gap {
                    reason: jeliya_api::GapReason::SubscriptionLapse,
                    to: jeliya_api::GapTo::Open,
                },
                None,
                actions,
            );
        }
    }

    /// A settled baseline read. Fenced by epoch and `read_id`; a stale reply is
    /// dropped, never applied over newer state (§R4).
    fn on_read_reply(
        &mut self,
        room_id: RoomId,
        read_id: u64,
        epoch: u64,
        reply: ReadReply,
        actions: &mut Vec<Action>,
    ) {
        let Some(room) = self.rooms.get_mut(&room_id) else {
            // The room was forgotten; the reply is inert.
            return;
        };
        if room.reconciling_epoch() != Some(epoch) || room.outstanding_read_id() != Some(read_id) {
            actions.push(Action::DropStale { room_id, read_id });
            return;
        }
        if self.state != State::Ready {
            room.park_outstanding();
            return;
        }
        match room.on_read_reply(reply) {
            ReplyOutcome::Retry { reason } => self.launch(&room_id, reason, None, actions),
            ReplyOutcome::NextRead(request) => actions.push(Action::IssueRead {
                room_id,
                read_id,
                epoch,
                request,
            }),
            ReplyOutcome::Converged {
                view,
                rerun,
                dropped,
            } => {
                if dropped > 0 {
                    actions.push(Action::EmitLagged {
                        room_id: room_id.clone(),
                        dropped,
                    });
                }
                actions.push(Action::EmitView(view));
                if let Some(reason) = rerun {
                    let from_pos = from_pos_for(&reason);
                    self.launch(&room_id, reason, from_pos, actions);
                }
            }
            ReplyOutcome::Restart { reason, from_pos } => {
                self.launch(&room_id, reason, Some(from_pos), actions)
            }
            // A failed read parks the room in `NeedsReconcile`. A failed settle
            // is still a settle (§R9): when a re-trigger accrued while the read
            // was outstanding, relaunch it exactly once — otherwise a coalesced
            // `resync_required` position is lost with no watermark encoding it,
            // and the room keeps applying pushes over repudiated history. With
            // no accrued trigger the room simply parks (bounded: no auto-spin).
            ReplyOutcome::Failed { rerun, .. } => {
                if let Some(reason) = rerun {
                    let from_pos = from_pos_for(&reason);
                    self.launch(&room_id, reason, from_pos, actions);
                }
            }
        }
    }

    /// An adapter resume: the same outcome as a reconnect, with **no** fabricated
    /// lifecycle transition — the core emits no [`ClientEvent`] at all (§R11).
    fn on_resume(&mut self, actions: &mut Vec<Action>) {
        self.epoch += 1;
        for room_id in self.room_ids() {
            self.launch(&room_id, ResyncReason::Resume, None, actions);
        }
    }

    /// Total stop: cancel every outstanding read and forget every room (§R13).
    fn on_stop(&mut self, actions: &mut Vec<Action>) {
        let mut rooms: Vec<_> = self.rooms.drain().collect();
        rooms.sort_by(|left, right| left.0.cmp(&right.0));
        for (room_id, room) in rooms {
            if let Some(read_id) = room.outstanding_read_id() {
                actions.push(Action::CancelRead { room_id, read_id });
            }
        }
        self.stopped = true;
    }

    /// Trigger a reconciliation for one room: coalesce into the pending re-run if
    /// it is already reconciling (single-flight, §R9); otherwise launch.
    fn trigger(
        &mut self,
        room_id: &RoomId,
        reason: ResyncReason,
        from_pos_override: Option<u64>,
        actions: &mut Vec<Action>,
    ) {
        let Some(room) = self.rooms.get_mut(room_id) else {
            return;
        };
        // A daemon-named cursor or bounded gap end proves committed history
        // the clamped read start cannot encode; record it before it can be
        // coalesced away by a stronger cause.
        room.note_trigger_evidence(&reason, from_pos_override);
        if room.is_reconciling() {
            room.coalesce_rerun(reason, from_pos_override);
        } else {
            self.launch(room_id, reason, from_pos_override, actions);
        }
    }

    /// Launch a reconciliation for one room under the current epoch, cancelling
    /// any superseded in-flight read and emitting the observable cause first.
    fn launch(
        &mut self,
        room_id: &RoomId,
        reason: ResyncReason,
        from_pos_override: Option<u64>,
        actions: &mut Vec<Action>,
    ) {
        let epoch = self.epoch;
        let read_id = self.alloc_read_id();
        let Some(room) = self.rooms.get_mut(room_id) else {
            return;
        };
        if let Some(old) = room.outstanding_read_id() {
            actions.push(Action::CancelRead {
                room_id: room_id.clone(),
                read_id: old,
            });
        }
        // Fold in a cause parked by an earlier failure so its evidence survives
        // (§R9). A parked `resync_required` names a discard position that no
        // watermark encodes, so dropping it would silently strand repudiated
        // positions in the timeline.
        room.note_trigger_evidence(&reason, from_pos_override);
        let reason = {
            let parked = room.take_pending_cause();
            room.coalesce_banking_loss(parked, reason)
        };
        room.note_trigger_evidence(&reason, None);
        let pending_gap = room.take_pending_gap_from();
        let mut effective_from = from_pos_for(&reason);
        for candidate in [from_pos_override, pending_gap].into_iter().flatten() {
            effective_from =
                Some(effective_from.map_or(candidate, |current| current.min(candidate)));
        }
        let from_pos_override = effective_from;
        let request = room.begin_reconcile(epoch, read_id, reason.clone(), from_pos_override);
        actions.push(Action::EmitResyncRequired {
            room_id: room_id.clone(),
            generation: epoch,
            reason,
        });
        actions.push(Action::IssueRead {
            room_id: room_id.clone(),
            read_id,
            epoch,
            request,
        });
    }

    /// The tracked room ids, for the relaunch-all loops (bounded by
    /// `max_active_rooms`).
    fn room_ids(&self) -> Vec<RoomId> {
        let mut room_ids: Vec<_> = self.rooms.keys().cloned().collect();
        room_ids.sort();
        room_ids
    }
}

/// The `from_pos` a rerun should re-read from: the daemon-named position for a
/// `resync_required`, else `None` (resync from the room's own watermark).
fn from_pos_for(reason: &ResyncReason) -> Option<u64> {
    match reason {
        ResyncReason::ResyncRequiredByDaemon { from_pos } => Some(*from_pos),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use jeliya_api::{
        ApiError, Cursor, DeviceId, GapReason, GapTo, Link, MemberRow, PeerRow, Reachability,
        RoomId, RoomMembersOut, RoomPeersOut, RoomTimelineOut, Standing, StreamResyncOut,
        SubjectId, Truncated,
    };

    use crate::error::CallError;
    use crate::event::{ClientEvent, RoomPush, State};
    use crate::reconcile::buffer::estimated_event_bytes;
    use crate::reconcile::reason::ResyncReason;
    use crate::reconcile::room::{ReadReply, ReadRequest};
    use crate::reconcile::view::RoomView;
    use crate::reconcile::ReconcileLimits;

    use super::{Action, Core, Input};

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn rid(s: &str) -> RoomId {
        RoomId::new(s)
    }

    fn evt(pos: u64, id: &str) -> jeliya_api::Event {
        let json = format!(
            r#"{{"pos":{pos},"event_id":"{id}","at":"1970-01-01T00:00:00Z","author":{{"state":"unresolved"}},"kind":"message","content":{{"body":"x"}}}}"#
        );
        serde_json::from_str(&json).expect("event json deserializes")
    }

    fn room_created_evt(pos: u64, id: &str) -> jeliya_api::Event {
        let json = format!(
            r#"{{"pos":{pos},"event_id":"{id}","at":"1970-01-01T00:00:00Z","author":{{"state":"unresolved"}},"kind":"room_created","content":{{"name":"room"}}}}"#
        );
        serde_json::from_str(&json).expect("room-created event json deserializes")
    }

    fn genesis_evt() -> jeliya_api::Event {
        room_created_evt(0, "genesis")
    }

    fn timeline_ok(room_id: RoomId, mut events: Vec<jeliya_api::Event>) -> ReadReply {
        // Ordinary test fixtures model a valid room history; make the mandatory
        // room-created origin explicit without weakening production validation.
        if events.first().is_none_or(|event| event.pos != 0) {
            events.insert(0, genesis_evt());
        }
        ReadReply::Timeline(Ok(RoomTimelineOut {
            room_id,
            events,
            truncated: Truncated::Complete,
        }))
    }

    fn resync_ok(room_id: RoomId, events: Vec<jeliya_api::Event>, next_pos: u64) -> ReadReply {
        ReadReply::Resync(Ok(StreamResyncOut {
            room_id,
            events,
            next_pos,
            truncated: Truncated::Complete,
        }))
    }

    fn members_ok(room_id: RoomId) -> ReadReply {
        ReadReply::Members(Ok(RoomMembersOut {
            room_id,
            members: vec![],
        }))
    }

    fn peers_ok(room_id: RoomId) -> ReadReply {
        ReadReply::Peers(Ok(RoomPeersOut {
            room_id,
            reachability: Reachability::Offline,
            peers: vec![],
        }))
    }

    /// A `room.peers` reply with an explicit reachability and roster.
    fn peers_ok_with(
        room_id: RoomId,
        reachability: Reachability,
        peers: Vec<PeerRow>,
    ) -> ReadReply {
        ReadReply::Peers(Ok(RoomPeersOut {
            room_id,
            reachability,
            peers,
        }))
    }

    /// A live `direct` link (built by deserializing, so tests carry no `time` dep).
    fn direct_link() -> Link {
        serde_json::from_str(r#"{"state":"direct","since":"1970-01-01T00:00:00Z"}"#)
            .expect("link json deserializes")
    }

    fn peer_row(subject: &str, device: &str) -> PeerRow {
        PeerRow {
            subject_id: SubjectId::new(subject),
            device_id: DeviceId::new(device),
            link: direct_link(),
        }
    }

    /// A live `peer` push for one device.
    fn peer_push(room_id: &RoomId, subject: &str, device: &str, generation: u64) -> ClientEvent {
        ClientEvent::Push(RoomPush::Peer {
            room_id: room_id.clone(),
            subject_id: SubjectId::new(subject),
            device_id: DeviceId::new(device),
            link: direct_link(),
            generation,
        })
    }

    fn timeout_err() -> CallError {
        CallError::Timeout
    }

    fn resync_required_err(room_id: RoomId, from_pos: u64) -> CallError {
        CallError::Wire(ApiError::ResyncRequired { room_id, from_pos })
    }

    /// Extract the reason from the first `EmitResyncRequired` action, if any.
    fn resync_reason(actions: &[Action]) -> Option<&ResyncReason> {
        actions.iter().find_map(|a| match a {
            Action::EmitResyncRequired { reason, .. } => Some(reason),
            _ => None,
        })
    }

    /// Extract the generation from the first `EmitResyncRequired` action.
    fn resync_generation(actions: &[Action]) -> Option<u64> {
        actions.iter().find_map(|a| match a {
            Action::EmitResyncRequired { generation, .. } => Some(*generation),
            _ => None,
        })
    }

    /// Extract the view from the first `EmitView` action, if any.
    fn emitted_view(actions: &[Action]) -> Option<&RoomView> {
        actions.iter().find_map(|a| match a {
            Action::EmitView(v) => Some(v),
            _ => None,
        })
    }

    /// The `from_pos` of the first issued `stream.resync`, if any.
    fn issued_resync_from(actions: &[Action]) -> Option<u64> {
        actions.iter().find_map(|a| match a {
            Action::IssueRead {
                request: ReadRequest::Resync(resync),
                ..
            } => Some(resync.from_pos),
            _ => None,
        })
    }

    /// Count the number of `IssueRead` actions.
    fn issue_read_count(actions: &[Action]) -> usize {
        actions
            .iter()
            .filter(|a| matches!(a, Action::IssueRead { .. }))
            .count()
    }

    /// True if any action is a `CancelRead`.
    fn has_cancel_read(actions: &[Action]) -> bool {
        actions
            .iter()
            .any(|a| matches!(a, Action::CancelRead { .. }))
    }

    /// True if any action is a `DropStale`.
    fn has_drop_stale(actions: &[Action]) -> bool {
        actions
            .iter()
            .any(|a| matches!(a, Action::DropStale { .. }))
    }

    /// True if any action is an `EmitView`.
    fn has_emit_view(actions: &[Action]) -> bool {
        actions.iter().any(|a| matches!(a, Action::EmitView(_)))
    }

    fn emitted_lagged(actions: &[Action]) -> Option<u64> {
        actions.iter().find_map(|action| match action {
            Action::EmitLagged { dropped, .. } => Some(*dropped),
            _ => None,
        })
    }

    /// Extract `(read_id, epoch)` from the first `IssueRead`.
    fn read_id_epoch(actions: &[Action]) -> (u64, u64) {
        actions
            .iter()
            .find_map(|a| match a {
                Action::IssueRead { read_id, epoch, .. } => Some((*read_id, *epoch)),
                _ => None,
            })
            .expect("expected at least one IssueRead action")
    }

    /// Extract `(read_id, epoch)` from the first `IssueRead` for a specific room.
    fn read_id_epoch_for(actions: &[Action], room_id: &RoomId) -> (u64, u64) {
        actions
            .iter()
            .find_map(|a| match a {
                Action::IssueRead {
                    room_id: rid,
                    read_id,
                    epoch,
                    ..
                } if rid == room_id => Some((*read_id, *epoch)),
                _ => None,
            })
            .expect("expected an IssueRead for this room")
    }

    /// Drive a room through a full bootstrap (Timeline → Members → Peers).
    ///
    /// Works whether the core is in initial (pre-Ready) or already-Ready state:
    /// - Pre-Ready: `ActivateRoom` produces nothing; we send one `Lifecycle::Ready`
    ///   which may also trigger reconnects for previously-converged rooms.
    /// - Already-Ready: `ActivateRoom` immediately issues the bootstrap read.
    ///
    /// Returns the actions from the final `Peers` reply (contains `EmitView`).
    fn complete_bootstrap(
        core: &mut Core,
        room_id: &RoomId,
        events: Vec<jeliya_api::Event>,
    ) -> Vec<Action> {
        let from_pos = events.last().map_or(0, |event| event.pos);
        let activate_actions = core.step(Input::ActivateRoom {
            room_id: room_id.clone(),
            from_pos,
        });

        // If ActivateRoom already issued a read (core already in Ready state),
        // use those actions; otherwise fire a Lifecycle::Ready to start bootstrap.
        let bootstrap_actions = if activate_actions
            .iter()
            .any(|a| matches!(a, Action::IssueRead { .. }))
        {
            activate_actions
        } else {
            core.step(Input::Lifecycle {
                to: State::Ready,
                coalesced_through_problem: false,
            })
        };

        let (read_id, epoch) = read_id_epoch_for(&bootstrap_actions, room_id);

        // Timeline reply
        let _ = core.step(Input::ReadReply {
            room_id: room_id.clone(),
            read_id,
            epoch,
            reply: timeline_ok(room_id.clone(), events),
        });
        // Members reply
        let _ = core.step(Input::ReadReply {
            room_id: room_id.clone(),
            read_id,
            epoch,
            reply: members_ok(room_id.clone()),
        });
        // Peers reply → convergence
        core.step(Input::ReadReply {
            room_id: room_id.clone(),
            read_id,
            epoch,
            reply: peers_ok(room_id.clone()),
        })
    }

    // -----------------------------------------------------------------------
    // AC-1 — Every gap reason is observable
    // -----------------------------------------------------------------------

    #[test]
    fn ac1_bootstrap_reason_emitted_at_first_ready() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 0,
        });
        let actions = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        assert!(
            matches!(resync_reason(&actions), Some(ResyncReason::Bootstrap)),
            "first Ready must emit Bootstrap reason"
        );
    }

    #[test]
    fn duplicate_ready_does_not_cancel_bootstrap_or_bump_epoch() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 0,
        });
        let first = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (read_id, epoch) = read_id_epoch_for(&first, &room);

        let duplicate = core.step(Input::LifecycleObserved {
            from: State::Ready,
            to: State::Ready,
            coalesced_through_problem: false,
        });
        assert_eq!(core.epoch(), epoch, "duplicate Ready must not bump epoch");
        assert_eq!(
            issue_read_count(&duplicate),
            0,
            "duplicate Ready must not launch a replacement read"
        );
        assert!(
            !duplicate.iter().any(|action| matches!(
                action,
                Action::CancelRead { read_id: id, .. } if *id == read_id
            )),
            "duplicate Ready must not cancel the bootstrap"
        );
    }

    #[test]
    fn ac1_reconnect_reason_emitted_after_first_convergence() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        complete_bootstrap(&mut core, &room, vec![]);

        // Second Ready (reconnect)
        let actions = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        assert!(
            matches!(resync_reason(&actions), Some(ResyncReason::Reconnect)),
            "second Ready must emit Reconnect reason, not Bootstrap"
        );
    }

    #[test]
    fn ac1_gap_reason_preserves_wire_cause() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        complete_bootstrap(&mut core, &room, vec![evt(1, "e1")]);

        let actions = core.step(Input::Event(ClientEvent::Gap {
            room_id: room.clone(),
            from_pos: 1,
            to: GapTo::Bounded { pos: 3 },
            reason: GapReason::Retention,
        }));
        match resync_reason(&actions) {
            Some(ResyncReason::Gap { reason, to }) => {
                assert_eq!(*reason, GapReason::Retention);
                assert_eq!(*to, GapTo::Bounded { pos: 3 });
            }
            other => panic!("expected Gap reason, got {other:?}"),
        }
    }

    #[test]
    fn ac1_local_overflow_reason_from_lagged_event() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        complete_bootstrap(&mut core, &room, vec![evt(1, "e1")]);

        let actions = core.step(Input::Event(ClientEvent::Lagged {
            room_id: Some(room.clone()),
            dropped: 7,
        }));
        assert!(
            matches!(
                resync_reason(&actions),
                Some(ResyncReason::LocalOverflow { dropped: 7 })
            ),
            "Lagged event must emit LocalOverflow reason"
        );
    }

    #[test]
    fn ac1_lagged_with_no_room_triggers_all_active_rooms() {
        let mut core = Core::new(ReconcileLimits::default());
        let r1 = rid("r1");
        let r2 = rid("r2");
        complete_bootstrap(&mut core, &r1, vec![]);
        complete_bootstrap(&mut core, &r2, vec![]);

        let actions = core.step(Input::Event(ClientEvent::Lagged {
            room_id: None,
            dropped: 3,
        }));
        let resync_rooms: Vec<_> = actions
            .iter()
            .filter_map(|a| match a {
                Action::EmitResyncRequired { room_id, .. } => Some(room_id.clone()),
                _ => None,
            })
            .collect();
        assert!(
            resync_rooms.contains(&r1),
            "all-room Lagged must trigger r1"
        );
        assert!(
            resync_rooms.contains(&r2),
            "all-room Lagged must trigger r2"
        );
    }

    #[test]
    fn ac1_resync_required_by_daemon_from_client_event() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        complete_bootstrap(&mut core, &room, vec![evt(1, "e1")]);

        let actions = core.step(Input::Event(ClientEvent::ResyncRequired {
            room_id: room.clone(),
            from_pos: 1,
        }));
        assert!(
            matches!(
                resync_reason(&actions),
                Some(ResyncReason::ResyncRequiredByDaemon { from_pos: 1 })
            ),
            "ResyncRequired event must emit ResyncRequiredByDaemon reason"
        );
    }

    #[test]
    fn ac1_resume_reason_from_resume_input() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        complete_bootstrap(&mut core, &room, vec![]);

        let actions = core.step(Input::Resume);
        assert!(
            matches!(resync_reason(&actions), Some(ResyncReason::Resume)),
            "Input::Resume must emit Resume reason"
        );
    }

    // -----------------------------------------------------------------------
    // AC-2 — Reconciliation is serialized and coalesced
    // -----------------------------------------------------------------------

    #[test]
    fn ac2_single_flight_coalesces_gap_during_outstanding_read() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        // Activate + Ready → bootstrap outstanding
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 0,
        });
        core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });

        // A gap arrives while the bootstrap read is in flight
        let gap_actions = core.step(Input::Event(ClientEvent::Gap {
            room_id: room.clone(),
            from_pos: 0,
            to: GapTo::Open,
            reason: GapReason::Backpressure,
        }));
        // Should coalesce into a rerun, NOT launch a second IssueRead
        assert_eq!(
            issue_read_count(&gap_actions),
            0,
            "gap during outstanding read must coalesce, not issue a second read"
        );
        assert!(!has_emit_view(&gap_actions));
    }

    #[test]
    fn ac2_coalescing_keeps_strongest_cause_reconnect_over_gap() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 0,
        });
        // First Ready → bootstrap in flight
        core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });

        // Gap coalesces as pending rerun
        core.step(Input::Event(ClientEvent::Gap {
            room_id: room.clone(),
            from_pos: 0,
            to: GapTo::Open,
            reason: GapReason::SubscriptionLapse,
        }));

        // Reconnect coalesces — must overwrite the weaker Gap
        core.step(Input::Event(ClientEvent::StateChanged {
            from: State::Interrupted,
            to: State::Ready,
            coalesced_through_problem: false,
        }));
        // Second Ready bumps epoch and relaunches; the coalesced rerun from the
        // previous reconciliation is also queued, but since the room was
        // relaunched by the Ready the pending rerun is irrelevant here.
        // The key assertion: no concurrent second read was issued for the Gap.
        // (The reconnect correctly relaunches with Reconnect reason.)
        let relaunch_actions = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        assert!(
            matches!(
                resync_reason(&relaunch_actions),
                Some(ResyncReason::Reconnect)
            ),
            "reconnect after gap must use Reconnect reason"
        );
    }

    #[test]
    fn ac2_coalesced_rerun_launches_once_after_convergence() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 0,
        });
        let a = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (read_id, epoch) = read_id_epoch(&a);

        // Coalesce a gap into the pending rerun while bootstrap is in flight
        core.step(Input::Event(ClientEvent::Gap {
            room_id: room.clone(),
            from_pos: 0,
            to: GapTo::Open,
            reason: GapReason::SubscriptionLapse,
        }));

        // Settle timeline
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: timeline_ok(room.clone(), vec![]),
        });
        // Members
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: members_ok(room.clone()),
        });
        // Peers → convergence, rerun launches immediately
        let a = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: peers_ok(room.clone()),
        });
        // The coalesced cause is a publication barrier; only the authoritative
        // rerun may publish.
        assert!(!has_emit_view(&a), "superseded view must be suppressed");
        assert_eq!(
            issue_read_count(&a),
            1,
            "exactly one rerun read launched after convergence"
        );
        assert!(
            matches!(resync_reason(&a), Some(ResyncReason::Gap { .. })),
            "rerun reason must match the coalesced gap"
        );
    }

    // -----------------------------------------------------------------------
    // AC-3 — Overflow cannot permanently deduplicate an undelivered event
    // -----------------------------------------------------------------------

    #[test]
    fn ac3_buffer_overflow_coalesces_local_overflow_rerun() {
        let limits = ReconcileLimits {
            buffer_depth: 1,
            ..ReconcileLimits::default()
        };
        let mut core = Core::new(limits);
        let room = rid("r");
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 0,
        });
        core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });

        // First push: fits in the depth-1 buffer
        core.step(Input::Event(ClientEvent::Push(RoomPush::Event {
            room_id: room.clone(),
            event: evt(1, "e1"),
        })));
        // Second push: overflow — must trigger LocalOverflow rerun, not silent drop
        let overflow_actions = core.step(Input::Event(ClientEvent::Push(RoomPush::Event {
            room_id: room.clone(),
            event: evt(2, "e2"),
        })));
        // No second IssueRead yet (still coalescing), but the internal rerun is set.
        // The overflow itself emits no immediate action — the rerun fires at convergence.
        // What we assert: no phantom IssueRead launched and no EmitView yet.
        assert_eq!(
            issue_read_count(&overflow_actions),
            0,
            "overflow must coalesce into rerun, not issue a second read immediately"
        );
    }

    #[test]
    fn ac3_overflow_rerun_recovers_dropped_events_via_rebaseline() {
        let limits = ReconcileLimits {
            buffer_depth: 1,
            ..ReconcileLimits::default()
        };
        let mut core = Core::new(limits);
        let room = rid("r");
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 0,
        });
        let a = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (read_id, epoch) = read_id_epoch(&a);

        // Buffer the first push (fits)
        core.step(Input::Event(ClientEvent::Push(RoomPush::Event {
            room_id: room.clone(),
            event: evt(1, "e1"),
        })));
        // Overflow the second
        core.step(Input::Event(ClientEvent::Push(RoomPush::Event {
            room_id: room.clone(),
            event: evt(2, "e2"),
        })));

        // Settle the bootstrap timeline (empty — events came via pushes)
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: timeline_ok(room.clone(), vec![]),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: members_ok(room.clone()),
        });
        let a = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: peers_ok(room.clone()),
        });
        // Known overflow is a publication barrier while LocalOverflow reruns.
        assert!(!has_emit_view(&a), "incomplete convergence is suppressed");
        assert_eq!(issue_read_count(&a), 1, "rerun read issued after overflow");
        assert!(
            matches!(resync_reason(&a), Some(ResyncReason::LocalOverflow { .. })),
            "rerun reason is LocalOverflow"
        );
        // The rerun is an Incremental resync (resync from watermark).
        // When it settles with e2 included, the final view must contain both events.
        let (rerun_read_id, rerun_epoch) = read_id_epoch(&a);
        // Settle rerun resync with e2
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: rerun_read_id,
            epoch: rerun_epoch,
            reply: resync_ok(room.clone(), vec![evt(2, "e2")], 2),
        });
        // Local overflow also refreshes presence, because the lost frame may
        // have been a peer push.
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: rerun_read_id,
            epoch: rerun_epoch,
            reply: members_ok(room.clone()),
        });
        let a = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: rerun_read_id,
            epoch: rerun_epoch,
            reply: peers_ok(room.clone()),
        });
        let view = emitted_view(&a).expect("rerun convergence emits view");
        assert_eq!(
            view.timeline
                .iter()
                .map(|event| event.pos)
                .collect::<Vec<_>>(),
            vec![0, 1, 2],
            "genesis and both events must be present after rerun"
        );
        assert!(
            view.timeline.iter().any(|e| e.event_id.as_str() == "e2"),
            "dropped event e2 must be recovered by the rerun"
        );
    }

    // -----------------------------------------------------------------------
    // AC-4 — Baseline and buffered pushes converge by event ID and signed timestamp
    // -----------------------------------------------------------------------

    #[test]
    fn ac4_contiguous_buffered_push_applies_after_baseline() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 1,
        });
        let a = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (read_id, epoch) = read_id_epoch(&a);

        // Buffer a push at pos=2 while bootstrap is outstanding
        core.step(Input::Event(ClientEvent::Push(RoomPush::Event {
            room_id: room.clone(),
            event: evt(2, "e2"),
        })));

        // Settle with pos=1 in the timeline baseline
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: timeline_ok(room.clone(), vec![evt(1, "e1")]),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: members_ok(room.clone()),
        });
        let a = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: peers_ok(room.clone()),
        });
        // After convergence the view includes genesis, pos=1 from the
        // baseline, and pos=2 from the live buffer.
        let view = emitted_view(&a).expect("view emitted after convergence");
        assert_eq!(
            view.timeline
                .iter()
                .map(|event| event.pos)
                .collect::<Vec<_>>(),
            vec![0, 1, 2],
            "timeline must contain genesis, baseline, and buffered event"
        );
    }

    #[test]
    fn ac4_duplicate_event_id_in_buffer_is_deduplicated() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 0,
        });
        let a = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (read_id, epoch) = read_id_epoch(&a);

        // Buffer a duplicate push for e1 (same pos, same event_id)
        core.step(Input::Event(ClientEvent::Push(RoomPush::Event {
            room_id: room.clone(),
            event: evt(1, "e1"),
        })));

        // Baseline already includes e1
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: timeline_ok(room.clone(), vec![evt(1, "e1")]),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: members_ok(room.clone()),
        });
        let a = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: peers_ok(room.clone()),
        });
        let view = emitted_view(&a).expect("duplicate test must reach convergence");
        assert_eq!(
            view.timeline
                .iter()
                .map(|event| event.pos)
                .collect::<Vec<_>>(),
            vec![0, 1],
            "duplicate buffered event must be deduped without dropping genesis"
        );
    }

    #[test]
    fn ac4_out_of_order_buffer_push_triggers_rerun() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 0,
        });
        let a = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (read_id, epoch) = read_id_epoch(&a);

        // Buffer a push at pos=5 — there is a gap vs the baseline (watermark=1)
        core.step(Input::Event(ClientEvent::Push(RoomPush::Event {
            room_id: room.clone(),
            event: evt(5, "e5"),
        })));

        // Settle with only pos=1 in the baseline
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: timeline_ok(room.clone(), vec![evt(1, "e1")]),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: members_ok(room.clone()),
        });
        let a = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: peers_ok(room.clone()),
        });
        // The buffered gap is a publication barrier until the rerun settles.
        assert!(!has_emit_view(&a), "incomplete view must be suppressed");
        assert_eq!(
            issue_read_count(&a),
            1,
            "out-of-order buffer push must trigger a rerun read"
        );
    }

    #[test]
    fn dedup_window_covers_the_rendered_timeline_when_configured_smaller() {
        let mut core = Core::new(ReconcileLimits {
            dedup_window: 1,
            timeline_depth: 3,
            ..ReconcileLimits::default()
        });
        let room = rid("r");
        let _ = complete_bootstrap(
            &mut core,
            &room,
            vec![evt(1, "old"), evt(2, "middle"), evt(3, "new")],
        );
        let duplicate = core.step(Input::Event(ClientEvent::Push(RoomPush::Event {
            room_id: room,
            event: evt(4, "old"),
        })));
        assert!(matches!(
            resync_reason(&duplicate),
            Some(ResyncReason::Gap { .. })
        ));
        assert!(!has_emit_view(&duplicate));
    }

    #[test]
    fn daemon_discard_rebuilds_ids_for_the_whole_rendered_window() {
        let mut core = Core::new(ReconcileLimits {
            dedup_window: 1,
            timeline_depth: 3,
            ..ReconcileLimits::default()
        });
        let room = rid("r");
        complete_bootstrap(
            &mut core,
            &room,
            vec![evt(1, "old"), evt(2, "middle"), evt(3, "new")],
        );
        let started = core.step(Input::Event(ClientEvent::ResyncRequired {
            room_id: room.clone(),
            from_pos: 3,
        }));
        let (read_id, epoch) = read_id_epoch_for(&started, &room);
        let rejected = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: resync_ok(room, vec![evt(4, "old")], 4),
        });
        assert!(matches!(
            resync_reason(&rejected),
            Some(ResyncReason::Gap { .. })
        ));
        assert!(!has_emit_view(&rejected));
    }

    #[test]
    fn live_id_reuse_beyond_render_and_recent_windows_fails_closed() {
        let mut core = Core::new(ReconcileLimits {
            dedup_window: 1,
            timeline_depth: 2,
            ..ReconcileLimits::default()
        });
        let room = rid("r");
        complete_bootstrap(&mut core, &room, vec![evt(1, "e1"), evt(2, "e2")]);
        assert!(has_emit_view(&core.step(Input::Event(ClientEvent::Push(
            RoomPush::Event {
                room_id: room.clone(),
                event: evt(3, "e3"),
            },
        )))));
        assert!(has_emit_view(&core.step(Input::Event(ClientEvent::Push(
            RoomPush::Event {
                room_id: room.clone(),
                event: evt(4, "e4"),
            },
        )))));
        let reused = core.step(Input::Event(ClientEvent::Push(RoomPush::Event {
            room_id: room,
            event: evt(5, "genesis"),
        })));
        assert!(matches!(
            resync_reason(&reused),
            Some(ResyncReason::Gap { .. })
        ));
        assert!(!has_emit_view(&reused));
    }

    #[test]
    fn daemon_truncation_allows_same_ids_in_the_replaced_suffix() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        complete_bootstrap(
            &mut core,
            &room,
            vec![evt(1, "e1"), evt(2, "e2"), evt(3, "e3")],
        );
        let started = core.step(Input::Event(ClientEvent::ResyncRequired {
            room_id: room.clone(),
            from_pos: 1,
        }));
        let (read_id, epoch) = read_id_epoch_for(&started, &room);
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: resync_ok(room.clone(), vec![evt(2, "e2"), evt(3, "e3")], 3),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: members_ok(room.clone()),
        });
        let settled = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: peers_ok(room),
        });
        let view = emitted_view(&settled).expect("repudiated suffix ids may be re-read");
        assert_eq!(
            view.timeline
                .iter()
                .map(|event| event.pos)
                .collect::<Vec<_>>(),
            vec![0, 1, 2, 3]
        );
    }

    #[test]
    fn duplicate_recent_id_above_watermark_forces_a_gap() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        complete_bootstrap(&mut core, &room, vec![evt(1, "e1")]);

        let started = core.step(Input::Event(ClientEvent::Gap {
            room_id: room.clone(),
            from_pos: 1,
            to: GapTo::Open,
            reason: GapReason::SubscriptionLapse,
        }));
        let (read_id, epoch) = read_id_epoch_for(&started, &room);
        // Same identity at a new position cannot be silently discarded: doing
        // so leaves the position after the watermark unexplained.
        core.step(Input::Event(ClientEvent::Push(RoomPush::Event {
            room_id: room.clone(),
            event: evt(2, "e1"),
        })));
        let settled = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: resync_ok(room.clone(), vec![], 1),
        });
        assert_eq!(
            issue_read_count(&settled),
            1,
            "a recent-id hit above the watermark must trigger a recovery"
        );
        assert!(
            matches!(resync_reason(&settled), Some(ResyncReason::Gap { .. })),
            "the unexplained position must be surfaced as a gap"
        );
    }

    #[test]
    fn daemon_cursor_above_first_baseline_does_not_adopt_unread_history() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 0,
        });
        let initial = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (read_id, epoch) = read_id_epoch_for(&initial, &room);
        core.step(Input::Event(ClientEvent::ResyncRequired {
            room_id: room.clone(),
            from_pos: 5,
        }));
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: timeline_ok(room.clone(), vec![]),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: members_ok(room.clone()),
        });
        let relaunched = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: peers_ok(room.clone()),
        });
        let (rerun_id, rerun_epoch) = read_id_epoch_for(&relaunched, &room);

        // A daemon position above the newly established genesis watermark is
        // not evidence that this client holds the intervening prefix. The
        // malformed incremental reply is rejected immediately and the old read
        // identity is superseded by one bounded structural recovery.
        let recovery = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: rerun_id,
            epoch: rerun_epoch,
            reply: resync_ok(room.clone(), vec![evt(6, "e6")], 6),
        });
        assert_eq!(
            issue_read_count(&recovery),
            1,
            "a later event cannot advance past never-read history"
        );
        assert!(
            matches!(resync_reason(&recovery), Some(ResyncReason::Gap { .. })),
            "the missing prefix must be surfaced as a gap"
        );
    }

    #[test]
    fn unsorted_baseline_page_cannot_hide_a_position_gap() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 2,
        });
        let actions = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (read_id, epoch) = read_id_epoch_for(&actions, &room);
        let invalid = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: timeline_ok(room.clone(), vec![evt(2, "e2"), evt(1, "e1")]),
        });
        assert_eq!(issue_read_count(&invalid), 1);
        assert!(matches!(
            resync_reason(&invalid),
            Some(ResyncReason::Gap { .. })
        ));
        assert!(!has_emit_view(&invalid));
    }

    /// The subscription anchor is a completeness floor, not a stop cursor: a
    /// baseline that reaches `Complete` below a non-zero anchor is missing a
    /// committed event and must never publish the partial prefix.
    #[test]
    fn bootstrap_complete_below_the_anchor_is_a_structural_gap() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 2,
        });
        let actions = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (read_id, epoch) = read_id_epoch_for(&actions, &room);

        let rejected = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: ReadReply::Timeline(Ok(RoomTimelineOut {
                room_id: room.clone(),
                events: vec![genesis_evt(), evt(1, "e1")],
                truncated: Truncated::Complete,
            })),
        });
        assert!(matches!(
            resync_reason(&rejected),
            Some(ResyncReason::Gap { .. })
        ));
        assert!(!has_emit_view(&rejected));
    }

    #[test]
    fn later_room_created_event_is_rejected_as_a_structural_gap() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 2,
        });
        let started = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (read_id, epoch) = read_id_epoch_for(&started, &room);
        let rejected = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: ReadReply::Timeline(Ok(RoomTimelineOut {
                room_id: room,
                events: vec![
                    genesis_evt(),
                    room_created_evt(1, "second-origin"),
                    evt(2, "e2"),
                ],
                truncated: Truncated::Complete,
            })),
        });
        assert!(matches!(
            resync_reason(&rejected),
            Some(ResyncReason::Gap { .. })
        ));
        assert!(!has_emit_view(&rejected));
    }

    #[test]
    fn live_room_created_event_cannot_create_a_second_origin() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        complete_bootstrap(&mut core, &room, vec![evt(1, "e1")]);
        let rejected = core.step(Input::Event(ClientEvent::Push(RoomPush::Event {
            room_id: room,
            event: room_created_evt(2, "second-origin"),
        })));
        assert!(matches!(
            resync_reason(&rejected),
            Some(ResyncReason::Gap { .. })
        ));
        assert!(!has_emit_view(&rejected));
    }

    #[test]
    fn buffered_room_created_event_cannot_create_a_second_origin() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 1,
        });
        let started = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (read_id, epoch) = read_id_epoch_for(&started, &room);
        core.step(Input::Event(ClientEvent::Push(RoomPush::Event {
            room_id: room.clone(),
            event: room_created_evt(2, "second-origin"),
        })));
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: timeline_ok(room.clone(), vec![evt(1, "e1")]),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: members_ok(room.clone()),
        });
        let rejected = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: peers_ok(room),
        });
        assert!(matches!(
            resync_reason(&rejected),
            Some(ResyncReason::Gap { .. })
        ));
        assert!(
            !has_emit_view(&rejected),
            "a rejected buffered origin is a publication barrier"
        );
    }

    #[test]
    fn incremental_resync_cannot_introduce_a_second_room_origin() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        complete_bootstrap(&mut core, &room, vec![evt(1, "e1")]);
        let started = core.step(Input::Event(ClientEvent::Gap {
            room_id: room.clone(),
            from_pos: 1,
            to: GapTo::Open,
            reason: GapReason::SubscriptionLapse,
        }));
        let (read_id, epoch) = read_id_epoch_for(&started, &room);
        let rejected = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: resync_ok(room, vec![room_created_evt(2, "second-origin")], 2),
        });
        assert!(matches!(
            resync_reason(&rejected),
            Some(ResyncReason::Gap { .. })
        ));
        assert!(!has_emit_view(&rejected));
    }

    #[test]
    fn malformed_suffix_after_anchor_is_still_rejected() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 1,
        });
        let started = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (read_id, epoch) = read_id_epoch_for(&started, &room);
        let rejected = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: ReadReply::Timeline(Ok(RoomTimelineOut {
                room_id: room,
                // Position two is absent. Even though the subscription anchor
                // was reached at one, a malformed returned suffix must not be
                // accepted as an authoritative response.
                events: vec![genesis_evt(), evt(1, "e1"), evt(3, "e3")],
                truncated: Truncated::Complete,
            })),
        });
        assert!(matches!(
            resync_reason(&rejected),
            Some(ResyncReason::Gap { .. })
        ));
        assert!(!has_emit_view(&rejected));
    }

    #[test]
    fn changed_anchor_does_not_launch_replacement_while_interrupted() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 1,
        });
        let started = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (read_id, epoch) = read_id_epoch_for(&started, &room);
        core.step(Input::Lifecycle {
            to: State::Interrupted,
            coalesced_through_problem: false,
        });
        let deferred = core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 2,
        });
        assert_eq!(issue_read_count(&deferred), 0);

        let old_settles_offline = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: timeline_ok(room.clone(), vec![evt(1, "e1")]),
        });
        assert_eq!(
            issue_read_count(&old_settles_offline),
            0,
            "settling old I/O must not feed a dead transport"
        );
        assert!(!has_emit_view(&old_settles_offline));

        let replacement = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        assert_eq!(issue_read_count(&replacement), 1);
        assert!(replacement.iter().any(|action| matches!(
            action,
            Action::IssueRead {
                request: ReadRequest::Timeline(_),
                ..
            }
        )));
    }

    #[test]
    fn anchor_change_during_reconcile_suppresses_old_view_and_forces_replacement() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 1,
        });
        let started = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (old_read_id, old_epoch) = read_id_epoch_for(&started, &room);

        // A queued gap is the stronger observable cause, but the subsequent
        // anchor change independently requires a full replacement. Reactivation
        // must cancel a stalled old read immediately rather than waiting for it.
        core.step(Input::Event(ClientEvent::Gap {
            room_id: room.clone(),
            from_pos: 0,
            to: GapTo::Open,
            reason: GapReason::SubscriptionLapse,
        }));
        let replacement = core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 2,
        });
        assert!(has_cancel_read(&replacement));
        assert!(!has_emit_view(&replacement));
        assert!(matches!(
            resync_reason(&replacement),
            Some(ResyncReason::Gap { .. })
        ));
        assert!(
            replacement.iter().any(|action| matches!(
                action,
                Action::IssueRead {
                    request: ReadRequest::Timeline(_),
                    ..
                }
            )),
            "replacement mode must survive cause coalescing"
        );

        let stale = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: old_read_id,
            epoch: old_epoch,
            reply: timeline_ok(room.clone(), vec![evt(1, "e1")]),
        });
        assert!(has_drop_stale(&stale));
        assert!(!has_emit_view(&stale));

        let (replacement_id, replacement_epoch) = read_id_epoch_for(&replacement, &room);
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: replacement_id,
            epoch: replacement_epoch,
            reply: timeline_ok(room.clone(), vec![evt(1, "e1"), evt(2, "e2")]),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: replacement_id,
            epoch: replacement_epoch,
            reply: members_ok(room.clone()),
        });
        let settled = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: replacement_id,
            epoch: replacement_epoch,
            reply: peers_ok(room),
        });
        let view = emitted_view(&settled).expect("replacement baseline must converge");
        assert_eq!(
            view.timeline
                .iter()
                .map(|event| event.pos)
                .collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
    }

    #[test]
    fn requested_page_size_never_exceeds_the_accepted_page_bound() {
        let mut core = Core::new(ReconcileLimits {
            read_page_size: 100,
            max_read_page_events: 2,
            ..ReconcileLimits::default()
        });
        let room = rid("r");
        core.step(Input::ActivateRoom {
            room_id: room,
            from_pos: 0,
        });
        let actions = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let limit = actions.iter().find_map(|action| match action {
            Action::IssueRead {
                request: ReadRequest::Timeline(request),
                ..
            } => Some(request.page.limit),
            _ => None,
        });
        assert_eq!(limit, Some(2));
    }

    #[test]
    fn timeline_reply_count_cannot_exceed_the_configured_page_bound() {
        let mut core = Core::new(ReconcileLimits {
            read_page_size: 10,
            max_read_page_events: 1,
            ..ReconcileLimits::default()
        });
        let room = rid("r");
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 1,
        });
        let started = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (read_id, epoch) = read_id_epoch_for(&started, &room);
        let rejected = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: timeline_ok(room, vec![evt(1, "e1")]),
        });
        assert!(matches!(
            resync_reason(&rejected),
            Some(ResyncReason::Gap { .. })
        ));
        assert!(!has_emit_view(&rejected));
    }

    #[test]
    fn rendered_timeline_obeys_its_byte_bound_independently_of_watermark() {
        let events = [genesis_evt(), evt(1, "e1"), evt(2, "e2")];
        let byte_cap = events
            .iter()
            .map(estimated_event_bytes)
            .max()
            .expect("fixtures");
        let mut core = Core::new(ReconcileLimits {
            timeline_bytes: byte_cap,
            ..ReconcileLimits::default()
        });
        let room = rid("r");
        let settled = complete_bootstrap(&mut core, &room, vec![evt(1, "e1"), evt(2, "e2")]);
        let view = emitted_view(&settled).expect("byte-bounded baseline converges");
        assert_eq!(
            view.timeline.last().map(|event| event.pos),
            Some(2),
            "the newest event remains rendered"
        );
        assert!(
            view.timeline
                .iter()
                .map(estimated_event_bytes)
                .fold(0_u64, u64::saturating_add)
                <= byte_cap
        );

        let extended = core.step(Input::Event(ClientEvent::Push(RoomPush::Event {
            room_id: room,
            event: evt(3, "e3"),
        })));
        let extended = emitted_view(&extended).expect("watermark advances past evicted history");
        assert_eq!(extended.timeline.last().map(|event| event.pos), Some(3));
    }

    #[test]
    fn oversized_room_identifier_is_not_retained_by_the_core() {
        let mut core = Core::new(ReconcileLimits {
            max_identifier_bytes: 3,
            ..ReconcileLimits::default()
        });
        let actions = core.step(Input::ActivateRoom {
            room_id: rid("room"),
            from_pos: 0,
        });
        assert!(actions.is_empty());
        assert_eq!(core.tracked_rooms(), 0);
    }

    #[test]
    fn bootstrap_rejects_invalid_more_cursor_even_after_anchor() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 1,
        });
        let started = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (read_id, epoch) = read_id_epoch_for(&started, &room);
        let rejected = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: ReadReply::Timeline(Ok(RoomTimelineOut {
                room_id: room,
                events: vec![genesis_evt(), evt(1, "e1")],
                truncated: Truncated::More {
                    cursor: Cursor::At { pos: 999 },
                },
            })),
        });
        assert!(matches!(
            resync_reason(&rejected),
            Some(ResyncReason::Gap { .. })
        ));
        assert!(!has_emit_view(&rejected));
    }

    #[test]
    fn live_contiguous_event_extends_a_converged_view() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        complete_bootstrap(&mut core, &room, vec![evt(1, "e1")]);

        let actions = core.step(Input::Event(ClientEvent::Push(RoomPush::Event {
            room_id: room.clone(),
            event: evt(2, "e2"),
        })));
        let view = emitted_view(&actions).expect("contiguous live event must emit a view");
        assert_eq!(
            view.timeline
                .iter()
                .map(|event| event.pos)
                .collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert_eq!(view.timeline[2].event_id.as_str(), "e2");
    }

    // -----------------------------------------------------------------------
    // AC-5 — Peer state is replaced from authoritative reads, never merged
    // -----------------------------------------------------------------------

    #[test]
    fn daemon_discard_below_render_window_keeps_resync_contiguous() {
        let limits = ReconcileLimits {
            timeline_depth: 2,
            ..ReconcileLimits::default()
        };
        let mut core = Core::new(limits);
        let room = rid("r");
        let _ = complete_bootstrap(
            &mut core,
            &room,
            vec![evt(1, "e1"), evt(2, "e2"), evt(3, "e3"), evt(4, "e4")],
        );
        let started = core.step(Input::Event(ClientEvent::ResyncRequired {
            room_id: room.clone(),
            from_pos: 1,
        }));
        let (read_id, epoch) = read_id_epoch_for(&started, &room);
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: resync_ok(room.clone(), vec![evt(2, "e2"), evt(3, "e3")], 3),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: members_ok(room.clone()),
        });
        let settled = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: peers_ok(room.clone()),
        });
        let view = emitted_view(&settled).expect("discard recovery must converge");
        assert_eq!(
            view.timeline
                .iter()
                .map(|event| event.pos)
                .collect::<Vec<_>>(),
            vec![2, 3]
        );
        assert_eq!(issue_read_count(&settled), 0);
    }

    #[test]
    fn paged_baseline_collection_stays_within_the_transient_bound() {
        let limits = ReconcileLimits {
            timeline_depth: 1,
            read_page_size: 1,
            ..ReconcileLimits::default()
        };
        let mut core = Core::new(limits);
        let room = rid("r");
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 3,
        });
        let actions = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (read_id, epoch) = read_id_epoch_for(&actions, &room);
        for (event, next_pos) in [(genesis_evt(), 1), (evt(1, "e1"), 2), (evt(2, "e2"), 3)] {
            let next = core.step(Input::ReadReply {
                room_id: room.clone(),
                read_id,
                epoch,
                reply: ReadReply::Timeline(Ok(RoomTimelineOut {
                    room_id: room.clone(),
                    events: vec![event],
                    truncated: Truncated::More {
                        cursor: Cursor::At { pos: next_pos },
                    },
                })),
            });
            assert_eq!(issue_read_count(&next), 1);
        }
        // This is a continuation page, so it must not use `timeline_ok`, whose
        // fixture convenience would prepend a second genesis event.
        let members = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: ReadReply::Timeline(Ok(RoomTimelineOut {
                room_id: room.clone(),
                events: vec![evt(3, "e3")],
                truncated: Truncated::Complete,
            })),
        });
        assert!(members.iter().any(|action| matches!(
            action,
            Action::IssueRead {
                request: ReadRequest::Members(_),
                ..
            }
        )));
        let peers = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: members_ok(room.clone()),
        });
        assert!(peers.iter().any(|action| matches!(
            action,
            Action::IssueRead {
                request: ReadRequest::Peers(_),
                ..
            }
        )));
        let settled = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: peers_ok(room.clone()),
        });
        let view = emitted_view(&settled).expect("paged baseline must converge");
        assert_eq!(
            view.timeline
                .iter()
                .map(|event| event.pos)
                .collect::<Vec<_>>(),
            vec![3]
        );
    }

    #[test]
    fn conflicting_live_event_at_a_retained_position_forces_a_gap() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        let _ = complete_bootstrap(&mut core, &room, vec![evt(1, "original")]);
        let conflict = core.step(Input::Event(ClientEvent::Push(RoomPush::Event {
            room_id: room,
            event: evt(1, "different"),
        })));
        assert!(matches!(
            resync_reason(&conflict),
            Some(ResyncReason::Gap { .. })
        ));
        assert!(!has_emit_view(&conflict));
    }

    #[test]
    fn live_recent_id_at_a_new_position_forces_a_gap() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        let _ = complete_bootstrap(&mut core, &room, vec![evt(1, "e1")]);
        let actions = core.step(Input::Event(ClientEvent::Push(RoomPush::Event {
            room_id: room.clone(),
            event: evt(2, "e1"),
        })));
        assert_eq!(issue_read_count(&actions), 1);
        assert!(matches!(
            resync_reason(&actions),
            Some(ResyncReason::Gap { .. })
        ));
    }

    #[test]
    fn rendered_timeline_is_a_bounded_window_over_the_watermark() {
        let limits = ReconcileLimits {
            timeline_depth: 1,
            ..ReconcileLimits::default()
        };
        let mut core = Core::new(limits);
        let room = rid("r");
        let a = complete_bootstrap(&mut core, &room, vec![evt(1, "e1"), evt(2, "e2")]);
        let view = emitted_view(&a).expect("bootstrap must emit a view");
        assert_eq!(view.timeline.len(), 1);
        assert_eq!(view.timeline[0].pos, 2);

        let a = core.step(Input::Event(ClientEvent::Push(RoomPush::Event {
            room_id: room.clone(),
            event: evt(3, "e3"),
        })));
        let view = emitted_view(&a).expect("live extension must emit a view");
        assert_eq!(view.timeline.len(), 1);
        assert_eq!(view.timeline[0].pos, 3);
    }

    #[test]
    fn bootstrap_retains_genesis_event_at_position_zero() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        // Anchor one is inclusive, so the replacement must retain both the
        // mandatory genesis event and the first post-genesis event.
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 1,
        });
        let actions = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (read_id, epoch) = read_id_epoch_for(&actions, &room);
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: timeline_ok(room.clone(), vec![genesis_evt(), evt(1, "e1")]),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: members_ok(room.clone()),
        });
        let settled = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: peers_ok(room.clone()),
        });
        let view = emitted_view(&settled).expect("genesis baseline must converge");
        assert_eq!(
            view.timeline
                .iter()
                .map(|event| event.pos)
                .collect::<Vec<_>>(),
            vec![0, 1]
        );
        let extended = core.step(Input::Event(ClientEvent::Push(RoomPush::Event {
            room_id: room.clone(),
            event: evt(2, "e2"),
        })));
        assert_eq!(emitted_view(&extended).unwrap().timeline[2].pos, 2);
    }

    #[test]
    fn first_live_event_extends_an_empty_converged_room() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        complete_bootstrap(&mut core, &room, vec![]);

        let actions = core.step(Input::Event(ClientEvent::Push(RoomPush::Event {
            room_id: room.clone(),
            event: evt(1, "e1"),
        })));
        let view = emitted_view(&actions).expect("first live event must emit a view");
        assert_eq!(
            view.timeline
                .iter()
                .map(|event| event.pos)
                .collect::<Vec<_>>(),
            vec![0, 1]
        );
        assert_eq!(issue_read_count(&actions), 0);
    }

    #[test]
    fn peer_push_while_parked_becomes_observable_presence_loss() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 0,
        });
        let started = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (failed_id, failed_epoch) = read_id_epoch_for(&started, &room);
        core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: failed_id,
            epoch: failed_epoch,
            reply: ReadReply::Timeline(Err(CallError::Timeout)),
        });
        let parked_push = core.step(Input::Event(peer_push(&room, "s", "d", 1)));
        assert!(parked_push.is_empty());

        let relaunched = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (read_id, epoch) = read_id_epoch_for(&relaunched, &room);
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: timeline_ok(room.clone(), vec![]),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: members_ok(room.clone()),
        });
        let recovery = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: peers_ok(room),
        });
        assert!(!has_emit_view(&recovery));
        assert!(matches!(
            resync_reason(&recovery),
            Some(ResyncReason::LocalOverflow { dropped: 1 })
        ));
    }

    #[test]
    fn peer_buffer_overflow_forces_a_presence_refresh() {
        let limits = ReconcileLimits {
            buffer_depth: 1,
            ..ReconcileLimits::default()
        };
        let mut core = Core::new(limits);
        let room = rid("r");
        let _ = complete_bootstrap(&mut core, &room, vec![evt(1, "e1")]);
        let started = core.step(Input::Event(ClientEvent::Gap {
            room_id: room.clone(),
            from_pos: 1,
            to: GapTo::Open,
            reason: GapReason::SubscriptionLapse,
        }));
        let (read_id, epoch) = read_id_epoch_for(&started, &room);
        core.step(Input::Event(peer_push(&room, "s1", "d1", 1)));
        core.step(Input::Event(peer_push(&room, "s2", "d2", 1)));

        let settled = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: resync_ok(room.clone(), vec![], 1),
        });
        let got_reason = resync_reason(&settled);
        assert!(
            matches!(got_reason, Some(ResyncReason::LocalOverflow { .. })),
            "peer loss must be visible as LocalOverflow, got {got_reason:?}"
        );
        let (rerun_id, rerun_epoch) = read_id_epoch(&settled);
        assert!(settled.iter().any(|action| matches!(
            action,
            Action::IssueRead {
                request: ReadRequest::Resync(_),
                ..
            }
        )));
        let members = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: rerun_id,
            epoch: rerun_epoch,
            reply: resync_ok(room.clone(), vec![], 1),
        });
        assert!(
            members.iter().any(|action| matches!(
                action,
                Action::IssueRead {
                    request: ReadRequest::Members(_),
                    ..
                }
            )),
            "peer loss rerun must refresh members"
        );
        let peers = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: rerun_id,
            epoch: rerun_epoch,
            reply: members_ok(room.clone()),
        });
        assert!(
            peers.iter().any(|action| matches!(
                action,
                Action::IssueRead {
                    request: ReadRequest::Peers(_),
                    ..
                }
            )),
            "peer loss rerun must refresh peers"
        );
        let final_actions = core.step(Input::ReadReply {
            room_id: room,
            read_id: rerun_id,
            epoch: rerun_epoch,
            reply: peers_ok(rid("r")),
        });
        assert!(has_emit_view(&final_actions));
    }

    #[test]
    fn peer_push_during_events_only_reconcile_is_applied_after_baseline() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        let _ = complete_bootstrap(&mut core, &room, vec![evt(1, "e1")]);
        let started = core.step(Input::Event(ClientEvent::Gap {
            room_id: room.clone(),
            from_pos: 1,
            to: GapTo::Open,
            reason: GapReason::SubscriptionLapse,
        }));
        let (read_id, epoch) = read_id_epoch_for(&started, &room);
        let push_actions = core.step(Input::Event(peer_push(&room, "s1", "d1", 1)));
        assert!(!has_emit_view(&push_actions));

        let settled = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: resync_ok(room.clone(), vec![], 1),
        });
        let view = emitted_view(&settled).expect("buffered peer push must survive the read");
        assert_eq!(view.peers.len(), 1);
        assert_eq!(view.peers[0].subject_id.as_str(), "s1");
    }

    #[test]
    fn peer_push_capacity_cannot_grow_from_wire_keys() {
        let limits = ReconcileLimits {
            peer_capacity: 1,
            ..ReconcileLimits::default()
        };
        let mut core = Core::new(limits);
        let room = rid("r");
        let _ = complete_bootstrap(&mut core, &room, vec![evt(1, "e1")]);
        let first = core.step(Input::Event(peer_push(&room, "s1", "d1", 1)));
        assert_eq!(emitted_view(&first).map(|view| view.peers.len()), Some(1));
        let second = core.step(Input::Event(peer_push(&room, "s2", "d2", 1)));
        assert_eq!(issue_read_count(&second), 1);
        assert!(
            matches!(
                resync_reason(&second),
                Some(ResyncReason::LocalOverflow { .. })
            ),
            "a wire-supplied peer key beyond the bound must force refresh"
        );
    }

    #[test]
    fn live_membership_event_updates_the_rendered_roster() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        let _ = complete_bootstrap(&mut core, &room, vec![evt(1, "e1")]);

        let alice: MemberRow = serde_json::from_str(
            r#"{"subject_id":"alice","role":"member","standing":"active","joined_at":"1970-01-01T00:00:00Z"}"#,
        )
        .expect("member row");
        // Seed the authoritative member row through a presence-triggering
        // reconnect so the live fold has signed role/join evidence to retain.
        let reconnect = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (read_id, epoch) = read_id_epoch(&reconnect);
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: resync_ok(room.clone(), vec![], 1),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: ReadReply::Members(Ok(RoomMembersOut {
                room_id: room.clone(),
                members: vec![alice.clone()],
            })),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: peers_ok(room.clone()),
        });

        let leave: jeliya_api::Event = serde_json::from_str(
            r#"{"pos":2,"event_id":"leave","at":"1970-01-01T00:00:01Z","author":{"state":"unresolved"},"kind":"member_left","content":{"subject_id":"alice"}}"#,
        )
        .expect("member-left event");
        let actions = core.step(Input::Event(ClientEvent::Push(RoomPush::Event {
            room_id: room.clone(),
            event: leave,
        })));
        let view = emitted_view(&actions).expect("membership push must emit a view");
        assert_eq!(view.members[0].standing, jeliya_api::Standing::Left);

        let join: jeliya_api::Event = serde_json::from_str(
            r#"{"pos":3,"event_id":"join","at":"1970-01-01T00:00:02Z","author":{"state":"unresolved"},"kind":"member_joined","content":{"subject_id":"alice","role":"member"}}"#,
        )
        .expect("member-joined event");
        let actions = core.step(Input::Event(ClientEvent::Push(RoomPush::Event {
            room_id: room.clone(),
            event: join,
        })));
        let view = emitted_view(&actions).expect("join must emit a view");
        assert_eq!(view.members[0].standing, jeliya_api::Standing::Active);
        assert_ne!(view.members[0].joined_at, alice.joined_at);

        let remove: jeliya_api::Event = serde_json::from_str(
            r#"{"pos":4,"event_id":"remove","at":"1970-01-01T00:00:03Z","author":{"state":"unresolved"},"kind":"member_removed","content":{"subject_id":"alice","by":"authority"}}"#,
        )
        .expect("member-removed event");
        let actions = core.step(Input::Event(ClientEvent::Push(RoomPush::Event {
            room_id: room.clone(),
            event: remove,
        })));
        let view = emitted_view(&actions).expect("remove must emit a view");
        assert_eq!(view.members[0].standing, jeliya_api::Standing::Removed);

        let unknown_leave: jeliya_api::Event = serde_json::from_str(
            r#"{"pos":5,"event_id":"unknown-leave","at":"1970-01-01T00:00:04Z","author":{"state":"unresolved"},"kind":"member_left","content":{"subject_id":"missing"}}"#,
        )
        .expect("unknown member-left event");
        let actions = core.step(Input::Event(ClientEvent::Push(RoomPush::Event {
            room_id: room.clone(),
            event: unknown_leave,
        })));
        let (refresh_id, refresh_epoch) = read_id_epoch(&actions);
        let members = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: refresh_id,
            epoch: refresh_epoch,
            reply: resync_ok(room.clone(), vec![], 5),
        });
        assert!(
            members.iter().any(|action| matches!(
                action,
                Action::IssueRead {
                    request: ReadRequest::Members(_),
                    ..
                }
            )),
            "unknown membership target must refresh the authoritative roster"
        );
    }

    #[test]
    fn ac5_peer_snapshot_replaced_not_merged_on_reconnect() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");

        // Bootstrap with one member
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 0,
        });
        let a = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (read_id, epoch) = read_id_epoch(&a);
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: timeline_ok(room.clone(), vec![]),
        });
        // Members reply with Alice
        let alice: MemberRow = serde_json::from_str(
            r#"{"subject_id":"alice","role":"member","standing":"active","joined_at":"1970-01-01T00:00:00Z"}"#,
        )
        .expect("alice MemberRow deserializes");
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: ReadReply::Members(Ok(RoomMembersOut {
                room_id: room.clone(),
                members: vec![alice.clone()],
            })),
        });
        let a = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: peers_ok(room.clone()),
        });
        // First view has Alice
        let view = emitted_view(&a).expect("bootstrap must emit the member view");
        assert_eq!(view.members.len(), 1);
        assert_eq!(view.members[0].subject_id.as_str(), "alice");

        // Reconnect — this time the authoritative members reply omits Alice (she left)
        let a = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (read_id2, epoch2) = read_id_epoch(&a);
        // Reconnect is Incremental + presence → resync + members + peers
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: read_id2,
            epoch: epoch2,
            reply: resync_ok(room.clone(), vec![], 0),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: read_id2,
            epoch: epoch2,
            reply: ReadReply::Members(Ok(RoomMembersOut {
                room_id: room.clone(),
                members: vec![], // Alice removed
            })),
        });
        let a = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: read_id2,
            epoch: epoch2,
            reply: peers_ok(room.clone()),
        });
        let view = emitted_view(&a).expect("reconnect must emit the replacement view");
        assert!(
            view.members.is_empty(),
            "removed member must not survive a wholesale replacement"
        );
    }

    // -----------------------------------------------------------------------
    // AC-6 — DirectClient resume uses the same outcome without fabricating a reconnect
    // -----------------------------------------------------------------------

    #[test]
    fn ac6_resume_emits_resume_reason_not_reconnect() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        complete_bootstrap(&mut core, &room, vec![]);

        let actions = core.step(Input::Resume);
        // Must emit ResyncRequired with Resume reason
        assert!(
            matches!(resync_reason(&actions), Some(ResyncReason::Resume)),
            "Input::Resume must emit Resume reason, not Reconnect"
        );
        // Must NOT synthesize a StateChanged
        let has_state_changed = actions.iter().any(|a| {
            matches!(
                a,
                Action::EmitResyncRequired {
                    reason: ResyncReason::Reconnect,
                    ..
                }
            )
        });
        assert!(
            !has_state_changed,
            "resume must not emit Reconnect reason (no fabricated reconnect)"
        );
    }

    #[test]
    fn ac6_resume_produces_incremental_read_for_every_active_room() {
        let mut core = Core::new(ReconcileLimits::default());
        let r1 = rid("r1");
        let r2 = rid("r2");
        complete_bootstrap(&mut core, &r1, vec![evt(1, "e1")]);
        complete_bootstrap(&mut core, &r2, vec![evt(1, "e1")]);

        let actions = core.step(Input::Resume);
        let resync_rooms: Vec<_> = actions
            .iter()
            .filter_map(|a| match a {
                Action::EmitResyncRequired {
                    room_id,
                    reason: ResyncReason::Resume,
                    ..
                } => Some(room_id.clone()),
                _ => None,
            })
            .collect();
        assert!(resync_rooms.contains(&r1), "resume must relaunch r1");
        assert!(resync_rooms.contains(&r2), "resume must relaunch r2");
    }

    // -----------------------------------------------------------------------
    // Verification: stale generation — DropStale, newer state untouched
    // -----------------------------------------------------------------------

    #[test]
    fn stale_read_reply_old_epoch_is_dropped_not_applied() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 0,
        });
        // First Ready → epoch=1, read_id=0
        let a = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (old_read_id, old_epoch) = read_id_epoch(&a);

        // Reconnect before settling → epoch bumps to 2, new read issued
        let a = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (new_read_id, new_epoch) = read_id_epoch(&a);
        assert!(new_epoch > old_epoch, "epoch must have bumped");
        assert_ne!(new_read_id, old_read_id);

        // Stale reply (old epoch) arrives
        let stale_actions = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: old_read_id,
            epoch: old_epoch,
            reply: timeline_ok(room.clone(), vec![evt(1, "stale")]),
        });
        assert!(
            has_drop_stale(&stale_actions),
            "stale reply must be dropped"
        );
        assert!(
            !has_emit_view(&stale_actions),
            "stale reply must not emit a view"
        );
    }

    #[test]
    fn stale_read_reply_wrong_read_id_is_dropped() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 0,
        });
        let a = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (read_id, epoch) = read_id_epoch(&a);

        // Reply with a wrong read_id (off by one)
        let stale_actions = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: read_id.wrapping_add(99),
            epoch,
            reply: timeline_ok(room.clone(), vec![]),
        });
        assert!(
            has_drop_stale(&stale_actions),
            "wrong read_id must be dropped"
        );
        assert!(!has_emit_view(&stale_actions));
    }

    // -----------------------------------------------------------------------
    // Verification: reconnect during open
    // -----------------------------------------------------------------------

    #[test]
    fn reconnect_during_converged_room_launches_incremental_resync() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        complete_bootstrap(&mut core, &room, vec![evt(1, "e1")]);

        // Reconnect
        let a = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        assert!(
            matches!(resync_reason(&a), Some(ResyncReason::Reconnect)),
            "reconnect reason must be Reconnect, not Bootstrap"
        );
        // Must issue an IssueRead (the incremental resync)
        assert_eq!(issue_read_count(&a), 1);
    }

    #[test]
    fn coalesced_flap_is_treated_as_reconnect() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        complete_bootstrap(&mut core, &room, vec![]);

        // A merged flap: Ready → Interrupted → Ready collapsed into one event
        let a = core.step(Input::Event(ClientEvent::StateChanged {
            from: State::Ready,
            to: State::Ready,
            coalesced_through_problem: true,
        }));
        assert!(
            matches!(resync_reason(&a), Some(ResyncReason::Reconnect)),
            "a coalesced flap must trigger a Reconnect, not Bootstrap"
        );
    }

    // -----------------------------------------------------------------------
    // Verification: push during bootstrap is buffered, not applied
    // -----------------------------------------------------------------------

    #[test]
    fn push_during_bootstrap_is_buffered_and_not_applied_immediately() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 0,
        });
        let bootstrap = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (read_id, epoch) = read_id_epoch(&bootstrap);

        // Push arrives while bootstrap is outstanding — must NOT emit a view
        let push_actions = core.step(Input::Event(ClientEvent::Push(RoomPush::Event {
            room_id: room.clone(),
            event: evt(1, "e1"),
        })));
        assert!(
            !has_emit_view(&push_actions),
            "push during bootstrap must be buffered, not emitted"
        );
        assert_eq!(
            issue_read_count(&push_actions),
            0,
            "push during bootstrap must not issue a new read"
        );

        let a = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: timeline_ok(room.clone(), vec![]),
        });
        assert_eq!(issue_read_count(&a), 1, "members read follows the timeline");
        let a = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: members_ok(room.clone()),
        });
        assert_eq!(issue_read_count(&a), 1, "peers read follows the members");
        let a = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: peers_ok(room.clone()),
        });
        let view = emitted_view(&a).expect("buffered push must appear at convergence");
        assert_eq!(
            view.timeline
                .iter()
                .map(|event| event.pos)
                .collect::<Vec<_>>(),
            vec![0, 1]
        );
        assert_eq!(view.timeline[1].event_id.as_str(), "e1");
    }

    // -----------------------------------------------------------------------
    // Verification: cancellation — read dropped, state cleared
    // -----------------------------------------------------------------------

    #[test]
    fn deactivate_room_cancels_outstanding_read() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 0,
        });
        core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });

        let cancel_actions = core.step(Input::DeactivateRoom {
            room_id: room.clone(),
        });
        assert!(
            has_cancel_read(&cancel_actions),
            "deactivate must cancel the outstanding read"
        );
        assert_eq!(
            core.tracked_rooms(),
            0,
            "room must be forgotten after deactivate"
        );
    }

    #[test]
    fn cancel_input_clears_room_and_outstanding_read() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 0,
        });
        core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });

        let cancel_actions = core.step(Input::Cancel {
            room_id: room.clone(),
        });
        assert!(
            has_cancel_read(&cancel_actions),
            "Cancel must cancel the outstanding read"
        );
        assert_eq!(core.tracked_rooms(), 0);
    }

    // -----------------------------------------------------------------------
    // Verification: stop — all reads cancelled, state empty, idempotent
    // -----------------------------------------------------------------------

    #[test]
    fn stop_cancels_all_outstanding_reads_and_clears_rooms() {
        let mut core = Core::new(ReconcileLimits::default());
        let r1 = rid("r1");
        let r2 = rid("r2");
        core.step(Input::ActivateRoom {
            room_id: r1.clone(),
            from_pos: 0,
        });
        core.step(Input::ActivateRoom {
            room_id: r2.clone(),
            from_pos: 0,
        });
        core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });

        let stop_actions = core.step(Input::Stop);
        let cancel_count = stop_actions
            .iter()
            .filter(|a| matches!(a, Action::CancelRead { .. }))
            .count();
        assert_eq!(cancel_count, 2, "stop must cancel every outstanding read");
        assert_eq!(core.tracked_rooms(), 0, "stop must clear all tracked rooms");
    }

    #[test]
    fn stop_is_idempotent() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 0,
        });
        core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        core.step(Input::Stop);
        // Second stop must produce no actions
        let a = core.step(Input::Stop);
        assert!(a.is_empty(), "second Stop must be a no-op");
    }

    #[test]
    fn inputs_after_stop_are_inert() {
        let mut core = Core::new(ReconcileLimits::default());
        core.step(Input::Stop);
        // ActivateRoom after stop must produce nothing
        let a = core.step(Input::ActivateRoom {
            room_id: rid("r"),
            from_pos: 0,
        });
        assert!(a.is_empty(), "ActivateRoom after Stop must be a no-op");
        assert_eq!(core.tracked_rooms(), 0);
    }

    // -----------------------------------------------------------------------
    // Verification: decode failure triggers gap, not silent drop
    // -----------------------------------------------------------------------

    #[test]
    fn decode_failed_triggers_gap_resync_not_silent_drop() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        complete_bootstrap(&mut core, &room, vec![evt(1, "e1")]);

        let a = core.step(Input::DecodeFailed {
            room_id: room.clone(),
        });
        // Must trigger a reconciliation (detected gap, not silent drop)
        assert!(
            matches!(resync_reason(&a), Some(ResyncReason::Gap { .. })),
            "DecodeFailed must trigger a Gap reconciliation, not a silent drop"
        );
        assert_eq!(issue_read_count(&a), 1, "must issue a read for the gap");
    }

    // -----------------------------------------------------------------------
    // Verification: max_active_rooms limit
    // -----------------------------------------------------------------------

    #[test]
    fn scoped_decode_failure_reconciles_only_the_named_room() {
        let mut core = Core::new(ReconcileLimits::default());
        let r1 = rid("r1");
        let r2 = rid("r2");
        complete_bootstrap(&mut core, &r1, vec![evt(1, "e1")]);
        complete_bootstrap(&mut core, &r2, vec![evt(1, "e1")]);

        let actions = core.step(Input::DecodeFailed {
            room_id: r1.clone(),
        });
        let rooms: Vec<_> = actions
            .iter()
            .filter_map(|action| match action {
                Action::EmitResyncRequired {
                    room_id,
                    reason: ResyncReason::Gap { .. },
                    ..
                } => Some(room_id.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(rooms, vec![r1]);
        assert!(!rooms.contains(&r2));
    }

    #[test]
    fn activate_beyond_max_rooms_is_refused() {
        let limits = ReconcileLimits {
            max_active_rooms: 1,
            ..ReconcileLimits::default()
        };
        let mut core = Core::new(limits);
        core.step(Input::ActivateRoom {
            room_id: rid("r1"),
            from_pos: 0,
        });
        core.step(Input::ActivateRoom {
            room_id: rid("r2"),
            from_pos: 0,
        });
        // Only one room must be tracked
        assert_eq!(
            core.tracked_rooms(),
            1,
            "activation beyond max_active_rooms must be refused"
        );
    }

    // -----------------------------------------------------------------------
    // Verification: resync_required daemon reply restarts the reconciliation
    // -----------------------------------------------------------------------

    #[test]
    fn misrouted_timeline_reply_is_rejected_before_convergence() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 0,
        });
        let actions = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (read_id, epoch) = read_id_epoch_for(&actions, &room);
        let rejected = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: timeline_ok(rid("other"), vec![]),
        });
        assert!(matches!(
            resync_reason(&rejected),
            Some(ResyncReason::Gap { .. })
        ));
        assert!(!has_emit_view(&rejected));
    }

    #[test]
    fn duplicate_event_ids_across_baseline_pages_force_recovery() {
        let limits = ReconcileLimits {
            timeline_depth: 2,
            dedup_window: 1,
            read_page_size: 1,
            ..ReconcileLimits::default()
        };
        let mut core = Core::new(limits);
        let room = rid("r");
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 2,
        });
        let actions = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (read_id, epoch) = read_id_epoch_for(&actions, &room);
        let first = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: ReadReply::Timeline(Ok(RoomTimelineOut {
                room_id: room.clone(),
                events: vec![genesis_evt()],
                truncated: Truncated::More {
                    cursor: Cursor::At { pos: 1 },
                },
            })),
        });
        assert_eq!(issue_read_count(&first), 1);
        let second = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: ReadReply::Timeline(Ok(RoomTimelineOut {
                room_id: room.clone(),
                events: vec![evt(1, "same")],
                truncated: Truncated::More {
                    cursor: Cursor::At { pos: 2 },
                },
            })),
        });
        assert_eq!(issue_read_count(&second), 1);
        let rejected = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: ReadReply::Timeline(Ok(RoomTimelineOut {
                room_id: room,
                events: vec![evt(2, "same")],
                truncated: Truncated::Complete,
            })),
        });
        assert!(matches!(
            resync_reason(&rejected),
            Some(ResyncReason::Gap { .. })
        ));
        assert!(!has_emit_view(&rejected));
    }

    #[test]
    fn distant_duplicate_id_across_baseline_pages_fails_closed() {
        let mut core = Core::new(ReconcileLimits {
            timeline_depth: 2,
            dedup_window: 1,
            read_page_size: 2,
            ..ReconcileLimits::default()
        });
        let room = rid("r");
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 4,
        });
        let started = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (read_id, epoch) = read_id_epoch_for(&started, &room);
        let first = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: ReadReply::Timeline(Ok(RoomTimelineOut {
                room_id: room.clone(),
                events: vec![genesis_evt(), evt(1, "duplicate")],
                truncated: Truncated::More {
                    cursor: Cursor::At { pos: 2 },
                },
            })),
        });
        assert_eq!(issue_read_count(&first), 1);
        let second = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: ReadReply::Timeline(Ok(RoomTimelineOut {
                room_id: room.clone(),
                events: vec![evt(2, "b"), evt(3, "c")],
                truncated: Truncated::More {
                    cursor: Cursor::At { pos: 4 },
                },
            })),
        });
        assert_eq!(issue_read_count(&second), 1);
        let rejected = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: ReadReply::Timeline(Ok(RoomTimelineOut {
                room_id: room,
                events: vec![evt(4, "duplicate")],
                truncated: Truncated::Complete,
            })),
        });
        assert!(matches!(
            resync_reason(&rejected),
            Some(ResyncReason::Gap { .. })
        ));
        assert!(!has_emit_view(&rejected));
    }

    #[test]
    fn durable_id_ceiling_retry_preserves_buffered_peer_and_accrued_cause() {
        let mut core = Core::new(ReconcileLimits {
            max_baseline_events: 2,
            ..ReconcileLimits::default()
        });
        let room = rid("r");
        complete_bootstrap(&mut core, &room, vec![evt(1, "e1")]);
        let started = core.step(Input::Event(ClientEvent::Gap {
            room_id: room.clone(),
            from_pos: 1,
            to: GapTo::Open,
            reason: GapReason::Backpressure,
        }));
        let (read_id, epoch) = read_id_epoch_for(&started, &room);
        core.step(Input::Event(peer_push(&room, "s", "d", 1)));
        core.step(Input::Event(ClientEvent::ResyncRequired {
            room_id: room.clone(),
            from_pos: 1,
        }));

        let retry = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: resync_ok(room.clone(), vec![evt(2, "e2")], 2),
        });
        assert!(!has_emit_view(&retry));
        assert!(
            matches!(
                resync_reason(&retry),
                Some(ResyncReason::ResyncRequiredByDaemon { from_pos: 1 })
            ),
            "unexpected retry cause: {:?}",
            resync_reason(&retry)
        );
        let (retry_id, retry_epoch) = read_id_epoch_for(&retry, &room);

        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: retry_id,
            epoch: retry_epoch,
            reply: resync_ok(room.clone(), vec![], 1),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: retry_id,
            epoch: retry_epoch,
            reply: members_ok(room.clone()),
        });
        let peer_recovery = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: retry_id,
            epoch: retry_epoch,
            reply: peers_ok(room),
        });
        assert!(!has_emit_view(&peer_recovery));
        assert!(matches!(
            resync_reason(&peer_recovery),
            Some(ResyncReason::LocalOverflow { dropped }) if *dropped >= 1
        ));
    }

    #[test]
    fn total_baseline_scan_ceiling_fails_closed_without_probabilistic_ids() {
        let mut core = Core::new(ReconcileLimits {
            read_page_size: 2,
            max_baseline_events: 2,
            ..ReconcileLimits::default()
        });
        let room = rid("r");
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 2,
        });
        let started = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (read_id, epoch) = read_id_epoch_for(&started, &room);
        let first = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: ReadReply::Timeline(Ok(RoomTimelineOut {
                room_id: room.clone(),
                events: vec![genesis_evt(), evt(1, "e1")],
                truncated: Truncated::More {
                    cursor: Cursor::At { pos: 2 },
                },
            })),
        });
        assert_eq!(issue_read_count(&first), 1);
        let rejected = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: ReadReply::Timeline(Ok(RoomTimelineOut {
                room_id: room,
                events: vec![evt(2, "e2")],
                truncated: Truncated::Complete,
            })),
        });
        assert!(matches!(
            resync_reason(&rejected),
            Some(ResyncReason::Gap { .. })
        ));
        assert!(!has_emit_view(&rejected));
    }

    #[test]
    fn inconsistent_resync_next_pos_is_not_converged_as_authority() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        let _ = complete_bootstrap(&mut core, &room, vec![evt(1, "e1")]);
        let started = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (read_id, epoch) = read_id_epoch(&started);
        let settled = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: resync_ok(room.clone(), vec![evt(2, "e2")], 999),
        });
        assert_eq!(issue_read_count(&settled), 1);
        assert!(matches!(
            resync_reason(&settled),
            Some(ResyncReason::Gap { .. })
        ));
        assert!(!has_emit_view(&settled));
    }

    #[test]
    fn daemon_resync_required_reply_restarts_from_named_pos() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        complete_bootstrap(&mut core, &room, vec![evt(1, "e1"), evt(2, "e2")]);

        // Reconnect → Incremental resync in flight
        let a = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (read_id, epoch) = read_id_epoch(&a);

        // Daemon answers resync_required from pos=1
        let a = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: ReadReply::Resync(Err(resync_required_err(room.clone(), 1))),
        });
        // Must relaunch with ResyncRequiredByDaemon from pos=1
        assert!(
            matches!(
                resync_reason(&a),
                Some(ResyncReason::ResyncRequiredByDaemon { from_pos: 1 })
            ),
            "daemon resync_required must relaunch with ResyncRequiredByDaemon reason"
        );
        assert_eq!(
            issue_read_count(&a),
            1,
            "must issue a new read from the named pos"
        );
    }

    // -----------------------------------------------------------------------
    // Verification: failed read parks room in NeedsReconcile (no auto-spin)
    // -----------------------------------------------------------------------

    #[test]
    fn repeated_structural_failure_parks_even_with_a_coalesced_trigger() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 1,
        });
        let started = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (first_id, first_epoch) = read_id_epoch_for(&started, &room);
        let malformed = || {
            ReadReply::Timeline(Ok(RoomTimelineOut {
                room_id: room.clone(),
                events: vec![genesis_evt(), evt(2, "e2")],
                truncated: Truncated::Complete,
            }))
        };
        let retry = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: first_id,
            epoch: first_epoch,
            reply: malformed(),
        });
        assert_eq!(
            issue_read_count(&retry),
            1,
            "one structural retry is allowed"
        );
        let (second_id, second_epoch) = read_id_epoch_for(&retry, &room);

        core.step(Input::Event(ClientEvent::Gap {
            room_id: room.clone(),
            from_pos: 0,
            to: GapTo::Open,
            reason: GapReason::SubscriptionLapse,
        }));
        let parked = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: second_id,
            epoch: second_epoch,
            reply: malformed(),
        });
        assert_eq!(
            issue_read_count(&parked),
            0,
            "a coalesced trigger must not bypass the structural retry budget"
        );
        assert!(!has_emit_view(&parked));

        let later_liveness = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        assert_eq!(issue_read_count(&later_liveness), 1);
    }

    #[test]
    fn buffered_event_survives_a_parked_read_failure() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 0,
        });
        let started = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (read_id, epoch) = read_id_epoch_for(&started, &room);
        core.step(Input::Event(ClientEvent::Push(RoomPush::Event {
            room_id: room.clone(),
            event: evt(1, "e1"),
        })));
        let failed = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: ReadReply::Timeline(Err(timeout_err())),
        });
        assert_eq!(issue_read_count(&failed), 0);

        let relaunched = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (retry_id, retry_epoch) = read_id_epoch_for(&relaunched, &room);
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: retry_id,
            epoch: retry_epoch,
            reply: timeline_ok(room.clone(), vec![]),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: retry_id,
            epoch: retry_epoch,
            reply: members_ok(room.clone()),
        });
        let settled = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: retry_id,
            epoch: retry_epoch,
            reply: peers_ok(room),
        });
        let view = emitted_view(&settled).expect("parked buffered evidence must converge");
        assert_eq!(
            view.timeline
                .iter()
                .map(|event| event.pos)
                .collect::<Vec<_>>(),
            vec![0, 1]
        );
    }

    #[test]
    fn failed_buffer_overflow_is_counted_exactly_once() {
        let mut core = Core::new(ReconcileLimits {
            buffer_depth: 1,
            ..ReconcileLimits::default()
        });
        let room = rid("r");
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 0,
        });
        let started = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (failed_id, failed_epoch) = read_id_epoch_for(&started, &room);
        core.step(Input::Event(ClientEvent::Push(RoomPush::Event {
            room_id: room.clone(),
            event: evt(1, "held"),
        })));
        core.step(Input::Event(ClientEvent::Push(RoomPush::Event {
            room_id: room.clone(),
            event: evt(2, "lost"),
        })));
        let failed = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: failed_id,
            epoch: failed_epoch,
            reply: ReadReply::Timeline(Err(CallError::Timeout)),
        });
        assert_eq!(issue_read_count(&failed), 0);

        let relaunched = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (read_id, epoch) = read_id_epoch_for(&relaunched, &room);
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: timeline_ok(room.clone(), vec![]),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: members_ok(room.clone()),
        });
        let recovery = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: peers_ok(room),
        });
        assert!(matches!(
            resync_reason(&recovery),
            Some(ResyncReason::LocalOverflow { dropped: 1 })
        ));
    }

    #[test]
    fn coalesced_daemon_cause_cannot_erase_quantitative_local_loss() {
        let mut core = Core::new(ReconcileLimits {
            buffer_depth: 1,
            ..ReconcileLimits::default()
        });
        let room = rid("r");
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 0,
        });
        let started = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (read_id, epoch) = read_id_epoch_for(&started, &room);
        core.step(Input::Event(ClientEvent::Push(RoomPush::Event {
            room_id: room.clone(),
            event: evt(1, "held"),
        })));
        core.step(Input::Event(ClientEvent::Push(RoomPush::Event {
            room_id: room.clone(),
            event: evt(2, "lost"),
        })));
        core.step(Input::Event(ClientEvent::ResyncRequired {
            room_id: room.clone(),
            from_pos: 0,
        }));
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: timeline_ok(room.clone(), vec![]),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: members_ok(room.clone()),
        });
        let daemon = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: peers_ok(room.clone()),
        });
        assert!(matches!(
            resync_reason(&daemon),
            Some(ResyncReason::ResyncRequiredByDaemon { from_pos: 0 })
        ));
        assert!(!has_emit_view(&daemon));
        let (daemon_id, daemon_epoch) = read_id_epoch_for(&daemon, &room);
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: daemon_id,
            epoch: daemon_epoch,
            reply: resync_ok(room.clone(), vec![evt(1, "held"), evt(2, "lost")], 2),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: daemon_id,
            epoch: daemon_epoch,
            reply: members_ok(room.clone()),
        });
        let local_loss = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: daemon_id,
            epoch: daemon_epoch,
            reply: peers_ok(room),
        });
        assert!(has_emit_view(&local_loss));
        assert!(local_loss
            .iter()
            .any(|action| matches!(action, Action::EmitLagged { dropped: 1, .. })));
        assert_eq!(issue_read_count(&local_loss), 0);
    }

    #[test]
    fn superseding_cause_cannot_erase_inflight_buffer_loss() {
        let mut core = Core::new(ReconcileLimits {
            buffer_depth: 1,
            ..ReconcileLimits::default()
        });
        let room = rid("r");
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 0,
        });
        let started = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        core.step(Input::Event(ClientEvent::Push(RoomPush::Event {
            room_id: room.clone(),
            event: evt(1, "held"),
        })));
        core.step(Input::Event(ClientEvent::Push(RoomPush::Event {
            room_id: room.clone(),
            event: evt(2, "lost"),
        })));

        let superseded = core.step(Input::Resume);
        assert!(has_cancel_read(&superseded));
        assert!(matches!(
            resync_reason(&superseded),
            Some(ResyncReason::Resume)
        ));
        let (read_id, epoch) = read_id_epoch_for(&superseded, &room);
        // The old read is fenced even if it later settles.
        let (old_id, old_epoch) = read_id_epoch_for(&started, &room);
        assert!(has_drop_stale(&core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: old_id,
            epoch: old_epoch,
            reply: timeline_ok(room.clone(), vec![]),
        })));

        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: timeline_ok(room.clone(), vec![]),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: members_ok(room.clone()),
        });
        let recovery = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: peers_ok(room),
        });
        assert!(!has_emit_view(&recovery));
        assert!(matches!(
            resync_reason(&recovery),
            Some(ResyncReason::LocalOverflow { dropped: 1 })
        ));
        assert_eq!(issue_read_count(&recovery), 1);
    }

    #[test]
    fn parked_buffer_overflow_is_recovered_before_publication() {
        let limits = ReconcileLimits {
            buffer_depth: 1,
            ..ReconcileLimits::default()
        };
        let mut core = Core::new(limits);
        let room = rid("r");
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 0,
        });
        let started = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (read_id, epoch) = read_id_epoch_for(&started, &room);
        core.step(Input::Event(ClientEvent::Push(RoomPush::Event {
            room_id: room.clone(),
            event: evt(1, "e1"),
        })));
        core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: ReadReply::Timeline(Err(timeout_err())),
        });
        // The failed read's e1 occupies the parked buffer; e2 must be counted
        // as loss rather than silently replacing or disappearing.
        core.step(Input::Event(ClientEvent::Push(RoomPush::Event {
            room_id: room.clone(),
            event: evt(2, "e2"),
        })));

        let relaunched = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (retry_id, retry_epoch) = read_id_epoch_for(&relaunched, &room);
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: retry_id,
            epoch: retry_epoch,
            reply: timeline_ok(room.clone(), vec![]),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: retry_id,
            epoch: retry_epoch,
            reply: members_ok(room.clone()),
        });
        let recovery = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: retry_id,
            epoch: retry_epoch,
            reply: peers_ok(room.clone()),
        });
        assert!(
            !has_emit_view(&recovery),
            "a view with known parked loss must not be published"
        );
        assert!(matches!(
            resync_reason(&recovery),
            Some(ResyncReason::LocalOverflow { dropped: 1 })
        ));

        let (overflow_id, overflow_epoch) = read_id_epoch_for(&recovery, &room);
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: overflow_id,
            epoch: overflow_epoch,
            reply: resync_ok(room.clone(), vec![evt(2, "e2")], 2),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: overflow_id,
            epoch: overflow_epoch,
            reply: members_ok(room.clone()),
        });
        let settled = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: overflow_id,
            epoch: overflow_epoch,
            reply: peers_ok(room),
        });
        let view = emitted_view(&settled).expect("overflow recovery must converge");
        assert_eq!(
            view.timeline
                .iter()
                .map(|event| event.pos)
                .collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
    }

    #[test]
    fn failed_read_parks_room_awaiting_liveness_trigger() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 0,
        });
        let a = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (read_id, epoch) = read_id_epoch(&a);

        // Timeline read times out
        let a = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: ReadReply::Timeline(Err(timeout_err())),
        });
        // No immediate relaunch — the room waits for the next liveness trigger
        assert_eq!(
            issue_read_count(&a),
            0,
            "failed read must not auto-spin; wait for liveness"
        );
        assert!(!has_emit_view(&a));

        // Next Ready relaunches
        let a = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        assert_eq!(
            issue_read_count(&a),
            1,
            "liveness trigger after failure must relaunch"
        );
    }

    /// A coalesced `resync_required` carries the daemon's discard position, and
    /// **no watermark encodes it**. If a failed settle drops the pending re-run,
    /// `truncate_to` never runs: the repudiated positions stay in the timeline
    /// forever while the room keeps applying pushes and broadcasting `Converged`,
    /// so the consumer observes a permanently wrong timeline and no error.
    #[test]
    fn coalesced_lower_cursor_suppresses_the_repudiated_view() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        let _ = complete_bootstrap(
            &mut core,
            &room,
            vec![evt(1, "a"), evt(2, "b"), evt(3, "c")],
        );
        let started = core.step(Input::Event(ClientEvent::Gap {
            room_id: room.clone(),
            from_pos: 3,
            to: GapTo::Open,
            reason: GapReason::Backpressure,
        }));
        let (read_id, epoch) = read_id_epoch_for(&started, &room);
        core.step(Input::Event(ClientEvent::ResyncRequired {
            room_id: room.clone(),
            from_pos: 1,
        }));
        let rerun = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: resync_ok(room, vec![], 3),
        });
        assert!(!has_emit_view(&rerun));
        assert_eq!(issued_resync_from(&rerun), Some(1));
        assert!(matches!(
            resync_reason(&rerun),
            Some(ResyncReason::ResyncRequiredByDaemon { from_pos: 1 })
        ));
    }

    #[test]
    fn a_coalesced_resync_required_survives_a_failed_read() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        let _ = complete_bootstrap(
            &mut core,
            &room,
            vec![evt(1, "a"), evt(2, "b"), evt(3, "c")],
        );

        // A gap launches an incremental resync from the watermark.
        let a = core.step(Input::Event(ClientEvent::Gap {
            room_id: room.clone(),
            from_pos: 3,
            to: GapTo::Open,
            reason: GapReason::Backpressure,
        }));
        let (read_id, epoch) = read_id_epoch_for(&a, &room);

        // While it is outstanding the daemon repudiates back to pos 1.
        let a = core.step(Input::Event(ClientEvent::ResyncRequired {
            room_id: room.clone(),
            from_pos: 1,
        }));
        assert_eq!(
            issue_read_count(&a),
            0,
            "must coalesce into the in-flight read, not launch a second"
        );

        // The outstanding read fails on a still-live connection (a timeout
        // settles with no lifecycle transition), so nothing else will relaunch.
        let a = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: ReadReply::Resync(Err(timeout_err())),
        });

        assert_eq!(
            issue_read_count(&a),
            1,
            "the coalesced resync_required must relaunch on the failed settle"
        );
        assert!(
            matches!(
                resync_reason(&a),
                Some(ResyncReason::ResyncRequiredByDaemon { from_pos: 1 })
            ),
            "the daemon's discard position must survive the failed settle, got {:?}",
            resync_reason(&a)
        );
    }

    /// A `gap` names the position it starts *after*. Positions this room already
    /// applied above that are inside the discontinuity; resyncing from the room's
    /// own (higher) watermark would silently retain a repudiated suffix.
    #[test]
    fn a_gap_re_reads_from_the_repudiated_position_not_the_watermark() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        let _ = complete_bootstrap(
            &mut core,
            &room,
            vec![evt(1, "a"), evt(2, "b"), evt(3, "c")],
        );

        let a = core.step(Input::Event(ClientEvent::Gap {
            room_id: room.clone(),
            from_pos: 1,
            to: GapTo::Open,
            reason: GapReason::Backpressure,
        }));

        assert_eq!(
            issued_resync_from(&a),
            Some(1),
            "must re-read from the gap's from_pos, not the watermark (3)"
        );
    }

    /// A superseding launch replaces the whole reconciliation. Any trigger that
    /// coalesced into it but has not run yet must be folded into the new cause —
    /// a coalesced `resync_required` is the only carrier of its discard position.
    #[test]
    fn a_superseding_launch_keeps_the_coalesced_cause() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        let _ = complete_bootstrap(
            &mut core,
            &room,
            vec![evt(1, "a"), evt(2, "b"), evt(3, "c")],
        );

        // A gap starts a reconciliation from the watermark.
        let _ = core.step(Input::Event(ClientEvent::Gap {
            room_id: room.clone(),
            from_pos: 3,
            to: GapTo::Open,
            reason: GapReason::Backpressure,
        }));
        // The daemon repudiates back to pos 1; it coalesces into the in-flight run.
        let a = core.step(Input::Event(ClientEvent::ResyncRequired {
            room_id: room.clone(),
            from_pos: 1,
        }));
        assert_eq!(issue_read_count(&a), 0, "coalesced, not launched");

        // A reconnect supersedes the in-flight reconciliation.
        let a = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: true,
        });

        assert_eq!(
            issued_resync_from(&a),
            Some(1),
            "the superseding launch must still honour the daemon's discard position"
        );
    }

    /// §R8: an authoritative read is the truth. Once it removes a device, a
    /// delayed frame from an *older* connection must not bring the peer back —
    /// which is exactly what dropping the per-device fences on snapshot
    /// replacement allowed.
    #[test]
    fn first_peer_snapshot_fences_a_delayed_generation_one_push() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 1,
        });
        let started = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (read_id, epoch) = read_id_epoch_for(&started, &room);
        // Generation one was observed before the first snapshot and is omitted
        // by that authority. A later copy of the same frame is stale.
        core.step(Input::Event(peer_push(&room, "s1", "d1", 1)));
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: timeline_ok(room.clone(), vec![evt(1, "a")]),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: members_ok(room.clone()),
        });
        let recovery = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: peers_ok(room.clone()),
        });
        let (recovery_id, recovery_epoch) = read_id_epoch_for(&recovery, &room);
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: recovery_id,
            epoch: recovery_epoch,
            reply: resync_ok(room.clone(), vec![], 1),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: recovery_id,
            epoch: recovery_epoch,
            reply: members_ok(room.clone()),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: recovery_id,
            epoch: recovery_epoch,
            reply: peers_ok(room.clone()),
        });

        let delayed = core.step(Input::Event(peer_push(&room, "s1", "d1", 1)));
        assert!(
            !has_emit_view(&delayed),
            "a generation-one frame older than the first authoritative snapshot must not resurrect a device"
        );
    }

    #[test]
    fn a_stale_peer_push_cannot_resurrect_a_device_an_authoritative_read_removed() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        let _ = complete_bootstrap(&mut core, &room, vec![evt(1, "a")]);

        // A live push introduces device d1 at generation 5.
        let a = core.step(Input::Event(peer_push(&room, "s1", "d1", 5)));
        assert_eq!(
            emitted_view(&a).map(|v| v.peers.len()),
            Some(1),
            "the live push introduces the device"
        );

        // A same-transport Resume re-reads presence authoritatively, and d1 is gone.
        let a = core.step(Input::Resume);
        let (read_id, epoch) = read_id_epoch_for(&a, &room);
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: resync_ok(room.clone(), vec![], 1),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: members_ok(room.clone()),
        });
        let a = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: peers_ok_with(room.clone(), Reachability::Alone, vec![]),
        });
        assert_eq!(
            emitted_view(&a).map(|v| v.peers.len()),
            Some(0),
            "the authoritative read removed the device"
        );

        // A delayed frame from the OLDER connection (generation 4) arrives.
        let a = core.step(Input::Event(peer_push(&room, "s1", "d1", 4)));
        assert!(
            !has_emit_view(&a),
            "a stale-generation frame must be discarded, not resurrect the peer"
        );
    }

    #[test]
    fn removed_high_generation_device_does_not_fence_a_different_device() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        let _ = complete_bootstrap(&mut core, &room, vec![evt(1, "a")]);
        assert!(has_emit_view(
            &core.step(Input::Event(peer_push(&room, "s-a", "d-a", 100,)))
        ));
        let started = core.step(Input::Resume);
        let (read_id, epoch) = read_id_epoch_for(&started, &room);
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: resync_ok(room.clone(), vec![], 1),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: members_ok(room.clone()),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: peers_ok(room.clone()),
        });
        let new_device = core.step(Input::Event(peer_push(&room, "s-b", "d-b", 1)));
        let view = emitted_view(&new_device).expect("generation fences are per device");
        assert!(view
            .peers
            .iter()
            .any(|peer| peer.device_id.as_str() == "d-b"));
    }

    #[test]
    fn reconnect_epoch_rebuilds_peer_payload_generation_fences() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        let _ = complete_bootstrap(&mut core, &room, vec![evt(1, "a")]);
        let _ = core.step(Input::Event(peer_push(&room, "s-a", "d-a", 100)));
        let _ = core.step(Input::Event(ClientEvent::Gap {
            room_id: room.clone(),
            from_pos: 1,
            to: GapTo::Open,
            reason: GapReason::Backpressure,
        }));
        let _ = core.step(Input::Event(ClientEvent::ResyncRequired {
            room_id: room.clone(),
            from_pos: 1,
        }));
        core.step(Input::Lifecycle {
            to: State::Interrupted,
            coalesced_through_problem: false,
        });
        let started = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        assert!(matches!(
            resync_reason(&started),
            Some(ResyncReason::ResyncRequiredByDaemon { from_pos: 1 })
        ));
        let (read_id, epoch) = read_id_epoch_for(&started, &room);
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: resync_ok(room.clone(), vec![], 1),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: members_ok(room.clone()),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: peers_ok_with(
                room.clone(),
                Reachability::Connected,
                vec![peer_row("s-a", "d-a")],
            ),
        });
        let fresh = core.step(Input::Event(peer_push(&room, "s-a", "d-a", 1)));
        assert!(
            has_emit_view(&fresh),
            "a new transport epoch may restart a peer payload generation"
        );
    }

    #[test]
    fn equal_generation_for_an_omitted_peer_forces_authoritative_refresh() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        let _ = complete_bootstrap(&mut core, &room, vec![evt(1, "a")]);
        let _ = core.step(Input::Event(peer_push(&room, "s-a", "d-a", 5)));
        let started = core.step(Input::Resume);
        let (read_id, epoch) = read_id_epoch_for(&started, &room);
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: resync_ok(room.clone(), vec![], 1),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: members_ok(room.clone()),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: peers_ok(room.clone()),
        });
        let ambiguous = core.step(Input::Event(peer_push(&room, "s-a", "d-a", 5)));
        assert!(!has_emit_view(&ambiguous));
        assert_eq!(issue_read_count(&ambiguous), 1);
        assert!(matches!(
            resync_reason(&ambiguous),
            Some(ResyncReason::LocalOverflow { .. })
        ));
    }

    #[test]
    fn same_connection_generation_allows_later_peer_link_change() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        let _ = complete_bootstrap(&mut core, &room, vec![evt(1, "a")]);
        let first = core.step(Input::Event(peer_push(&room, "s1", "d1", 2)));
        assert!(has_emit_view(&first));
        let second = core.step(Input::Event(ClientEvent::Push(RoomPush::Peer {
            room_id: room,
            subject_id: SubjectId::new("s1"),
            device_id: DeviceId::new("d1"),
            link: Link::NotConnected {
                reason: jeliya_api::LinkReason::Closed,
            },
            generation: 2,
        })));
        let view = emitted_view(&second).expect("later same-generation link change applies");
        assert!(matches!(
            view.peers.as_slice(),
            [PeerRow {
                link: Link::NotConnected { .. },
                ..
            }]
        ));
    }

    /// The whole-room aggregate is derived from the per-device links, so it has
    /// to move with them: a freshly linked peer must not still read `Alone`.
    #[test]
    fn reachability_follows_the_links_a_peer_push_changes() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");

        // Converge with a live-but-peerless room.
        let a = core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 0,
        });
        let bootstrap = if a.iter().any(|x| matches!(x, Action::IssueRead { .. })) {
            a
        } else {
            core.step(Input::Lifecycle {
                to: State::Ready,
                coalesced_through_problem: false,
            })
        };
        let (read_id, epoch) = read_id_epoch_for(&bootstrap, &room);
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: timeline_ok(room.clone(), vec![evt(1, "a")]),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: members_ok(room.clone()),
        });
        let a = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: peers_ok_with(room.clone(), Reachability::Alone, vec![]),
        });
        assert_eq!(
            emitted_view(&a).map(|v| v.reachability),
            Some(Reachability::Alone)
        );

        // A peer links up: the aggregate must become Connected.
        let a = core.step(Input::Event(peer_push(&room, "s1", "d1", 1)));
        assert_eq!(
            emitted_view(&a).map(|v| v.reachability),
            Some(Reachability::Connected),
            "a linked peer must move the aggregate off Alone"
        );
    }

    /// `Connecting`/`Offline` are transport-level answers the daemon owns; a
    /// peer push must not fabricate liveness the client does not have.
    #[test]
    fn a_peer_push_does_not_fabricate_liveness_while_offline() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        // `peers_ok` reports Offline.
        let _ = complete_bootstrap(&mut core, &room, vec![evt(1, "a")]);

        let a = core.step(Input::Event(peer_push(&room, "s1", "d1", 1)));
        assert_eq!(
            emitted_view(&a).map(|v| v.reachability),
            Some(Reachability::Offline),
            "an offline room must stay offline"
        );
    }

    // -----------------------------------------------------------------------
    // Subscribe-to-activation handoff: an event committed after the anchor but
    // before the room's activation was admitted may already have been pushed
    // (and dropped, the room being untracked). The bootstrap read is the only
    // recovery, so it must not discard the validated post-anchor suffix.
    // -----------------------------------------------------------------------

    #[test]
    fn bootstrap_retains_events_committed_beyond_the_activation_anchor() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        // The anchor (1) is the head at `stream.subscribe` time; position 2
        // was committed — and its push lost — before activation was admitted.
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 1,
        });
        let started = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (read_id, epoch) = read_id_epoch_for(&started, &room);
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: timeline_ok(room.clone(), vec![evt(1, "e1"), evt(2, "raced")]),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: members_ok(room.clone()),
        });
        let settled = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: peers_ok(room.clone()),
        });
        let view = emitted_view(&settled).expect("the bootstrap converges");
        assert_eq!(
            view.timeline.last().map(|event| event.pos),
            Some(2),
            "an event committed past the anchor before activation must not vanish"
        );
        assert_eq!(
            view.timeline.last().map(|event| event.event_id.as_str()),
            Some("raced")
        );

        // The subscription later replays the same event as a live push (it was
        // queued from the anchor): an exact identity/position replay is
        // ignored, never a spurious gap.
        let replay = core.step(Input::Event(ClientEvent::Push(RoomPush::Event {
            room_id: room.clone(),
            event: evt(2, "raced"),
        })));
        assert_eq!(issue_read_count(&replay), 0, "a replay must not resync");
        assert!(!has_emit_view(&replay), "a replay changes nothing");
    }

    #[test]
    fn bootstrap_pages_to_complete_past_the_anchor() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 2,
        });
        let started = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (read_id, epoch) = read_id_epoch_for(&started, &room);
        // The anchor (2) is reached mid-page, but the daemon reports more
        // committed history: the baseline keeps paging to Complete instead of
        // silently discarding it.
        let continued = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: ReadReply::Timeline(Ok(RoomTimelineOut {
                room_id: room.clone(),
                events: vec![genesis_evt(), evt(1, "e1"), evt(2, "e2")],
                truncated: Truncated::More {
                    cursor: Cursor::At { pos: 3 },
                },
            })),
        });
        assert!(
            continued.iter().any(|action| matches!(
                action,
                Action::IssueRead {
                    request: ReadRequest::Timeline(_),
                    ..
                }
            )),
            "a continuation past the anchor is followed, not abandoned"
        );
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: ReadReply::Timeline(Ok(RoomTimelineOut {
                room_id: room.clone(),
                events: vec![evt(3, "e3")],
                truncated: Truncated::Complete,
            })),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: members_ok(room.clone()),
        });
        let settled = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: peers_ok(room.clone()),
        });
        let view = emitted_view(&settled).expect("the paged bootstrap converges");
        assert_eq!(view.timeline.last().map(|event| event.pos), Some(3));
    }

    // -----------------------------------------------------------------------
    // Required frontier: a trigger naming a position above the watermark is
    // evidence the committed history extends at least that far. Authority must
    // reach it before any view is published; a persistently short daemon parks.
    // -----------------------------------------------------------------------

    /// A daemon cursor above the watermark is clamped for the read, but the
    /// named position must not be forgotten: an empty authoritative suffix
    /// below it cannot publish as converged.
    #[test]
    fn daemon_cursor_above_watermark_cannot_converge_below_the_known_frontier() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        complete_bootstrap(&mut core, &room, vec![evt(1, "e1")]);

        // The daemon names position 5; the client holds only up to 1.
        let started = core.step(Input::Event(ClientEvent::ResyncRequired {
            room_id: room.clone(),
            from_pos: 5,
        }));
        assert_eq!(
            issued_resync_from(&started),
            Some(1),
            "the read is clamped to the held prefix"
        );
        let (read_id, epoch) = read_id_epoch_for(&started, &room);
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: resync_ok(room.clone(), vec![], 1),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: members_ok(room.clone()),
        });
        let settled = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: peers_ok(room.clone()),
        });
        assert!(
            !has_emit_view(&settled),
            "an empty suffix below the daemon-named frontier must not publish"
        );
        assert_eq!(
            issue_read_count(&settled),
            1,
            "one bounded follow-up read chases the frontier"
        );

        // The follow-up also comes back empty: park, never publish below the
        // frontier.
        let (retry_id, retry_epoch) = read_id_epoch_for(&settled, &room);
        let parked = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: retry_id,
            epoch: retry_epoch,
            reply: resync_ok(room.clone(), vec![], 1),
        });
        assert!(!has_emit_view(&parked), "fail closed, never publish short");
        assert_eq!(issue_read_count(&parked), 0, "the frontier retry is spent");

        // A later liveness trigger relaunches; once authority actually serves
        // through the frontier the room converges and publishes.
        let relaunched = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (live_id, live_epoch) = read_id_epoch_for(&relaunched, &room);
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: live_id,
            epoch: live_epoch,
            reply: resync_ok(
                room.clone(),
                vec![evt(2, "e2"), evt(3, "e3"), evt(4, "e4"), evt(5, "e5")],
                5,
            ),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: live_id,
            epoch: live_epoch,
            reply: members_ok(room.clone()),
        });
        let converged = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: live_id,
            epoch: live_epoch,
            reply: peers_ok(room.clone()),
        });
        let view = emitted_view(&converged).expect("reaching the frontier converges");
        assert_eq!(view.timeline.last().map(|event| event.pos), Some(5));
    }

    /// With no watermark yet, a coalesced gap's cursor cannot steer the read,
    /// but its named frontier (including a bounded upper end) still forbids an
    /// old-anchor baseline from converging below it.
    #[test]
    fn bounded_gap_frontier_survives_a_genesis_bootstrap() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        core.step(Input::ActivateRoom {
            room_id: room.clone(),
            from_pos: 0,
        });
        let started = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (read_id, epoch) = read_id_epoch_for(&started, &room);

        // While the bootstrap is in flight, a gap proves history through 3.
        core.step(Input::Event(ClientEvent::Gap {
            room_id: room.clone(),
            from_pos: 2,
            to: GapTo::Bounded { pos: 3 },
            reason: GapReason::Retention,
        }));

        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: timeline_ok(room.clone(), vec![]),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: members_ok(room.clone()),
        });
        // The coalesced gap is a publication barrier; the rerun relaunches.
        let rerun = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: peers_ok(room.clone()),
        });
        assert!(!has_emit_view(&rerun));
        let (rerun_id, rerun_epoch) = read_id_epoch_for(&rerun, &room);

        // The rerun's authoritative suffix is empty: still below the bounded
        // frontier 3, so nothing may publish.
        let short = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: rerun_id,
            epoch: rerun_epoch,
            reply: resync_ok(room.clone(), vec![], 0),
        });
        assert!(
            !has_emit_view(&short),
            "a genesis-only baseline below a bounded gap frontier must not publish"
        );
        assert_eq!(issue_read_count(&short), 1, "one bounded frontier chase");

        let (retry_id, retry_epoch) = read_id_epoch_for(&short, &room);
        let parked = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: retry_id,
            epoch: retry_epoch,
            reply: resync_ok(room.clone(), vec![], 0),
        });
        assert!(!has_emit_view(&parked));
        assert_eq!(issue_read_count(&parked), 0, "park after the spent retry");

        // Liveness recovers once authority serves through the frontier.
        let relaunched = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (live_id, live_epoch) = read_id_epoch_for(&relaunched, &room);
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: live_id,
            epoch: live_epoch,
            reply: resync_ok(
                room.clone(),
                vec![evt(1, "e1"), evt(2, "e2"), evt(3, "e3")],
                3,
            ),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: live_id,
            epoch: live_epoch,
            reply: members_ok(room.clone()),
        });
        let converged = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: live_id,
            epoch: live_epoch,
            reply: peers_ok(room.clone()),
        });
        let view = emitted_view(&converged).expect("reaching the frontier converges");
        assert_eq!(view.timeline.last().map(|event| event.pos), Some(3));
    }

    // -----------------------------------------------------------------------
    // Bounded `resync_required` redirect chains: repeated non-progressing
    // cursors must not issue reads forever.
    // -----------------------------------------------------------------------

    /// A `resync_required` whose effective cursor equals the read it answers
    /// makes no progress. One retry is allowed; a second identical redirect
    /// parks the room instead of hot-looping against the daemon.
    #[test]
    fn non_progressing_resync_required_parks_after_one_retry() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        complete_bootstrap(&mut core, &room, vec![evt(1, "e1")]);

        // Reconnect → incremental resync from the watermark (1) in flight.
        let reconnect = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (read_id, epoch) = read_id_epoch_for(&reconnect, &room);

        // The daemon redirects to the exact cursor we just read from.
        let first = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: ReadReply::Resync(Err(resync_required_err(room.clone(), 1))),
        });
        assert_eq!(issue_read_count(&first), 1, "one non-progress retry runs");
        let (retry_id, retry_epoch) = read_id_epoch_for(&first, &room);

        // The identical redirect again: no progress is provable — park.
        let parked = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: retry_id,
            epoch: retry_epoch,
            reply: ReadReply::Resync(Err(resync_required_err(room.clone(), 1))),
        });
        assert_eq!(
            issue_read_count(&parked),
            0,
            "a non-progressing redirect chain must park, not loop"
        );
        assert!(!has_emit_view(&parked));

        // A later liveness trigger still relaunches the parked room.
        let relaunched = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        assert_eq!(issue_read_count(&relaunched), 1);
    }

    /// Even strictly-decreasing daemon redirects are a bounded chain: a daemon
    /// that keeps naming lower cursors cannot make the client issue reads
    /// without limit before a converge.
    #[test]
    fn descending_resync_required_chain_is_bounded() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        let history: Vec<_> = (1..=12).map(|pos| evt(pos, &format!("e{pos}"))).collect();
        complete_bootstrap(&mut core, &room, history);

        let reconnect = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (mut read_id, mut epoch) = read_id_epoch_for(&reconnect, &room);

        // Eight strictly-progressing redirects are served (11 down to 4).
        for from_pos in (4..=11).rev() {
            let redirected = core.step(Input::ReadReply {
                room_id: room.clone(),
                read_id,
                epoch,
                reply: ReadReply::Resync(Err(resync_required_err(room.clone(), from_pos))),
            });
            assert_eq!(
                issue_read_count(&redirected),
                1,
                "a progressing redirect to {from_pos} relaunches"
            );
            (read_id, epoch) = read_id_epoch_for(&redirected, &room);
        }

        // The ninth redirect exceeds the chain bound: park.
        let parked = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: ReadReply::Resync(Err(resync_required_err(room.clone(), 3))),
        });
        assert_eq!(
            issue_read_count(&parked),
            0,
            "an unbounded descending redirect chain must park"
        );
        assert!(!has_emit_view(&parked));
    }

    // -----------------------------------------------------------------------
    // Rollback below the retained render window: the recovered view must not
    // pretend the room is empty when history merely fell out of the window.
    // -----------------------------------------------------------------------

    /// A daemon discard below the oldest retained event, answered by an empty
    /// authoritative suffix, must rebuild the render window with a full
    /// timeline replacement instead of publishing an empty "authoritative"
    /// view for a room that has history.
    #[test]
    fn empty_suffix_after_rollback_below_window_rebuilds_the_timeline() {
        let limits = ReconcileLimits {
            timeline_depth: 2,
            ..ReconcileLimits::default()
        };
        let mut core = Core::new(limits);
        let room = rid("r");
        // Window retains [3, 4]; positions 0..=2 were evicted by the depth cap.
        complete_bootstrap(
            &mut core,
            &room,
            vec![evt(1, "e1"), evt(2, "e2"), evt(3, "e3"), evt(4, "e4")],
        );

        let started = core.step(Input::Event(ClientEvent::ResyncRequired {
            room_id: room.clone(),
            from_pos: 1,
        }));
        let (read_id, epoch) = read_id_epoch_for(&started, &room);
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: resync_ok(room.clone(), vec![], 1),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: members_ok(room.clone()),
        });
        let settled = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: peers_ok(room.clone()),
        });
        assert!(
            emitted_view(&settled).is_none(),
            "an empty rendered window over surviving history must not publish"
        );
        let replacement_issued = settled.iter().any(|action| {
            matches!(
                action,
                Action::IssueRead {
                    request: ReadRequest::Timeline(_),
                    ..
                }
            )
        });
        assert!(
            replacement_issued,
            "the window is rebuilt by a full timeline replacement"
        );

        // The replacement serves the daemon's post-discard history through the
        // subscription anchor; the rebuilt window renders its newest tail.
        let (rebuild_id, rebuild_epoch) = read_id_epoch_for(&settled, &room);
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: rebuild_id,
            epoch: rebuild_epoch,
            reply: timeline_ok(
                room.clone(),
                vec![evt(1, "e1"), evt(2, "r2"), evt(3, "r3"), evt(4, "r4")],
            ),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: rebuild_id,
            epoch: rebuild_epoch,
            reply: members_ok(room.clone()),
        });
        let rebuilt = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: rebuild_id,
            epoch: rebuild_epoch,
            reply: peers_ok(room.clone()),
        });
        let view = emitted_view(&rebuilt).expect("the replacement converges");
        assert_eq!(
            view.timeline
                .iter()
                .map(|event| event.pos)
                .collect::<Vec<_>>(),
            vec![3, 4],
            "the rebuilt window renders the newest authoritative tail"
        );
        assert_eq!(view.timeline[0].event_id.as_str(), "r3");
    }

    // -----------------------------------------------------------------------
    // Saturated tombstone retention is deterministic: which removed peers stay
    // fenced when the bounded tombstone map fills must not depend on hash-map
    // iteration order. (Before the ordered fencing this test failed on most
    // runs, depending on the per-process hasher seed.)
    // -----------------------------------------------------------------------

    #[test]
    fn saturated_peer_tombstone_retention_is_deterministic() {
        let limits = ReconcileLimits {
            peer_capacity: 4,
            ..ReconcileLimits::default()
        };
        let mut core = Core::new(limits);
        let room = rid("r");
        complete_bootstrap(&mut core, &room, vec![evt(1, "e1")]);

        // Each cycle: a resume-triggered snapshot replacement, then pushes
        // that stamp connection generations. Resume never resets the
        // per-device fences (same transport epoch throughout).
        let replace_snapshot = |core: &mut Core, rows: Vec<PeerRow>| {
            let relaunched = core.step(Input::Resume);
            let (read_id, epoch) = read_id_epoch_for(&relaunched, &room);
            let _ = core.step(Input::ReadReply {
                room_id: room.clone(),
                read_id,
                epoch,
                reply: resync_ok(room.clone(), vec![], 1),
            });
            let _ = core.step(Input::ReadReply {
                room_id: room.clone(),
                read_id,
                epoch,
                reply: members_ok(room.clone()),
            });
            let _ = core.step(Input::ReadReply {
                room_id: room.clone(),
                read_id,
                epoch,
                reply: peers_ok_with(room.clone(), Reachability::Connected, rows),
            });
        };

        // Cycle 1: s1/s2 appear and stamp generation 5.
        replace_snapshot(&mut core, vec![peer_row("s1", "d"), peer_row("s2", "d")]);
        core.step(Input::Event(peer_push(&room, "s1", "d", 5)));
        core.step(Input::Event(peer_push(&room, "s2", "d", 5)));
        // Cycle 2: t1..t3 replace them; s1/s2 take two of the four fence slots.
        replace_snapshot(
            &mut core,
            vec![
                peer_row("t1", "d"),
                peer_row("t2", "d"),
                peer_row("t3", "d"),
            ],
        );
        core.step(Input::Event(peer_push(&room, "t1", "d", 5)));
        core.step(Input::Event(peer_push(&room, "t2", "d", 5)));
        core.step(Input::Event(peer_push(&room, "t3", "d", 5)));
        // Cycle 3: everything removed. Two slots remain for three keys: the
        // two lowest (t1, t2) are fenced; t3 overflows into fail-closed mode.
        replace_snapshot(&mut core, vec![]);

        for fenced in ["t1", "t2"] {
            let stale = core.step(Input::Event(peer_push(&room, fenced, "d", 1)));
            assert!(
                stale.is_empty(),
                "a stale generation for deterministically fenced {fenced} is \
                 silently discarded, got {} actions",
                stale.len()
            );
        }
        let unfenced = core.step(Input::Event(peer_push(&room, "t3", "d", 1)));
        assert!(
            matches!(
                resync_reason(&unfenced),
                Some(ResyncReason::LocalOverflow { .. })
            ),
            "the overflowed key fails closed into an authoritative refresh"
        );
    }

    // -----------------------------------------------------------------------
    // Quantitative loss is orthogonal to cause priority: a stronger coalesced
    // cause selects the recovery, but must not erase how many pushes were
    // lost — the count surfaces as an attributed Lagged boundary before the
    // covering converged view.
    // -----------------------------------------------------------------------

    #[test]
    fn external_lagged_count_survives_a_stronger_coalesced_cause() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        complete_bootstrap(&mut core, &room, vec![evt(1, "e1")]);

        // A gap opens a reconciliation.
        let started = core.step(Input::Event(ClientEvent::Gap {
            room_id: room.clone(),
            from_pos: 1,
            to: GapTo::Open,
            reason: GapReason::SubscriptionLapse,
        }));
        let (read_id, epoch) = read_id_epoch_for(&started, &room);

        // Quantitative local loss coalesces while the read is in flight …
        core.step(Input::Event(ClientEvent::Lagged {
            room_id: Some(room.clone()),
            dropped: 7,
        }));
        // … and a stronger daemon cause then wins the pending rerun.
        core.step(Input::Event(ClientEvent::ResyncRequired {
            room_id: room.clone(),
            from_pos: 1,
        }));

        // The in-flight read settles; the coalesced rerun relaunches with the
        // daemon cause (a publication barrier, so nothing publishes yet).
        let rerun = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: resync_ok(room.clone(), vec![], 1),
        });
        assert!(!has_emit_view(&rerun));
        assert!(
            matches!(
                resync_reason(&rerun),
                Some(ResyncReason::ResyncRequiredByDaemon { from_pos: 1 })
            ),
            "the stronger cause selects the recovery"
        );

        // The daemon-caused recovery settles and publishes: the erased-cause
        // loss count must surface as an attributed boundary before the view.
        let (recover_id, recover_epoch) = read_id_epoch_for(&rerun, &room);
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: recover_id,
            epoch: recover_epoch,
            reply: resync_ok(room.clone(), vec![], 1),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: recover_id,
            epoch: recover_epoch,
            reply: members_ok(room.clone()),
        });
        let converged = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: recover_id,
            epoch: recover_epoch,
            reply: peers_ok(room.clone()),
        });
        assert!(
            has_emit_view(&converged),
            "the covering authority publishes"
        );
        assert_eq!(
            emitted_lagged(&converged),
            Some(7),
            "the quantitative loss must not vanish with the outranked cause"
        );
    }

    // -----------------------------------------------------------------------
    // Lost peer-buffer keys stay fenced: a peer push dropped by the bounded
    // buffer must leave a generation tombstone, or a stale same-generation
    // frame can reinsert a phantom peer after an omitting snapshot.
    // -----------------------------------------------------------------------

    #[test]
    fn peer_push_dropped_by_buffer_limits_fences_stale_resurrection() {
        let limits = ReconcileLimits {
            buffer_depth: 1,
            ..ReconcileLimits::default()
        };
        let mut core = Core::new(limits);
        let room = rid("r");
        complete_bootstrap(&mut core, &room, vec![evt(1, "e1")]);

        // Reconnect → incremental read in flight; one buffered event push
        // fills the combined transient budget.
        let reconnect = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (read_id, epoch) = read_id_epoch_for(&reconnect, &room);
        core.step(Input::Event(ClientEvent::Push(RoomPush::Event {
            room_id: room.clone(),
            event: evt(2, "e2"),
        })));
        // The peer push overflows the buffer: its key/generation is dropped.
        core.step(Input::Event(peer_push(&room, "s1", "d1", 5)));

        // The read settles; the authoritative snapshot omits (s1, d1) — the
        // peer disconnected while the client could not observe it.
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: resync_ok(room.clone(), vec![evt(2, "e2")], 2),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: members_ok(room.clone()),
        });
        let rerun = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: peers_ok(room.clone()),
        });
        // The recorded loss forces one presence-refreshing rerun.
        let (rerun_id, rerun_epoch) = read_id_epoch_for(&rerun, &room);
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: rerun_id,
            epoch: rerun_epoch,
            reply: resync_ok(room.clone(), vec![], 2),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: rerun_id,
            epoch: rerun_epoch,
            reply: members_ok(room.clone()),
        });
        let settled = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: rerun_id,
            epoch: rerun_epoch,
            reply: peers_ok(room.clone()),
        });
        let view = emitted_view(&settled).expect("the loss-covering rerun converges");
        assert!(view.peers.is_empty(), "authority removed the peer");

        // A stale frame from the same dropped connection generation replays.
        // The forgotten key must not be treated as a fresh peer: fail closed
        // into an authoritative presence refresh, never a phantom row.
        let stale = core.step(Input::Event(peer_push(&room, "s1", "d1", 5)));
        assert!(
            emitted_view(&stale).is_none_or(|view| view.peers.is_empty()),
            "a stale same-generation frame must not resurrect the dropped peer"
        );

        // A genuinely newer connection generation is still admitted.
        let fresh = core.step(Input::Event(peer_push(&room, "s1", "d1", 6)));
        if let Some(view) = emitted_view(&fresh) {
            assert_eq!(view.peers.len(), 1, "a newer generation reconnects");
        }
    }

    #[test]
    fn peer_push_dropped_while_parked_fences_stale_resurrection() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        complete_bootstrap(&mut core, &room, vec![evt(1, "e1")]);

        // A failed read parks the room.
        let reconnect = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (read_id, epoch) = read_id_epoch_for(&reconnect, &room);
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: ReadReply::Resync(Err(timeout_err())),
        });

        // A peer push while parked is counted as loss; its key must be fenced.
        core.step(Input::Event(peer_push(&room, "s1", "d1", 5)));

        // A resume relaunches the parked room within the SAME transport epoch
        // (unlike a reconnect, it does not reset the per-device fences — a
        // stale frame from the dropped connection can still arrive). The
        // authoritative snapshot omits the peer, and the pre-existing loss
        // forces one covering rerun before publication.
        let relaunched = core.step(Input::Resume);
        let (live_id, live_epoch) = read_id_epoch_for(&relaunched, &room);
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: live_id,
            epoch: live_epoch,
            reply: resync_ok(room.clone(), vec![], 1),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: live_id,
            epoch: live_epoch,
            reply: members_ok(room.clone()),
        });
        let rerun = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: live_id,
            epoch: live_epoch,
            reply: peers_ok(room.clone()),
        });
        let (rerun_id, rerun_epoch) = read_id_epoch_for(&rerun, &room);
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: rerun_id,
            epoch: rerun_epoch,
            reply: resync_ok(room.clone(), vec![], 1),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: rerun_id,
            epoch: rerun_epoch,
            reply: members_ok(room.clone()),
        });
        let settled = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: rerun_id,
            epoch: rerun_epoch,
            reply: peers_ok(room.clone()),
        });
        assert!(
            emitted_view(&settled).is_some_and(|view| view.peers.is_empty()),
            "the covering rerun publishes the authoritative empty presence"
        );

        // The stale same-generation frame must not reinsert the phantom.
        let stale = core.step(Input::Event(peer_push(&room, "s1", "d1", 5)));
        assert!(
            emitted_view(&stale).is_none_or(|view| view.peers.is_empty()),
            "a stale same-generation frame must not resurrect a parked-dropped peer"
        );
    }

    // -----------------------------------------------------------------------
    // Contradictory pushes at one position: the first arbitrary claimant must
    // not survive into the published view. Recovery re-reads from before the
    // disputed position so authority itself settles it.
    // -----------------------------------------------------------------------

    #[test]
    fn contradictory_buffered_pushes_cannot_partially_commit() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        complete_bootstrap(&mut core, &room, vec![evt(1, "e1")]);

        // A gap opens a reconciliation; two contradictory claimants for
        // position 2 arrive while the baseline read is in flight.
        let started = core.step(Input::Event(ClientEvent::Gap {
            room_id: room.clone(),
            from_pos: 1,
            to: GapTo::Open,
            reason: GapReason::SubscriptionLapse,
        }));
        let (read_id, epoch) = read_id_epoch_for(&started, &room);
        core.step(Input::Event(ClientEvent::Push(RoomPush::Event {
            room_id: room.clone(),
            event: evt(2, "first-claimant"),
        })));
        core.step(Input::Event(ClientEvent::Push(RoomPush::Event {
            room_id: room.clone(),
            event: evt(2, "second-claimant"),
        })));

        // The read settles with nothing after 1; the buffered contradiction
        // surfaces during convergence and must not publish either claimant.
        let rerun = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: resync_ok(room.clone(), vec![], 1),
        });
        assert!(!has_emit_view(&rerun), "no claimant may publish unverified");
        assert_eq!(
            issued_resync_from(&rerun),
            Some(1),
            "recovery must re-read from before the disputed position, not after it"
        );

        // Authority answers with the real event at position 2.
        let (recover_id, recover_epoch) = read_id_epoch_for(&rerun, &room);
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: recover_id,
            epoch: recover_epoch,
            reply: resync_ok(room.clone(), vec![evt(2, "authoritative")], 2),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: recover_id,
            epoch: recover_epoch,
            reply: members_ok(room.clone()),
        });
        let converged = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: recover_id,
            epoch: recover_epoch,
            reply: peers_ok(room.clone()),
        });
        let view = emitted_view(&converged).expect("authoritative recovery converges");
        assert_eq!(
            view.timeline.last().map(|event| event.event_id.as_str()),
            Some("authoritative"),
            "only the authoritative claimant survives in the published view"
        );
        assert!(
            view.timeline
                .iter()
                .all(|event| event.event_id.as_str() != "first-claimant"),
            "the arbitrary first claimant must not survive: {:?}",
            view.timeline
                .iter()
                .map(|event| event.event_id.as_str())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn live_conflict_below_watermark_re_reads_the_disputed_position() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        complete_bootstrap(&mut core, &room, vec![evt(1, "e1"), evt(2, "e2")]);

        // A live push contradicts the committed event at position 2.
        let started = core.step(Input::Event(ClientEvent::Push(RoomPush::Event {
            room_id: room.clone(),
            event: evt(2, "imposter"),
        })));
        assert!(!has_emit_view(&started));
        assert_eq!(
            issued_resync_from(&started),
            Some(1),
            "the disputed position itself must be re-verified by authority"
        );

        let (read_id, epoch) = read_id_epoch_for(&started, &room);
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: resync_ok(room.clone(), vec![evt(2, "e2")], 2),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: members_ok(room.clone()),
        });
        let converged = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: peers_ok(room.clone()),
        });
        let view = emitted_view(&converged).expect("authority settles the dispute");
        assert_eq!(
            view.timeline
                .iter()
                .map(|event| event.event_id.as_str())
                .collect::<Vec<_>>(),
            vec!["genesis", "e1", "e2"],
            "the committed event survives; the imposter does not"
        );
    }

    // -----------------------------------------------------------------------
    // Membership must not survive a repudiated suffix: a lowering truncation
    // forces an authoritative roster replacement.
    // -----------------------------------------------------------------------

    #[test]
    fn lowering_truncation_replaces_derived_membership_authoritatively() {
        let mut core = Core::new(ReconcileLimits::default());
        let room = rid("r");
        complete_bootstrap(&mut core, &room, vec![evt(1, "e1")]);

        // Seed the authoritative roster with an active member.
        let alice: MemberRow = serde_json::from_str(
            r#"{"subject_id":"alice","role":"member","standing":"active","joined_at":"1970-01-01T00:00:00Z"}"#,
        )
        .expect("member row");
        let reconnect = core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let (read_id, epoch) = read_id_epoch_for(&reconnect, &room);
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: resync_ok(room.clone(), vec![], 1),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: ReadReply::Members(Ok(RoomMembersOut {
                room_id: room.clone(),
                members: vec![alice],
            })),
        });
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id,
            epoch,
            reply: peers_ok(room.clone()),
        });

        // A live signed leave at position 2 folds into the roster.
        let leave: jeliya_api::Event = serde_json::from_str(
            r#"{"pos":2,"event_id":"leave","at":"1970-01-01T00:00:01Z","author":{"state":"unresolved"},"kind":"member_left","content":{"subject_id":"alice"}}"#,
        )
        .expect("member-left event");
        let folded = core.step(Input::Event(ClientEvent::Push(RoomPush::Event {
            room_id: room.clone(),
            event: leave,
        })));
        assert_eq!(
            emitted_view(&folded).map(|view| view.members[0].standing),
            Some(Standing::Left)
        );

        // The daemon repudiates position 2 and replaces it with a plain
        // message. The derived Left standing is now unsupported evidence.
        let started = core.step(Input::Event(ClientEvent::Gap {
            room_id: room.clone(),
            from_pos: 1,
            to: GapTo::Open,
            reason: GapReason::Backpressure,
        }));
        assert_eq!(issued_resync_from(&started), Some(1));
        let (gap_id, gap_epoch) = read_id_epoch_for(&started, &room);
        let after_suffix = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: gap_id,
            epoch: gap_epoch,
            reply: resync_ok(room.clone(), vec![evt(2, "m2")], 2),
        });
        let members_requested = after_suffix.iter().any(|action| {
            matches!(
                action,
                Action::IssueRead {
                    request: ReadRequest::Members(_),
                    ..
                }
            )
        });
        assert!(
            members_requested,
            "a lowering truncation must force an authoritative roster replacement"
        );
        assert!(
            !has_emit_view(&after_suffix),
            "no view publishes while the derived roster is unsupported"
        );

        // Authority still names alice active; the replaced suffix carried no
        // membership change, so the discarded leave must not survive.
        let alice_active: MemberRow = serde_json::from_str(
            r#"{"subject_id":"alice","role":"member","standing":"active","joined_at":"1970-01-01T00:00:00Z"}"#,
        )
        .expect("member row");
        let _ = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: gap_id,
            epoch: gap_epoch,
            reply: ReadReply::Members(Ok(RoomMembersOut {
                room_id: room.clone(),
                members: vec![alice_active],
            })),
        });
        let converged = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: gap_id,
            epoch: gap_epoch,
            reply: peers_ok(room.clone()),
        });
        let view = emitted_view(&converged).expect("the roster replacement converges");
        assert_eq!(
            view.members[0].standing,
            Standing::Active,
            "the repudiated leave must not survive in the derived roster"
        );
        assert_eq!(view.timeline.last().map(|event| event.pos), Some(2));
        assert_eq!(
            view.timeline.last().map(|event| event.event_id.as_str()),
            Some("m2")
        );
    }
}
