//! Byte-aware queued admission and the in-flight throttle bookkeeping (§K2).
//!
//! Two of the kernel's three hard bounds are admission-time refusals, made
//! **visible** as [`CallError::QueueFull`] rather than absorbed:
//!
//! - `queue_depth` — the maximum number of admitted-but-unsent calls.
//! - `outbound_bytes` — a byte cap on the queued outbound payloads (protocol
//!   §Bounded resources: "outbound queues MUST be byte-bounded; a
//!   message-count-only queue is insufficient").
//!
//! The third bound, `in_flight`, is a *throttle* enforced by the core (a call
//! stays queued when the in-flight slots are full) and is **never** a
//! `QueueFull` — see [`crate::kernel::core`].

use crate::error::CallError;

/// The served-limit name reported when the queued **count** cap is hit.
pub(crate) const RESOURCE_QUEUE_DEPTH: &str = "queue_depth";
/// The served-limit name reported when the queued **byte** cap is hit.
pub(crate) const RESOURCE_OUTBOUND_BYTES: &str = "outbound_bytes";

/// The queued-admission accountant: a running count and byte total against two
/// configured caps. It owns no calls itself (the core owns the queue); it only
/// answers "may one more be admitted?" and tracks the release on settle/cancel.
pub(crate) struct Admission {
    queue_depth: u32,
    outbound_bytes: u64,
    queued_count: u32,
    queued_bytes: u64,
}

impl Admission {
    /// A fresh accountant for the given caps.
    pub(crate) fn new(queue_depth: u32, outbound_bytes: u64) -> Self {
        Self {
            queue_depth,
            outbound_bytes,
            queued_count: 0,
            queued_bytes: 0,
        }
    }

    /// Try to admit one call whose serialized `in` is `payload_bytes`. On
    /// success the caps are charged and the caller must later [`release`] the
    /// same byte count; on refusal nothing is charged and a
    /// [`CallError::QueueFull`] names the exhausted resource — `DefinitelyNot`,
    /// because no byte was sent.
    ///
    /// [`release`]: Self::release
    pub(crate) fn try_admit(&mut self, payload_bytes: u64) -> Result<(), CallError> {
        if self.queued_count >= self.queue_depth {
            return Err(CallError::QueueFull {
                resource: RESOURCE_QUEUE_DEPTH,
                limit: self.queue_depth as u64,
            });
        }
        // Charge bytes only if the running total would stay within the cap.
        // `saturating_add` keeps the comparison honest even for a payload near
        // `u64::MAX`; such a payload is refused, never wrapped into "fits".
        if self.queued_bytes.saturating_add(payload_bytes) > self.outbound_bytes {
            return Err(CallError::QueueFull {
                resource: RESOURCE_OUTBOUND_BYTES,
                limit: self.outbound_bytes,
            });
        }
        self.queued_count += 1;
        self.queued_bytes += payload_bytes;
        Ok(())
    }

    /// Re-charge a held call's bytes **without** an admission check: the
    /// call was admitted once and cannot be refused now (it is
    /// settled-pending work held for replay across a reconnect). Subsequent
    /// NEW admissions see the held bytes and refuse — exactly the
    /// backpressure the byte bound exists to provide (§K12).
    pub(crate) fn charge_unchecked(&mut self, payload_bytes: u64) {
        self.queued_count = self.queued_count.saturating_add(1);
        self.queued_bytes = self.queued_bytes.saturating_add(payload_bytes);
    }

    /// Release one previously-admitted call's charge (on send-completion via a
    /// settle, on cancellation, or on disconnect). Saturating so a double
    /// release can never underflow into a spuriously huge free budget.
    pub(crate) fn release(&mut self, payload_bytes: u64) {
        self.queued_count = self.queued_count.saturating_sub(1);
        self.queued_bytes = self.queued_bytes.saturating_sub(payload_bytes);
    }

    /// Drop all accounting to empty (used by total stop, §K11).
    pub(crate) fn clear(&mut self) {
        self.queued_count = 0;
        self.queued_bytes = 0;
    }

    /// The current queued count — asserted `<= queue_depth` by construction.
    #[cfg(test)]
    pub(crate) fn queued_count(&self) -> u32 {
        self.queued_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_past_the_count_cap_with_the_named_resource() {
        let mut admission = Admission::new(2, 1_000);
        assert!(admission.try_admit(1).is_ok());
        assert!(admission.try_admit(1).is_ok());
        let err = admission
            .try_admit(1)
            .expect_err("third call over the count cap");
        assert!(
            matches!(
                err,
                CallError::QueueFull {
                    resource: RESOURCE_QUEUE_DEPTH,
                    limit: 2
                }
            ),
            "{err:?}"
        );
    }

    #[test]
    fn refuses_past_the_byte_cap_with_the_named_resource() {
        let mut admission = Admission::new(100, 10);
        assert!(admission.try_admit(6).is_ok());
        let err = admission.try_admit(5).expect_err("6 + 5 exceeds 10");
        assert!(
            matches!(
                err,
                CallError::QueueFull {
                    resource: RESOURCE_OUTBOUND_BYTES,
                    limit: 10
                }
            ),
            "{err:?}"
        );
    }

    #[test]
    fn release_frees_both_count_and_bytes() {
        let mut admission = Admission::new(1, 10);
        admission.try_admit(10).expect("first fits exactly");
        // Full on both axes.
        assert!(admission.try_admit(1).is_err());
        admission.release(10);
        // Freed: a fresh call fits again.
        assert!(admission.try_admit(10).is_ok());
    }

    #[test]
    fn a_payload_exactly_at_the_cap_is_admitted() {
        let mut admission = Admission::new(1, 10);
        assert!(admission.try_admit(10).is_ok());
    }

    #[test]
    fn release_is_saturating_and_never_underflows() {
        let mut admission = Admission::new(4, 100);
        admission.release(50); // nothing charged yet
        assert_eq!(admission.queued_count(), 0);
        assert!(admission.try_admit(100).is_ok());
    }
}
