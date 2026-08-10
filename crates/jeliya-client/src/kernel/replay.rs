//! Deduplication and replay policy: opt-in, bounded, guaranteed-only (§K5).
//!
//! The protocol's idempotency ledger is keyed on `(session principal, op_id)`,
//! and only the mutating operations that accept an envelope `op_id` guarantee
//! "a replayed `op_id` returns the original result and performs no second
//! effect." The kernel encodes exactly that guarantee as a per-call
//! [`ReplayPolicy`] decided at admission:
//!
//! | Call shape (from `ErasedCall`)                         | Policy                  |
//! |--------------------------------------------------------|-------------------------|
//! | `mutating == true` **and** `op_id == Some(_)`          | `ReplayableUnderOpId`   |
//! | `mutating == true` **and** `op_id == None`             | `Never`                 |
//! | `mutating == false` (any `op_id`)                      | `Never`                 |
//!
//! Only `mutating && op_id.is_some()` earns [`ReplayPolicy::ReplayableUnderOpId`],
//! matching the protocol's "`op_id` deduplicated" row. A `Dedup::Key` on a
//! non-deduplicating operation stays [`ReplayPolicy::Never`] even though the
//! envelope still carries the `op_id` (the daemon "accepts it and ignores it"):
//! the kernel does not manufacture a replay guarantee the protocol does not
//! give. The heuristic never *under*-settles a mutation as executed-when-it-was
//! not; its only risk is treating a would-be-replayable call as `Never`, which
//! is the safe direction (§K5 note; the exact-table alternative is §14 Q3).

use jeliya_api::OpId;

/// Whether a call may be auto-replayed on the next connection after a
/// disconnect (§K5).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ReplayPolicy {
    /// Never auto-replayed. On disconnect it settles with the
    /// never-sent/may-have-executed classification (§K6). This is the default
    /// and covers every non-mutating call and every mutation without an
    /// `op_id`.
    Never,
    /// MAY be replayed under the same `op_id` on the next connection, bounded by
    /// the reconnect budget; the daemon's ledger returns the original result or
    /// the committed error, so the second attempt performs no second effect.
    ReplayableUnderOpId,
}

impl ReplayPolicy {
    /// Derive the policy from an [`ErasedCall`](crate::backend::ErasedCall)'s
    /// routing facts. Only a mutating operation carrying a caller-chosen
    /// `op_id` is replayable.
    pub(crate) fn derive(mutating: bool, op_id: Option<&OpId>) -> Self {
        if mutating && op_id.is_some() {
            ReplayPolicy::ReplayableUnderOpId
        } else {
            ReplayPolicy::Never
        }
    }

    /// Whether a held call under this policy may be re-sent on reconnect.
    pub(crate) fn is_replayable(self) -> bool {
        matches!(self, ReplayPolicy::ReplayableUnderOpId)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutating_with_op_id_is_replayable() {
        let op_id = OpId::new("stable-key");
        assert_eq!(
            ReplayPolicy::derive(true, Some(&op_id)),
            ReplayPolicy::ReplayableUnderOpId
        );
    }

    #[test]
    fn mutating_without_op_id_never_replays() {
        assert_eq!(ReplayPolicy::derive(true, None), ReplayPolicy::Never);
    }

    #[test]
    fn non_mutating_never_replays_even_with_op_id() {
        let op_id = OpId::new("ignored");
        assert_eq!(
            ReplayPolicy::derive(false, Some(&op_id)),
            ReplayPolicy::Never
        );
        assert_eq!(ReplayPolicy::derive(false, None), ReplayPolicy::Never);
    }

    #[test]
    fn only_the_replayable_policy_reports_replayable() {
        assert!(ReplayPolicy::ReplayableUnderOpId.is_replayable());
        assert!(!ReplayPolicy::Never.is_replayable());
    }
}
