//! The pure, renderer-agnostic core of the room Activity destination (#179 §3).
//!
//! Everything in `room/` is host-testable and free of Dioxus, `web-sys`, and
//! `cfg` (Decision-3): it holds the *decisions* the Activity pane renders — the
//! exhaustive event projection ([`projection`]), agent-status run folding and
//! the activity filters ([`runs`]), the evidence-backed send state machine
//! ([`send`]), the pure scroll math ([`scroll`]), and the fold of the
//! reconciler's [`RoomUpdate`](jeliya_client::RoomUpdate) stream into a per-room
//! view plus pending-send reconciliation ([`reconcile`]).
//!
//! The desktop (#184) and Android (#193) targets reuse this core unchanged
//! behind their own composition; only the thin components in
//! [`crate::components`] own DOM measurement, through Dioxus's renderer-agnostic
//! mounted element API (D6).

pub mod projection;
pub mod reconcile;
pub mod runs;
pub mod scroll;
pub mod send;

use std::collections::HashSet;

use jeliya_api::{Event, EventId, Timestamp};
use jeliya_client::RoomView;

use crate::room::send::{op_id_for, SendEntry, SendId, SendIds, SendPhase};

/// The per-room model the Activity pane renders: the reconciler's authoritative
/// view plus the honest reconcile notices and the local pending sends.
///
/// It is derived state — a fold of the reconciler's `RoomUpdate` fan-out (§9,
/// [`reconcile`]) — never a second copy of daemon truth. The signed timeline in
/// `view` is the only history authority (D1); `sends` are the local, not-yet-
/// reconciled evidence layered on top.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct RoomActivityState {
    /// The latest converged authoritative view, or `None` before the first
    /// convergence (booting is *unknown*, not *zero* — D7).
    pub view: Option<RoomView>,
    /// Whether a reconciliation is in flight (a non-blocking notice; every
    /// resync cause is observable — #169 AC-1).
    pub resyncing: bool,
    /// An honest local-loss marker (updates were dropped in a bounded buffer or
    /// this subscription mailbox), recovered by the next converged view.
    pub lost: Option<u64>,
    /// The local sends not yet reconciled against a committed row.
    pub sends: PendingSends,
}

impl RoomActivityState {
    /// A fresh state before the first convergence.
    pub fn new() -> Self {
        Self::default()
    }

    /// The signed timeline of the latest converged view, or an empty slice when
    /// no view has converged yet.
    pub fn timeline(&self) -> &[Event] {
        self.view
            .as_ref()
            .map(|v| v.timeline.as_slice())
            .unwrap_or(&[])
    }

    /// The newest signed author-dated instant in the converged timeline, for
    /// advancing the device-local last-seen mark (§9). `None` when the room has
    /// no events — an absent last-event implies no unread (never a fabricated
    /// zero-time). Live activity only moves this forward.
    pub fn newest_signed_at(&self) -> Option<Timestamp> {
        self.timeline().iter().map(|event| event.at).max()
    }
}

/// The local, per-room-session set of pending sends and their id source (§6).
///
/// A send is added [`SendPhase::Pending`], advanced to `Syncing` on the reply
/// (or on a `DecodeReply` `Definitely` failure) or `Failed` on error, and
/// dropped the instant the converged timeline surfaces its `event_id`. Retry
/// reuses the same stable `op_id` (D4). Bounded by the active-room set (§11): a
/// room's pending map is only as large as its unresolved sends.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct PendingSends {
    entries: Vec<SendEntry>,
    ids: SendIds,
}

impl PendingSends {
    /// An empty pending set.
    pub fn new() -> Self {
        Self::default()
    }

    /// The pending entries, in insertion order.
    pub fn entries(&self) -> &[SendEntry] {
        &self.entries
    }

    /// Whether there are no pending entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// A pending entry by its local id.
    pub fn get(&self, client_id: SendId) -> Option<&SendEntry> {
        self.entries.iter().find(|e| e.client_id == client_id)
    }

    /// Begin a send: mint a fresh local id and its stable op_id, push a
    /// `Pending` entry, and return the id (to address later transitions). The
    /// caller dispatches `message.send` with `Dedup::Key(op_id_for(id))`.
    pub fn begin(&mut self, body: String, at: Timestamp) -> SendId {
        let client_id = self.ids.mint();
        self.entries.push(SendEntry::new(client_id, body, at));
        client_id
    }

    /// Set an entry's phase (the reply/error transitions live in [`send`]).
    pub fn set_phase(&mut self, client_id: SendId, phase: SendPhase) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.client_id == client_id) {
            entry.phase = phase;
        }
    }

    /// Move a failed entry back to `Pending` for a retry, returning the body and
    /// the **same** op_id to re-issue under (D4). `None` if the entry is gone or
    /// is not in a retryable `Failed` phase.
    pub fn retry(&mut self, client_id: SendId) -> Option<(String, jeliya_api::OpId)> {
        let entry = self.entries.iter_mut().find(|e| e.client_id == client_id)?;
        if !matches!(entry.phase, SendPhase::Failed { .. }) {
            return None;
        }
        entry.phase = SendPhase::Pending;
        Some((entry.body.clone(), op_id_for(entry.client_id)))
    }

    /// Drop an entry entirely (e.g. the user dismissed a failed send).
    pub fn remove(&mut self, client_id: SendId) {
        self.entries.retain(|e| e.client_id != client_id);
    }

    /// Reconcile against a converged timeline: drop every `Syncing` entry whose
    /// `event_id` now appears in the timeline (§9/§10). Entries with an unknown
    /// `event_id` (a `DecodeReply` `Definitely`) reconcile by absence — they are
    /// not dropped here (they linger until superseded), matching the honest
    /// boundary in §10.
    pub fn reconcile(&mut self, timeline: &[Event]) {
        let ids: HashSet<&EventId> = timeline.iter().map(|e| &e.event_id).collect();
        self.entries
            .retain(|entry| !entry.reconciled_by(&|id: &EventId| ids.contains(id)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::room::send::{op_id_for, SendPhase};
    use jeliya_api::{Author, EventId, EventKindContent, MessageSendOut, OpId, RoomId};

    fn ts(secs: i64) -> Timestamp {
        Timestamp::new(time::OffsetDateTime::from_unix_timestamp(secs).expect("valid instant"))
    }

    fn committed(event_id: &str) -> Event {
        Event {
            pos: 1,
            event_id: EventId::new(event_id),
            at: ts(10),
            author: Author::Unresolved,
            kind: EventKindContent::Message { body: "hi".into() },
        }
    }

    #[test]
    fn begin_mints_a_pending_entry_with_a_stable_op_id() {
        let mut sends = PendingSends::new();
        let id = sends.begin("hello".into(), ts(1));
        assert_eq!(id, SendId(0));
        let entry = sends.get(id).expect("entry present");
        assert_eq!(entry.phase, SendPhase::Pending);
        assert_eq!(entry.op_id, op_id_for(id));
        assert_eq!(entry.body, "hello");
    }

    #[test]
    fn a_reply_then_reconcile_drops_the_pending_entry() {
        let mut sends = PendingSends::new();
        let id = sends.begin("hello".into(), ts(1));
        sends.set_phase(
            id,
            crate::room::send::phase_for_reply(&MessageSendOut {
                room_id: RoomId::new("r"),
                event_id: EventId::new("event-1"),
                pos: 3,
                at: ts(2),
            }),
        );
        // A converged timeline WITHOUT the id keeps the entry.
        sends.reconcile(&[committed("other")]);
        assert_eq!(sends.entries().len(), 1);
        // The committed row arrives: the entry is dropped.
        sends.reconcile(&[committed("event-1")]);
        assert!(sends.is_empty());
    }

    #[test]
    fn retry_reuses_the_same_op_id_and_two_failures_retry_independently() {
        let mut sends = PendingSends::new();
        let a = sends.begin("first".into(), ts(1));
        let b = sends.begin("second".into(), ts(2));
        sends.set_phase(
            a,
            SendPhase::Failed {
                execution: jeliya_client::Execution::Unknown,
                code: None,
            },
        );
        sends.set_phase(
            b,
            SendPhase::Failed {
                execution: jeliya_client::Execution::DefinitelyNot,
                code: None,
            },
        );
        // Retrying `a` reuses ITS op_id and leaves `b` untouched.
        let (body, op_id) = sends.retry(a).expect("a is retryable");
        assert_eq!(body, "first");
        assert_eq!(op_id, op_id_for(a));
        assert_eq!(op_id, OpId::new("dx-send-0"));
        assert_eq!(sends.get(a).unwrap().phase, SendPhase::Pending);
        assert!(matches!(
            sends.get(b).unwrap().phase,
            SendPhase::Failed { .. }
        ));
        // Retrying a non-failed entry is refused.
        assert!(sends.retry(a).is_none());
    }

    #[test]
    fn a_syncing_entry_with_unknown_event_id_is_not_dropped_by_reconcile() {
        let mut sends = PendingSends::new();
        let id = sends.begin("hi".into(), ts(1));
        sends.set_phase(id, SendPhase::Syncing { event_id: None });
        sends.reconcile(&[committed("anything")]);
        assert_eq!(
            sends.entries().len(),
            1,
            "unknown-id sends reconcile by absence"
        );
    }

    #[test]
    fn newest_signed_at_is_none_without_events() {
        let state = RoomActivityState::new();
        assert_eq!(state.newest_signed_at(), None);
        assert!(state.timeline().is_empty());
    }

    #[test]
    fn newest_signed_at_returns_the_maximum_even_when_events_are_out_of_at_order() {
        use jeliya_api::Reachability;
        use jeliya_client::RoomView;
        let room = RoomId::new("r");
        let mut state = RoomActivityState::new();
        // Three events whose `at` does not increase with pos; newest_signed_at must
        // return the global max (ts(100)), never just the last in slice order.
        let events = vec![
            Event {
                pos: 0,
                event_id: EventId::new("e0"),
                at: ts(50),
                author: Author::Unresolved,
                kind: EventKindContent::Message { body: "a".into() },
            },
            Event {
                pos: 1,
                event_id: EventId::new("e1"),
                at: ts(100),
                author: Author::Unresolved,
                kind: EventKindContent::Message { body: "b".into() },
            },
            Event {
                pos: 2,
                event_id: EventId::new("e2"),
                at: ts(30),
                author: Author::Unresolved,
                kind: EventKindContent::Message { body: "c".into() },
            },
        ];
        state.apply_update(
            &room,
            &jeliya_client::RoomUpdate::Converged(RoomView {
                room_id: room.clone(),
                generation: 1,
                timeline: events,
                members: Vec::new(),
                peers: Vec::new(),
                reachability: Reachability::Connected,
            }),
        );
        assert_eq!(state.newest_signed_at(), Some(ts(100)));
    }

    #[test]
    fn remove_drops_the_entry_completely_and_is_empty_reflects_that() {
        let mut sends = PendingSends::new();
        let a = sends.begin("alpha".into(), ts(1));
        let b = sends.begin("beta".into(), ts(2));
        assert!(!sends.is_empty());
        sends.remove(a);
        assert!(sends.get(a).is_none(), "removed entry is gone");
        assert!(sends.get(b).is_some(), "other entries are unaffected");
        sends.remove(b);
        assert!(sends.is_empty());
    }

    #[test]
    fn get_for_unknown_id_returns_none() {
        let mut sends = PendingSends::new();
        let _id = sends.begin("hi".into(), ts(1));
        // SendId(99) was never minted.
        assert!(sends.get(SendId(99)).is_none());
    }

    #[test]
    fn retry_is_refused_for_pending_and_syncing_entries() {
        let mut sends = PendingSends::new();
        let pending_id = sends.begin("pending".into(), ts(1));
        // A still-Pending entry cannot be retried.
        assert!(
            sends.retry(pending_id).is_none(),
            "Pending entries are not retryable"
        );
        // A Syncing entry is also not retryable.
        sends.set_phase(pending_id, SendPhase::Syncing { event_id: None });
        assert!(
            sends.retry(pending_id).is_none(),
            "Syncing entries are not retryable"
        );
    }
}
