//! The bounded, byte-aware reconcile buffer (#169 §R5, §R15).
//!
//! While a room is reconciling, live `event` pushes for it are held here so
//! they can converge with the baseline read (§R6) instead of being applied
//! against a timeline that does not yet exist. The buffer is bounded on **both**
//! a count (`buffer_depth`) and a byte budget (`buffer_bytes`): a
//! message-count-only bound is insufficient, exactly as the kernel's outbound
//! queue is byte-bounded.
//!
//! **Overflow reports loss; it never drops silently.** When a push would exceed
//! either bound, [`ReconcileBuffer::push`] returns [`PushOutcome::Overflow`] and
//! does not store it. The core then forces a fresh authoritative baseline (§R5)
//! rather than marking the dropped events consumed — the dedup watermark is
//! never advanced past a dropped push, so the forced re-baseline re-reads it.

use std::collections::VecDeque;

use jeliya_api::{Audience, Author, Event, EventKindContent};

/// The outcome of offering one live push to the buffer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PushOutcome {
    /// The push was buffered within both bounds.
    Buffered,
    /// The push would exceed a bound and was **not** stored; the caller must
    /// force a fresh baseline and must not mark the dropped push consumed.
    Overflow,
}

/// A count- and byte-bounded ring of buffered live pushes for one room.
pub(crate) struct ReconcileBuffer {
    /// Max buffered pushes (count bound).
    depth: u32,
    /// Max buffered payload bytes (byte bound).
    bytes_cap: u64,
    /// The held pushes, in arrival order.
    events: VecDeque<Event>,
    /// Running total of the estimated payload bytes currently held.
    bytes: u64,
    /// Total pushes lost to overflow since the last [`Self::drain`], so the
    /// forced re-baseline can name how many were dropped ([`crate::ResyncReason::LocalOverflow`]).
    dropped: u64,
}

impl ReconcileBuffer {
    /// A fresh, empty buffer with the given bounds.
    pub(crate) fn new(depth: u32, bytes_cap: u64) -> Self {
        Self {
            depth,
            bytes_cap,
            events: VecDeque::new(),
            bytes: 0,
            dropped: 0,
        }
    }

    /// Offer one live `event` push. Buffered within both bounds, else
    /// [`PushOutcome::Overflow`] with the push discarded (loss recorded, never
    /// silent). Once any push has overflowed, further pushes also overflow: the
    /// buffer is being abandoned for a fresh baseline, so storing more would
    /// waste the bound without changing the outcome.
    pub(crate) fn push(&mut self, event: Event) -> PushOutcome {
        let size = estimated_event_bytes(&event);
        let would_overflow = self.dropped > 0
            || self.events.len() as u64 >= u64::from(self.depth)
            || self.bytes.saturating_add(size) > self.bytes_cap;
        if would_overflow {
            self.dropped = self.dropped.saturating_add(1);
            return PushOutcome::Overflow;
        }
        self.bytes = self.bytes.saturating_add(size);
        self.events.push_back(event);
        PushOutcome::Buffered
    }

    /// How many pushes are currently held.
    pub(crate) fn len(&self) -> usize {
        self.events.len()
    }

    /// Estimated bytes currently retained.
    pub(crate) fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Record one push that could not be retained because a sibling transient
    /// collection consumed the room's combined count/byte budget.
    pub(crate) fn note_drop(&mut self) {
        self.dropped = self.dropped.saturating_add(1);
    }

    /// How many pushes were lost to overflow since construction/`drain`.
    pub(crate) fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Take the buffered pushes for convergence, leaving the buffer empty and
    /// resetting its byte and loss accounting. The returned pushes are in
    /// arrival order; the caller sorts by `pos` before converging (§R6).
    pub(crate) fn drain(&mut self) -> Vec<Event> {
        self.bytes = 0;
        self.dropped = 0;
        self.events.drain(..).collect()
    }
}

/// A cheap, deterministic estimate of the memory one buffered event retains:
/// the lengths of its variable-size string fields plus a fixed overhead for the
/// scalar fields. It is a **memory proxy for the byte bound**, not a wire size,
/// and reads no clock and makes no RNG or serde call, so the sans-IO core stays
/// pure (the `boundaries.rs` reconcile scan asserts this).
pub(crate) fn estimated_event_bytes(event: &Event) -> u64 {
    // A fixed overhead covers `pos`, `at`, enum tags, and scalar fields; the
    // variable event id, resolved author subject, and content strings are added
    // separately below.
    const FIXED_OVERHEAD: u64 = 64;
    const STRING_OVERHEAD: u64 = 24;
    let string_bytes = |value: &str| STRING_OVERHEAD.saturating_add(value.len() as u64);
    let id = string_bytes(event.event_id.as_str());
    let author = match &event.author {
        Author::Resolved { subject_id, .. } => string_bytes(subject_id.as_str()),
        Author::Unresolved => 0,
    };
    let content = content_string_bytes(&event.kind);
    FIXED_OVERHEAD
        .saturating_add(id)
        .saturating_add(author)
        .saturating_add(content)
}

/// Whether every opaque identifier nested in an event fits the reconciler's
/// configured identifier ceiling. User text (message bodies, room/file names,
/// hosts and digests) is governed by the event/reply byte limits instead.
pub(crate) fn event_identifiers_within(event: &Event, max: u32) -> bool {
    let within = |value: &str| value.len() as u64 <= u64::from(max);
    if !within(event.event_id.as_str()) {
        return false;
    }
    if let Author::Resolved { subject_id, .. } = &event.author {
        if !within(subject_id.as_str()) {
            return false;
        }
    }
    match &event.kind {
        EventKindContent::RoomCreated { .. }
        | EventKindContent::Message { .. }
        | EventKindContent::AgentStatus { .. } => true,
        EventKindContent::MemberJoined { subject_id, .. }
        | EventKindContent::MemberLeft { subject_id } => within(subject_id.as_str()),
        EventKindContent::MemberRemoved { subject_id, by } => {
            within(subject_id.as_str()) && within(by.as_str())
        }
        EventKindContent::InviteRevoked { invite_id } => within(invite_id.as_str()),
        EventKindContent::FileShared { file_id, .. } => within(file_id.as_str()),
        EventKindContent::PipePublished {
            pipe_id, audience, ..
        } => {
            within(pipe_id.as_str())
                && match audience {
                    Audience::Room => true,
                    Audience::Subjects { subject_ids } => subject_ids
                        .iter()
                        .all(|subject_id| within(subject_id.as_str())),
                }
        }
        EventKindContent::PipeRevoked { pipe_id } => within(pipe_id.as_str()),
    }
}

/// Sum the lengths of the variable-size string fields an event's content
/// carries. Ids are opaque strings; bodies and names are user text — both
/// dominate an event's retained size.
fn content_string_bytes(kind: &EventKindContent) -> u64 {
    const STRING_OVERHEAD: u64 = 24;
    let len = |s: &str| STRING_OVERHEAD.saturating_add(s.len() as u64);
    match kind {
        EventKindContent::RoomCreated { name } => len(name),
        EventKindContent::Message { body } => len(body),
        EventKindContent::AgentStatus { .. } => 0,
        EventKindContent::MemberJoined { subject_id, .. } => len(subject_id.as_str()),
        EventKindContent::MemberLeft { subject_id } => len(subject_id.as_str()),
        EventKindContent::MemberRemoved { subject_id, by } => {
            len(subject_id.as_str()).saturating_add(len(by.as_str()))
        }
        EventKindContent::InviteRevoked { invite_id } => len(invite_id.as_str()),
        EventKindContent::FileShared {
            file_id,
            name,
            digest,
            ..
        } => len(file_id.as_str())
            .saturating_add(len(name))
            .saturating_add(len(digest)),
        EventKindContent::PipePublished {
            pipe_id,
            target,
            audience,
        } => {
            let audience_bytes = match audience {
                Audience::Room => 0,
                Audience::Subjects { subject_ids } => subject_ids
                    .iter()
                    .map(|subject_id| len(subject_id.as_str()))
                    .fold(0_u64, u64::saturating_add),
            };
            len(pipe_id.as_str())
                .saturating_add(len(&target.host))
                .saturating_add(audience_bytes)
        }
        EventKindContent::PipeRevoked { pipe_id } => len(pipe_id.as_str()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `message` event without a `time` dependency by deserializing the
    /// wire form (`jeliya-client` carries `serde_json`, not `time`).
    fn message_event(pos: u64, id: &str, body: &str) -> Event {
        let json = format!(
            "{{\"pos\":{pos},\"event_id\":\"{id}\",\"at\":\"1970-01-01T00:00:00Z\",\
             \"author\":{{\"state\":\"unresolved\"}},\"kind\":\"message\",\
             \"content\":{{\"body\":\"{body}\"}}}}"
        );
        serde_json::from_str(&json).expect("event json deserializes")
    }

    #[test]
    fn count_bound_overflows_without_storing() {
        let mut buffer = ReconcileBuffer::new(2, u64::MAX);
        assert_eq!(
            buffer.push(message_event(1, "a", "x")),
            PushOutcome::Buffered
        );
        assert_eq!(
            buffer.push(message_event(2, "b", "y")),
            PushOutcome::Buffered
        );
        // The third exceeds the depth bound: overflow, not stored.
        assert_eq!(
            buffer.push(message_event(3, "c", "z")),
            PushOutcome::Overflow
        );
        assert_eq!(buffer.len(), 2);
        assert_eq!(buffer.dropped(), 1);
    }

    #[test]
    fn byte_bound_overflows_without_storing() {
        // A tiny byte cap that the fixed overhead alone exceeds after one push.
        let mut buffer = ReconcileBuffer::new(
            u32::MAX,
            estimated_event_bytes(&message_event(1, "a", "hello")),
        );
        assert_eq!(
            buffer.push(message_event(1, "a", "hello")),
            PushOutcome::Buffered
        );
        assert_eq!(
            buffer.push(message_event(2, "b", "world")),
            PushOutcome::Overflow
        );
        assert_eq!(buffer.len(), 1);
    }

    #[test]
    fn drain_empties_and_resets_accounting() {
        let mut buffer = ReconcileBuffer::new(8, u64::MAX);
        buffer.push(message_event(1, "a", "x"));
        buffer.push(message_event(2, "b", "y"));
        let drained = buffer.drain();
        assert_eq!(drained.len(), 2);
        assert_eq!(buffer.len(), 0);
        assert_eq!(buffer.dropped(), 0);
    }

    #[test]
    fn pipe_audience_subject_ids_count_toward_byte_budget() {
        fn pipe_event(pos: u64, id: &str, subject_ids: &str) -> Event {
            let json = format!(
                r#"{{"pos":{pos},"event_id":"{id}","at":"1970-01-01T00:00:00Z","author":{{"state":"unresolved"}},"kind":"pipe_published","content":{{"pipe_id":"pipe","target":{{"host":"127.0.0.1","port":1}},"audience":{{"state":"subjects","subject_ids":[{subject_ids}]}}}}}}"#
            );
            serde_json::from_str(&json).expect("pipe event json deserializes")
        }

        let short = pipe_event(1, "a", r#""s""#);
        let long = pipe_event(2, "b", r#""a-very-long-subject-id""#);
        let cap = estimated_event_bytes(&short).saturating_mul(2);
        let mut buffer = ReconcileBuffer::new(2, cap);
        assert_eq!(buffer.push(short), PushOutcome::Buffered);
        assert_eq!(
            buffer.push(long),
            PushOutcome::Overflow,
            "audience subject ids must be included in the byte estimate"
        );
    }

    #[test]
    fn resolved_author_subject_id_counts_toward_byte_budget() {
        fn authored_event(pos: u64, id: &str, subject_id: &str) -> Event {
            let json = format!(
                r#"{{"pos":{pos},"event_id":"{id}","at":"1970-01-01T00:00:00Z","author":{{"state":"resolved","subject_id":"{subject_id}","role":"member","standing":"active"}},"kind":"message","content":{{"body":"x"}}}}"#
            );
            serde_json::from_str(&json).expect("resolved-author event deserializes")
        }
        let short = authored_event(1, "a", "s");
        let long = authored_event(2, "b", "a-very-long-author-subject-id");
        assert!(
            estimated_event_bytes(&long) > estimated_event_bytes(&short),
            "resolved author subject ids must be included in the estimate"
        );
    }

    #[test]
    fn overflow_latches_until_drain() {
        let mut buffer = ReconcileBuffer::new(1, u64::MAX);
        assert_eq!(
            buffer.push(message_event(1, "a", "x")),
            PushOutcome::Buffered
        );
        assert_eq!(
            buffer.push(message_event(2, "b", "y")),
            PushOutcome::Overflow
        );
        // Even though the second only overflowed the count, subsequent pushes
        // stay overflow until drain — the buffer is being abandoned.
        assert_eq!(
            buffer.push(message_event(3, "c", "z")),
            PushOutcome::Overflow
        );
        assert_eq!(buffer.dropped(), 2);
    }
}
