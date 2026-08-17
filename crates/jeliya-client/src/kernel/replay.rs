//! Deduplication and replay policy: opt-in, bounded, guaranteed-only (§K5).
//!
//! The protocol's idempotency ledger is keyed on `(session principal, op_id)`,
//! and only the operations in its **`op_id`-deduplicated set** guarantee "a
//! replayed `op_id` returns the original result and performs no second
//! effect." The kernel encodes exactly that guarantee as a per-call
//! [`ReplayPolicy`] decided at admission, behind **four** gates, ALL required:
//!
//! | Gate | Why |
//! |---|---|
//! | `mutating == true` | reads are simply re-issued, never "replayed" |
//! | `op_id == Some(_)` | the dedup ledger is keyed on it |
//! | the op is in the protocol's 13-op deduplicated set | outside it the daemon ignores the key: a replay returns the SECOND invocation's view — `created: false` for an ensure that created, `shutdown_in_progress` for an executed `daemon.stop` (whose typed classification would falsely claim `DefinitelyNot`) — a wrong answer, not a duplicate |
//! | the driver certifies dedup-scope continuity (stable principal AND same daemon incarnation) | an ephemeral principal — or a daemon restart, whose in-memory ledger empties while the storage-generation gate still passes — makes a replay re-execute |
//!
//! Everything failing any gate is [`ReplayPolicy::Never`] and settles a
//! disconnect honestly as `Disconnected { Unknown }`. (The earlier
//! `mutating && op_id` heuristic was overturned in review — see spec §K5.)

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

/// The two client-driven byte-stream operations (§S8). They appear in the
/// protocol's `op_id`-deduplicated set (`file.share` mints a dedup key), but a
/// **byte stream never survives its connection**: retrying the bytes requires a
/// new `op_id` from offset zero, and a mid-stream disconnect's `op_id` replay
/// returns the recorded *failure* (`stream_aborted{transport_lost}`), not the
/// original result — the "wrong answer, not a duplicate" rule (§K5). So a stream
/// op is [`ReplayPolicy::Never`] regardless of `mutating`/`op_id`, gated here
/// **before** the dedup-set check so `op_id_deduplicated`'s shared-set meaning
/// stays intact for the daemon-side intent (#233). Recognized by path exactly as
/// `derive` recognizes every other op.
pub(crate) fn is_stream_op(op: &str) -> bool {
    matches!(op, "file.share" | "file.read")
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
        // A byte stream never auto-replays (§S8): the gate runs before the
        // dedup-set check so a `file.share`'s dedup key still means "dedup this
        // op_id" to the daemon, while the client refuses to hold-and-re-send its
        // Text request across a reconnect and strand the stream's byte state.
        if is_stream_op(op) {
            return ReplayPolicy::Never;
        }
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

    /// §S8: `file.share` is `mutating` and lives in the `op_id`-deduplicated
    /// set, so before the stream gate it derived `ReplayableUnderOpId` under a
    /// stable principal — a defect, because a byte stream never survives its
    /// connection. The stream gate forces `Never` regardless.
    #[test]
    fn file_share_stream_op_never_replays_even_with_op_id() {
        let op_id = OpId::new("stable-key");
        assert_eq!(
            ReplayPolicy::derive("file.share", true, Some(&op_id), true),
            ReplayPolicy::Never
        );
    }

    /// §S8: `file.read` is non-mutating and would already be `Never`, but the
    /// gate makes the "streams never replay" rule explicit and path-based, not
    /// an accident of the mutating flag.
    #[test]
    fn file_read_stream_op_never_replays() {
        let op_id = OpId::new("stable-key");
        assert_eq!(
            ReplayPolicy::derive("file.read", false, Some(&op_id), true),
            ReplayPolicy::Never
        );
        assert!(is_stream_op("file.read"));
        assert!(is_stream_op("file.share"));
        assert!(!is_stream_op("message.send"));
    }
}
