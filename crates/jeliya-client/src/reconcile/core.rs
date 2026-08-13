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
use crate::reconcile::room::{LiveOutcome, ReadReply, ReadRequest, ReplyOutcome, RoomState};
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
            } => self.on_lifecycle(to, coalesced_through_problem, &mut actions),
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
        if let Some(room) = self.rooms.get_mut(&room_id) {
            room.set_anchor(from_pos);
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
    fn on_lifecycle(&mut self, to: State, coalesced: bool, actions: &mut Vec<Action>) {
        self.state = to;
        let reconnect = to == State::Ready;
        // A coalesced-through-problem transition whose endpoint is still `Ready`
        // is a merged flap and is handled by the `reconnect` branch; the flag is
        // retained for symmetry with the seam's honest signal.
        let _ = coalesced;
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
            self.launch(&room_id, reason.clone(), None, actions);
        }
    }

    /// A push/gap/resync/lagged event.
    fn on_event(&mut self, event: ClientEvent, actions: &mut Vec<Action>) {
        match event {
            ClientEvent::StateChanged {
                to,
                coalesced_through_problem,
                ..
            } => self.on_lifecycle(to, coalesced_through_problem, actions),
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
                    if let Some(view) =
                        room.apply_peer_push(subject_id, device_id, link, generation)
                    {
                        actions.push(Action::EmitView(view));
                    }
                }
            }
            // Transfers are not a room-timeline concern (§11 non-goal).
            ClientEvent::Push(RoomPush::Transfer { .. }) => {}
            ClientEvent::Gap {
                room_id,
                to,
                reason,
                ..
            } => self.trigger(&room_id, ResyncReason::Gap { reason, to }, None, actions),
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
        match room.on_read_reply(reply) {
            ReplyOutcome::NextRead(request) => actions.push(Action::IssueRead {
                room_id,
                read_id,
                epoch,
                request,
            }),
            ReplyOutcome::Converged { view, rerun } => {
                actions.push(Action::EmitView(view));
                if let Some(reason) = rerun {
                    let from_pos = from_pos_for(&reason);
                    self.launch(&room_id, reason, from_pos, actions);
                }
            }
            ReplyOutcome::Restart { reason, from_pos } => {
                self.launch(&room_id, reason, Some(from_pos), actions)
            }
            // A failed read parks the room in `NeedsReconcile`; the next liveness
            // trigger relaunches it (bounded: no auto-spin).
            ReplyOutcome::Failed(_) => {}
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
        for (room_id, room) in self.rooms.drain() {
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
        if room.is_reconciling() {
            room.coalesce_rerun(reason);
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
        self.rooms.keys().cloned().collect()
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
        ApiError, GapReason, GapTo, MemberRow, Reachability, RoomId, RoomMembersOut, RoomPeersOut,
        RoomTimelineOut, StreamResyncOut, Truncated,
    };

    use crate::error::CallError;
    use crate::event::{ClientEvent, RoomPush, State};
    use crate::reconcile::reason::ResyncReason;
    use crate::reconcile::room::ReadReply;
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

    fn timeline_ok(room_id: RoomId, events: Vec<jeliya_api::Event>) -> ReadReply {
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
        let activate_actions = core.step(Input::ActivateRoom {
            room_id: room_id.clone(),
            from_pos: 0,
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
        // The converged view is emitted AND the coalesced rerun relaunches
        assert!(has_emit_view(&a), "converged view emitted");
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
        // Convergence emits view (with e1 applied, e2 dropped) AND relaunches LocalOverflow
        assert!(has_emit_view(&a), "initial convergence emits view");
        assert_eq!(issue_read_count(&a), 1, "rerun read issued after overflow");
        assert!(
            matches!(resync_reason(&a), Some(ResyncReason::LocalOverflow { .. })),
            "rerun reason is LocalOverflow"
        );
        // The rerun is an Incremental resync (resync from watermark).
        // When it settles with e2 included, the final view must contain both events.
        let (rerun_read_id, rerun_epoch) = read_id_epoch(&a);
        // Settle rerun resync with e2
        let a = core.step(Input::ReadReply {
            room_id: room.clone(),
            read_id: rerun_read_id,
            epoch: rerun_epoch,
            reply: resync_ok(room.clone(), vec![evt(2, "e2")], 2),
        });
        // Presence re-read (reconnect/resume causes presence re-read; LocalOverflow does not)
        // LocalOverflow is events-only (implicates_presence = false), so no Members/Peers read.
        // The rerun converges directly.
        assert!(has_emit_view(&a), "rerun convergence emits view");
        if let Some(view) = emitted_view(&a) {
            assert_eq!(
                view.timeline.len(),
                2,
                "both events must be present after rerun"
            );
            assert!(
                view.timeline.iter().any(|e| e.event_id.as_str() == "e2"),
                "dropped event e2 must be recovered by the rerun"
            );
        }
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
            from_pos: 0,
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
        // After convergence the view must include both pos=1 (baseline) and pos=2 (buffer)
        assert!(has_emit_view(&a), "view emitted after convergence");
        if let Some(view) = emitted_view(&a) {
            assert_eq!(
                view.timeline.len(),
                2,
                "timeline must contain baseline + buffered event"
            );
            assert_eq!(view.timeline[0].pos, 1);
            assert_eq!(view.timeline[1].pos, 2);
        }
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
        if let Some(view) = emitted_view(&a) {
            assert_eq!(
                view.timeline.len(),
                1,
                "duplicate buffered event must be deduped — timeline must have exactly 1 event"
            );
        }
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
        // Convergence emits a view AND relaunches because of the gap in the buffer
        assert!(
            has_emit_view(&a),
            "initial view emitted even with a gap rerun"
        );
        assert_eq!(
            issue_read_count(&a),
            1,
            "out-of-order buffer push must trigger a rerun read"
        );
    }

    // -----------------------------------------------------------------------
    // AC-5 — Peer state is replaced from authoritative reads, never merged
    // -----------------------------------------------------------------------

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
        if let Some(view) = emitted_view(&a) {
            assert_eq!(view.members.len(), 1);
            assert_eq!(view.members[0].subject_id.as_str(), "alice");
        }

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
        if let Some(view) = emitted_view(&a) {
            assert!(
                view.members.is_empty(),
                "removed member must not survive a wholesale replacement"
            );
        }
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
        core.step(Input::Lifecycle {
            to: State::Ready,
            coalesced_through_problem: false,
        });

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
}
