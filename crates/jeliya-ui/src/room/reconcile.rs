//! Folding the reconciler's [`RoomUpdate`] stream into [`RoomActivityState`]
//! (#179 §9), and the pending↔committed reconciliation that guarantees no
//! duplicate messages across reconnect/resync (§9/§10).
//!
//! The reconciler (#169) is the **only** resync path: its timeline is already
//! gap-free and deduplicated by `event_id`, so #179 adds exactly one new dedup —
//! dropping a `Syncing { event_id }` pending send the instant the converged
//! timeline contains that id. Because the committed row is authoritative and
//! re-delivered under the same id after a reconnect, the pending was already
//! dropped and no duplicate can appear (§10).
//!
//! Pure: this module folds already-received updates; the async subscription and
//! drive live in [`crate::components::activity`] and [`crate::app`].

use jeliya_api::RoomId;
use jeliya_client::RoomUpdate;

use crate::room::RoomActivityState;

impl RoomActivityState {
    /// Fold one [`RoomUpdate`] for `room_id` into this state (§9). Updates for
    /// other rooms (the fan-out delivers every room's updates to every
    /// subscription) are ignored here — the pane filters to its own room.
    ///
    /// - `Resyncing` for this room → set the non-blocking notice, so every
    ///   resync cause is observable (#169 AC-1).
    /// - `Converged` for this room → replace the view (latest wins), clear the
    ///   resync/loss notices, and reconcile pending sends against the committed
    ///   timeline (drop any whose `event_id` now appears).
    /// - `Lagged` attributed to this room (or unattributed) → set an honest
    ///   loss marker, recovered by the next converged view.
    pub fn apply_update(&mut self, room_id: &RoomId, update: &RoomUpdate) {
        match update {
            RoomUpdate::Resyncing { room_id: r, .. } if r == room_id => {
                self.resyncing = true;
            }
            RoomUpdate::Converged(view) if &view.room_id == room_id => {
                self.view = Some(view.clone());
                self.resyncing = false;
                self.lost = None;
                self.sends.reconcile(&view.timeline);
            }
            RoomUpdate::Lagged {
                room_id: r,
                dropped,
            } if r.is_none() || r.as_ref() == Some(room_id) => {
                let total = self.lost.unwrap_or(0).saturating_add(*dropped);
                self.lost = Some(total);
            }
            // An update for another room, or a variant already handled.
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::room::send::SendPhase;
    use jeliya_api::{
        Author, Event, EventId, EventKindContent, MessageSendOut, Reachability, Timestamp,
    };
    use jeliya_client::{ResyncReason, ResyncRequired, RoomView};

    fn ts(secs: i64) -> Timestamp {
        Timestamp::new(time::OffsetDateTime::from_unix_timestamp(secs).expect("valid instant"))
    }

    fn event(event_id: &str, pos: u64) -> Event {
        Event {
            pos,
            event_id: EventId::new(event_id),
            at: ts(pos as i64),
            author: Author::Unresolved,
            kind: EventKindContent::Message { body: "hi".into() },
        }
    }

    fn view(room: &str, generation: u64, timeline: Vec<Event>) -> RoomView {
        RoomView {
            room_id: RoomId::new(room),
            generation,
            timeline,
            members: Vec::new(),
            peers: Vec::new(),
            reachability: Reachability::Connected,
        }
    }

    #[test]
    fn a_converged_view_for_this_room_replaces_state_and_clears_notices() {
        let room = RoomId::new("r");
        let mut state = RoomActivityState::new();
        state.resyncing = true;
        state.lost = Some(4);
        state.apply_update(
            &room,
            &RoomUpdate::Converged(view("r", 1, vec![event("e0", 0)])),
        );
        assert!(!state.resyncing);
        assert_eq!(state.lost, None);
        assert_eq!(state.timeline().len(), 1);
    }

    #[test]
    fn an_update_for_another_room_is_ignored() {
        let room = RoomId::new("r");
        let mut state = RoomActivityState::new();
        state.apply_update(
            &room,
            &RoomUpdate::Converged(view("other", 1, vec![event("x", 0)])),
        );
        assert!(state.view.is_none());
        state.apply_update(
            &room,
            &RoomUpdate::Resyncing {
                room_id: RoomId::new("other"),
                resync: ResyncRequired {
                    generation: 1,
                    reason: ResyncReason::Reconnect,
                },
            },
        );
        assert!(!state.resyncing);
    }

    #[test]
    fn resyncing_sets_the_notice_and_convergence_clears_it() {
        let room = RoomId::new("r");
        let mut state = RoomActivityState::new();
        state.apply_update(
            &room,
            &RoomUpdate::Resyncing {
                room_id: room.clone(),
                resync: ResyncRequired {
                    generation: 2,
                    reason: ResyncReason::Reconnect,
                },
            },
        );
        assert!(state.resyncing);
        state.apply_update(&room, &RoomUpdate::Converged(view("r", 2, Vec::new())));
        assert!(!state.resyncing);
    }

    #[test]
    fn a_lagged_marker_is_recorded_and_recovered_by_the_next_converged_view() {
        let room = RoomId::new("r");
        let mut state = RoomActivityState::new();
        state.apply_update(
            &room,
            &RoomUpdate::Lagged {
                room_id: Some(room.clone()),
                dropped: 3,
            },
        );
        assert_eq!(state.lost, Some(3));
        // An unattributed loss also counts for this room.
        state.apply_update(
            &room,
            &RoomUpdate::Lagged {
                room_id: None,
                dropped: 2,
            },
        );
        assert_eq!(state.lost, Some(5));
        state.apply_update(&room, &RoomUpdate::Converged(view("r", 3, Vec::new())));
        assert_eq!(state.lost, None, "the converged view recovers the loss");
    }

    #[test]
    fn convergence_drops_a_pending_send_whose_committed_row_arrived() {
        let room = RoomId::new("r");
        let mut state = RoomActivityState::new();
        let id = state.sends.begin("hello".into(), ts(5));
        state.sends.set_phase(
            id,
            crate::room::send::phase_for_reply(&MessageSendOut {
                room_id: room.clone(),
                event_id: EventId::new("committed"),
                pos: 9,
                at: ts(6),
            }),
        );
        // A converge WITHOUT the committed row keeps the pending (still syncing).
        state.apply_update(
            &room,
            &RoomUpdate::Converged(view("r", 1, vec![event("other", 0)])),
        );
        assert_eq!(state.sends.entries().len(), 1);
        assert!(matches!(
            state.sends.get(id).unwrap().phase,
            SendPhase::Syncing { .. }
        ));
        // The committed row converges: the pending is dropped, one row remains.
        state.apply_update(
            &room,
            &RoomUpdate::Converged(view("r", 2, vec![event("committed", 1)])),
        );
        assert!(
            state.sends.is_empty(),
            "no duplicate: pending drops on its committed row"
        );
    }
}
