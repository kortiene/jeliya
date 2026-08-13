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

use jeliya_api::{Event, EventKindContent};

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
    // A fixed overhead covers `pos`, `at`, the author discriminant, and the
    // enum tags — the parts that do not vary with content length.
    const FIXED_OVERHEAD: u64 = 64;
    let id = event.event_id.as_str().len() as u64;
    let content = content_string_bytes(&event.kind);
    FIXED_OVERHEAD.saturating_add(id).saturating_add(content)
}

/// Sum the lengths of the variable-size string fields an event's content
/// carries. Ids are opaque strings; bodies and names are user text — both
/// dominate an event's retained size.
fn content_string_bytes(kind: &EventKindContent) -> u64 {
    let len = |s: &str| s.len() as u64;
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
            pipe_id, target, ..
        } => len(pipe_id.as_str()).saturating_add(len(&target.host)),
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
