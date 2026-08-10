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
//! Only `mutating && op_id.is_some()` earns [`ReplayPolicy::ReplayableUnderOpId`].
//! This is deliberately **broader** than the daemon's dedup-ledger set: a
//! `Dedup::Key` on a mutating operation outside that set (`daemon.stop`,
//! `transfer.cancel`, the naturally idempotent mutations, connection-scoped
//! `stream.*`) is also classified replayable, because for every such operation
//! the protocol's idempotency table makes a repeat safe *within a daemon
//! lifetime* — naturally idempotent, connection-scoped re-issue, or a terminal
//! typed error (`daemon.stop` answers `shutdown_in_progress`). The risk
//! direction is therefore a harmless duplicate answer, never a silent double
//! effect; an exact per-operation table remains the §14 Q3 alternative.

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

/// The protocol's `op_id`-deduplicated operation set (protocol-v2
/// §Idempotency and retry) — the only operations whose replay returns the
/// **original** result. Every other operation ignores the envelope `op_id`:
/// a replay would return the SECOND invocation's view (`created: false` for
/// an ensure that created; `shutdown_in_progress` for a stop that executed,
/// whose typed-error classification would then claim `DefinitelyNot` for an
/// executed stop). Nothing outside this set may auto-replay.
fn op_id_deduplicated(op: &str) -> bool {
    matches!(
        op,
        "room.create"
            | "room.leave"
            | "member.remove"
            | "invite.mint"
            | "invite.revoke"
            | "message.send"
            | "status.post"
            | "file.share"
            | "file.fetch"
            | "pipe.publish"
            | "pipe.connect"
            | "pipe.release"
            | "pipe.revoke"
    )
}

impl ReplayPolicy {
    /// Derive the policy from an [`ErasedCall`](crate::backend::ErasedCall)'s
    /// routing facts. Only a mutating operation carrying a caller-chosen
    /// `op_id` is replayable — and only when the driver certifies a **stable
    /// session principal** across reconnects (`stable_principal`): the
    /// daemon's dedup ledger is keyed `(principal, op_id)`, so an adapter
    /// that omits `client_id` receives a fresh ephemeral principal per
    /// connection and a replay under the new principal would re-execute a
    /// mutation whose reply was lost. Without the certification, everything
    /// is `Never` and a disconnect settles honestly as
    /// `Disconnected { Unknown }` instead of auto-replaying.
    pub(crate) fn derive(
        op: &str,
        mutating: bool,
        op_id: Option<&OpId>,
        stable_principal: bool,
    ) -> Self {
        if stable_principal && mutating && op_id.is_some() && op_id_deduplicated(op) {
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
            ReplayPolicy::derive("message.send", true, Some(&op_id), true),
            ReplayPolicy::ReplayableUnderOpId
        );
    }

    #[test]
    fn mutating_without_op_id_never_replays() {
        assert_eq!(
            ReplayPolicy::derive("message.send", true, None, true),
            ReplayPolicy::Never
        );
    }

    #[test]
    fn non_mutating_never_replays_even_with_op_id() {
        let op_id = OpId::new("ignored");
        assert_eq!(
            ReplayPolicy::derive("room.list", false, Some(&op_id), true),
            ReplayPolicy::Never
        );
        assert_eq!(
            ReplayPolicy::derive("room.list", false, None, true),
            ReplayPolicy::Never
        );
    }

    #[test]
    fn an_ephemeral_principal_never_replays() {
        // Without a stable session principal, (principal, op_id) does not
        // survive the reconnect, so nothing may auto-replay.
        let op_id = OpId::new("stable-key");
        assert_eq!(
            ReplayPolicy::derive("message.send", true, Some(&op_id), false),
            ReplayPolicy::Never
        );
    }

    #[test]
    fn a_mutating_op_outside_the_dedup_set_never_replays() {
        // daemon.stop is terminal single-effect: a replay returns
        // shutdown_in_progress, whose typed classification would falsely
        // claim DefinitelyNot for an executed stop.
        let op_id = OpId::new("k");
        assert_eq!(
            ReplayPolicy::derive("daemon.stop", true, Some(&op_id), true),
            ReplayPolicy::Never
        );
        assert_eq!(
            ReplayPolicy::derive("subject.ensure", true, Some(&op_id), true),
            ReplayPolicy::Never
        );
        assert_eq!(
            ReplayPolicy::derive("transfer.cancel", true, Some(&op_id), true),
            ReplayPolicy::Never
        );
    }

    #[test]
    fn only_the_replayable_policy_reports_replayable() {
        assert!(ReplayPolicy::ReplayableUnderOpId.is_replayable());
        assert!(!ReplayPolicy::Never.is_replayable());
    }
}
