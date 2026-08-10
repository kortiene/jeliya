//! The outstanding-call ledger: settle-exactly-once, generation stamping, and
//! late/duplicate/tombstone suppression (§K4, §K6, §K7).
//!
//! The ledger maps a local [`CallId`] to its [`Entry`]. Settlement is a
//! **take**: removing the entry is what settles the call, so a second attempt
//! on the same id finds nothing and is a no-op — a duplicate or late reply can
//! never double-settle or re-open a call. Reply routing is
//! **generation-fenced**: a reply is matched to a call only when it arrives on
//! the same generation the call was sent under (a fresh connection resets the
//! wire-id space), so a stale-generation reply cannot settle a newer call.
//!
//! The senders themselves live in the async driver, not here: the ledger tracks
//! only the bounded per-call *state* the core reasons over, so its size is
//! `queue_depth + 2·in_flight` by construction — queued entries (admission
//! bound), sent-and-awaiting entries (the in-flight throttle), and the
//! tombstone budget the core enforces (§K12).

use std::collections::HashMap;

use jeliya_api::{OpId, RequestId};

use crate::backend::RawJson;
use crate::kernel::replay::ReplayPolicy;
use crate::kernel::timing::{Tick, TimerId};

/// A local, per-call identity assigned at dispatch. It is the ledger key and
/// the driver's reply-sender key; distinct from the wire [`RequestId`], which
/// is assigned only at send time and reset per generation.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub(crate) struct CallId(pub(crate) u64);

/// Whether a call has been put on the wire, and if so under which generation.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Phase {
    /// Admitted to the bounded queue, not yet sent (`Connecting`, or
    /// back-pressured while `Ready` because the in-flight slots are full, or
    /// held for replay across a reconnect).
    Queued,
    /// Sent and awaiting a reply, stamped with its issuing generation and wire
    /// id.
    Sent {
        /// The wire correlation id this call was sent under.
        wire_id: RequestId,
        /// The connection generation the call was sent under.
        generation: u64,
    },
}

/// One outstanding call's bounded state.
pub(crate) struct Entry {
    /// The operation's wire name (for re-send and diagnostics).
    pub(crate) op: &'static str,
    /// The caller's envelope `op_id`, forwarded verbatim on every send/replay.
    pub(crate) op_id: Option<OpId>,
    /// The serialized `in` bytes, cloned into each outbound frame.
    pub(crate) input: RawJson,
    /// The admission byte charge to release on settle/cancel.
    pub(crate) payload_bytes: u64,
    /// Whether the call may be replayed on reconnect (§K5).
    pub(crate) replay: ReplayPolicy,
    /// The per-call deadline timer to cancel on settle.
    pub(crate) deadline_timer: TimerId,
    /// The absolute tick the call's budget elapses at — checked by `flush` so
    /// a queued call whose deadline already passed is never put on the wire.
    pub(crate) deadline_at: Tick,
    /// Queued vs sent, with the sent stamp.
    pub(crate) phase: Phase,
    /// Whether the admission accountant currently carries this call's byte
    /// charge: true from admission until the (re-)send releases it, and set
    /// again when a sent call is held for replay across a reconnect, so held
    /// payloads never escape the byte bound (§K2/§K12).
    pub(crate) holds_charge: bool,
    /// Set once the call has been on the wire at least once — the
    /// never-sent/may-have-executed discriminant for disconnect and stop
    /// settlement (§K6).
    pub(crate) ever_sent: bool,
    /// Set when the caller dropped/cancelled its future while the call was
    /// sent: the entry is kept (a tombstone) so a real late reply is absorbed,
    /// but no value is delivered and the call is never replayed (§K9).
    pub(crate) cancelled: bool,
}

impl Entry {
    /// Whether this entry is currently sent-and-awaiting-reply.
    pub(crate) fn is_sent(&self) -> bool {
        matches!(self.phase, Phase::Sent { .. })
    }
}

/// The outstanding-call ledger.
pub(crate) struct Ledger {
    entries: HashMap<CallId, Entry>,
    /// Reverse index from the current generation's wire ids to call ids, for
    /// O(1) reply routing. Cleared on each reconnect (ids reset).
    wire_index: HashMap<RequestId, CallId>,
}

impl Ledger {
    /// An empty ledger.
    pub(crate) fn new() -> Self {
        Self {
            entries: HashMap::new(),
            wire_index: HashMap::new(),
        }
    }

    /// Insert a freshly-admitted (queued) call.
    pub(crate) fn insert(&mut self, call_id: CallId, entry: Entry) {
        self.entries.insert(call_id, entry);
    }

    /// Whether a call is still in the ledger.
    pub(crate) fn contains(&self, call_id: CallId) -> bool {
        self.entries.contains_key(&call_id)
    }

    /// Borrow a call's entry.
    pub(crate) fn get(&self, call_id: CallId) -> Option<&Entry> {
        self.entries.get(&call_id)
    }

    /// Mutably borrow a call's entry.
    pub(crate) fn get_mut(&mut self, call_id: CallId) -> Option<&mut Entry> {
        self.entries.get_mut(&call_id)
    }

    /// Stamp a queued call as sent under `wire_id`/`generation` and index it for
    /// reply routing.
    pub(crate) fn mark_sent(&mut self, call_id: CallId, wire_id: RequestId, generation: u64) {
        if let Some(entry) = self.entries.get_mut(&call_id) {
            entry.phase = Phase::Sent {
                wire_id,
                generation,
            };
            entry.ever_sent = true;
            self.wire_index.insert(wire_id, call_id);
        }
    }

    /// Take (remove) a call's entry, settling it. Also drops its wire index if
    /// it was sent. Returns the removed entry, or `None` if it was already
    /// settled — the structural guarantee that a call settles **exactly once**.
    pub(crate) fn take(&mut self, call_id: CallId) -> Option<Entry> {
        let entry = self.entries.remove(&call_id)?;
        if let Phase::Sent { wire_id, .. } = entry.phase {
            self.wire_index.remove(&wire_id);
        }
        Some(entry)
    }

    /// Resolve an inbound reply to the call it settles, **generation-fenced**.
    /// Returns `None` when no call matches (a late/duplicate reply) or when the
    /// reply's generation does not match the call's issuing generation (a
    /// stale-generation reply) — in both cases the reply is dropped and strands
    /// nothing.
    pub(crate) fn resolve_reply(&self, wire_id: RequestId, generation: u64) -> Option<CallId> {
        let call_id = *self.wire_index.get(&wire_id)?;
        let entry = self.entries.get(&call_id)?;
        match entry.phase {
            Phase::Sent {
                generation: sent_generation,
                ..
            } if sent_generation == generation => Some(call_id),
            // Stale generation, or somehow not sent: fence it.
            _ => None,
        }
    }

    /// The call owning `timer` (a per-call deadline), if any.
    pub(crate) fn find_by_deadline(&self, timer: TimerId) -> Option<CallId> {
        self.entries
            .iter()
            .find(|(_, entry)| entry.deadline_timer == timer)
            .map(|(call_id, _)| *call_id)
    }

    /// Whether `request_id` is a currently-outstanding wire id — the
    /// skip-outstanding predicate the id allocator consults (§K3).
    pub(crate) fn is_wire_id_outstanding(&self, request_id: RequestId) -> bool {
        self.wire_index.contains_key(&request_id)
    }

    /// Drop the whole wire index on a reconnect: the next connection starts a
    /// fresh id space, so no old wire id may still route a reply.
    pub(crate) fn clear_wire_index(&mut self) {
        self.wire_index.clear();
    }

    /// Drain every entry (total stop / terminal disconnect), leaving the ledger
    /// empty. The caller settles each drained entry exactly once.
    pub(crate) fn drain(&mut self) -> Vec<(CallId, Entry)> {
        self.wire_index.clear();
        self.entries.drain().collect()
    }

    /// Iterate every live `(CallId, &Entry)` — used to reclassify outstanding
    /// calls on a disconnect (§K6). The `CallId` is yielded by value (it is
    /// `Copy`) so the caller can collect ids and then mutate the ledger.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (CallId, &Entry)> {
        self.entries
            .iter()
            .map(|(call_id, entry)| (*call_id, entry))
    }

    /// The number of live entries — bounded by `queue_depth + in_flight`.
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the ledger is empty (asserted after stop, §K11).
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::replay::ReplayPolicy;

    fn entry(op: &'static str) -> Entry {
        Entry {
            op,
            op_id: None,
            input: RawJson::from_string(String::from("{}")),
            payload_bytes: 2,
            replay: ReplayPolicy::Never,
            deadline_timer: TimerId(0),
            deadline_at: Tick(u64::MAX),
            holds_charge: true,
            phase: Phase::Queued,
            ever_sent: false,
            cancelled: false,
        }
    }

    fn rid(v: u64) -> RequestId {
        RequestId::new(v).unwrap()
    }

    #[test]
    fn take_settles_exactly_once() {
        let mut ledger = Ledger::new();
        ledger.insert(CallId(1), entry("room.list"));
        assert!(ledger.take(CallId(1)).is_some());
        // A second take finds nothing — the exactly-once guarantee.
        assert!(ledger.take(CallId(1)).is_none());
    }

    #[test]
    fn resolve_reply_matches_only_on_the_issuing_generation() {
        let mut ledger = Ledger::new();
        ledger.insert(CallId(1), entry("message.send"));
        ledger.mark_sent(CallId(1), rid(5), 3);

        // Same generation: matched.
        assert_eq!(ledger.resolve_reply(rid(5), 3), Some(CallId(1)));
        // Older generation: fenced (dropped).
        assert_eq!(ledger.resolve_reply(rid(5), 2), None);
        // Newer generation: fenced (dropped).
        assert_eq!(ledger.resolve_reply(rid(5), 4), None);
    }

    #[test]
    fn a_late_reply_after_take_finds_nothing() {
        let mut ledger = Ledger::new();
        ledger.insert(CallId(1), entry("message.send"));
        ledger.mark_sent(CallId(1), rid(9), 1);
        // Settle it.
        assert!(ledger.take(CallId(1)).is_some());
        // The late reply for the same wire id now routes nowhere.
        assert_eq!(ledger.resolve_reply(rid(9), 1), None);
    }

    #[test]
    fn a_tombstoned_entry_still_absorbs_its_late_reply() {
        let mut ledger = Ledger::new();
        ledger.insert(CallId(1), entry("message.send"));
        ledger.mark_sent(CallId(1), rid(4), 1);
        // Caller cancelled while sent: entry kept, marked cancelled.
        ledger.get_mut(CallId(1)).unwrap().cancelled = true;
        // The real late reply still resolves to the (tombstoned) call, so the
        // core can absorb-and-reclaim it rather than mis-route it.
        assert_eq!(ledger.resolve_reply(rid(4), 1), Some(CallId(1)));
        assert!(ledger.get(CallId(1)).unwrap().cancelled);
    }

    #[test]
    fn clear_wire_index_fences_prior_generation_ids() {
        let mut ledger = Ledger::new();
        ledger.insert(CallId(1), entry("message.send"));
        ledger.mark_sent(CallId(1), rid(2), 1);
        ledger.clear_wire_index();
        assert_eq!(ledger.resolve_reply(rid(2), 1), None);
        assert!(!ledger.is_wire_id_outstanding(rid(2)));
    }

    #[test]
    fn outstanding_wire_ids_drive_the_skip_predicate() {
        let mut ledger = Ledger::new();
        ledger.insert(CallId(1), entry("message.send"));
        ledger.mark_sent(CallId(1), rid(7), 1);
        assert!(ledger.is_wire_id_outstanding(rid(7)));
        assert!(!ledger.is_wire_id_outstanding(rid(8)));
    }

    #[test]
    fn drain_empties_the_ledger() {
        let mut ledger = Ledger::new();
        ledger.insert(CallId(1), entry("a"));
        ledger.insert(CallId(2), entry("b"));
        let drained = ledger.drain();
        assert_eq!(drained.len(), 2);
        assert!(ledger.is_empty());
    }
}
