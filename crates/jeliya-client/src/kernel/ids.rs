//! Correlation-id allocation and the connection-generation counter (§K3, §K7).
//!
//! Two distinct id spaces, matching `jeliya-api`:
//!
//! - [`CorrelationId`] — the envelope `id: RequestId` (`u64`, `0..=2^53-1`).
//!   It correlates a reply to a request and is unique only while outstanding on
//!   **one** connection; it is reset per connection generation, so it never
//!   carries meaning across a reconnect.
//! - `op_id` — the envelope `OpId` (an opaque string), *caller-supplied*, the
//!   cross-reconnect dedup key. The kernel never generates it (see
//!   [`crate::kernel::replay`]); only correlation ids are allocated here.
//!
//! **Wrap safety.** The allocator issues the next id as a monotonic counter and
//! **skips any candidate currently outstanding**, refusing to reuse an
//! in-flight id. Because the outstanding set is bounded by
//! `queue_depth + in_flight` (both far below `2^53`), a free id always exists;
//! on the astronomically unreachable approach to `MAX_REQUEST_ID` the counter
//! wraps to zero and resumes skipping. A late reply bearing a wrapped-but-since
//! reused id is additionally rejected by generation fencing (§K7), because ids
//! reset per connection — a reused id can only collide *within* one connection,
//! where the skip-outstanding rule already forbids it.

use jeliya_api::{RequestId, MAX_REQUEST_ID};

/// The reply-correlation id: a browser-safe `RequestId`, allocated by the
/// kernel and serialized by the codec (#164).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct CorrelationId(RequestId);

impl CorrelationId {
    /// The wire `RequestId` this correlation id carries.
    pub(crate) fn request_id(self) -> RequestId {
        self.0
    }
}

/// A monotonic, wrap-safe correlation-id allocator, reset per connection
/// generation. It does not itself remember which ids are outstanding — the
/// caller supplies that predicate so the single source of truth stays the
/// in-flight ledger (§K4).
pub(crate) struct IdAllocator {
    /// The next candidate value to try.
    next: u64,
}

impl IdAllocator {
    /// A fresh allocator for a new connection: the id space starts at zero.
    pub(crate) fn new() -> Self {
        Self { next: 0 }
    }

    /// Reset to a fresh per-connection id space (called on each `Connected`).
    pub(crate) fn reset(&mut self) {
        self.next = 0;
    }

    /// Allocate the next free correlation id, skipping any value the
    /// `outstanding` predicate reports as still in use. The outstanding set is
    /// bounded well below `2^53`, so this always terminates with a free id.
    pub(crate) fn allocate(&mut self, outstanding: impl Fn(RequestId) -> bool) -> CorrelationId {
        loop {
            let candidate = self.next;
            // Advance the counter, wrapping at the browser-safe ceiling.
            self.next = if candidate >= MAX_REQUEST_ID {
                0
            } else {
                candidate + 1
            };
            let id = RequestId::new(candidate).expect("candidate stays within MAX_REQUEST_ID");
            if !outstanding(id) {
                return CorrelationId(id);
            }
        }
    }
}

/// The connection-generation counter (§K7). A monotonic `u64` that increments
/// on **every** transition into a live connection; every outstanding call is
/// stamped with the generation it was sent under, and every inbound frame is
/// fenced against it, so a stale-generation reply, push, or teardown is
/// discarded rather than allowed to settle a call or overwrite newer state.
pub(crate) struct GenerationCounter {
    current: u64,
}

impl GenerationCounter {
    /// Start before any connection: the first `Connected` yields generation 1.
    pub(crate) fn new() -> Self {
        Self { current: 0 }
    }

    /// The current live generation.
    pub(crate) fn current(&self) -> u64 {
        self.current
    }

    /// Enter a new live connection, returning the fresh generation.
    pub(crate) fn bump(&mut self) -> u64 {
        // A reconnect storm cannot realistically reach `u64::MAX`; saturate for
        // total honesty rather than wrap into a colliding generation.
        self.current = self.current.saturating_add(1);
        self.current
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn allocates_sequentially_from_zero() {
        let mut alloc = IdAllocator::new();
        let a = alloc.allocate(|_| false);
        let b = alloc.allocate(|_| false);
        assert_eq!(a.request_id().get(), 0);
        assert_eq!(b.request_id().get(), 1);
    }

    #[test]
    fn skips_outstanding_ids() {
        let mut alloc = IdAllocator::new();
        // Pretend 0 and 1 are outstanding; the allocator must land on 2.
        let outstanding: HashSet<u64> = [0, 1].into_iter().collect();
        let id = alloc.allocate(|rid| outstanding.contains(&rid.get()));
        assert_eq!(id.request_id().get(), 2);
    }

    #[test]
    fn reset_restarts_the_space() {
        let mut alloc = IdAllocator::new();
        let _ = alloc.allocate(|_| false);
        let _ = alloc.allocate(|_| false);
        alloc.reset();
        assert_eq!(alloc.allocate(|_| false).request_id().get(), 0);
    }

    /// Seeded near the ceiling with a saturated ledger, the allocator wraps to
    /// zero and still never returns an outstanding id (§K3 boundary property).
    #[test]
    fn wraps_at_the_ceiling_without_colliding_with_outstanding() {
        let mut alloc = IdAllocator::new();
        alloc.next = MAX_REQUEST_ID - 1;
        // The two ids straddling the wrap are outstanding: MAX_REQUEST_ID and 0.
        let outstanding: HashSet<u64> = [MAX_REQUEST_ID, 0].into_iter().collect();

        let first = alloc.allocate(|rid| outstanding.contains(&rid.get()));
        // MAX_REQUEST_ID-1 is free, so it is returned first.
        assert_eq!(first.request_id().get(), MAX_REQUEST_ID - 1);

        // Next candidate is MAX_REQUEST_ID (outstanding) → skip, wrap to 0
        // (outstanding) → skip, land on 1.
        let second = alloc.allocate(|rid| outstanding.contains(&rid.get()));
        assert_eq!(second.request_id().get(), 1);
    }

    #[test]
    fn generation_increments_per_connection() {
        let mut counter = GenerationCounter::new();
        assert_eq!(counter.current(), 0);
        assert_eq!(counter.bump(), 1);
        assert_eq!(counter.bump(), 2);
        assert_eq!(counter.current(), 2);
    }
}
