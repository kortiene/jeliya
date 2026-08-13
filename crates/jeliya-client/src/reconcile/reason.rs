//! Every observable re-baseline cause, and the reconciler-facing
//! `ResyncRequired { generation, reason }` record (#169 §R3).
//!
//! A [`ResyncReason`] accompanies every re-baseline, so "why did we resync" is
//! a first-class, tested value — never inferred from timing. The core emits it
//! at the *start* of every reconciliation (`Action::EmitResyncRequired`), so the
//! cause is observable before the outcome.

use jeliya_api::{GapReason, GapTo};

/// Why an authoritative re-baseline is being taken. Every detectable gap maps
/// to exactly one arm; the reconciler never re-baselines without naming a
/// cause (#169 AC-1).
///
/// The enum is `#[non_exhaustive]` because a future protocol generation may add
/// a detectable gap cause; a client that matches it must keep a wildcard arm.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum ResyncReason {
    /// First activation of a room — the baseline is built from nothing
    /// (`room.timeline` + `room.members` + `room.peers`).
    Bootstrap,
    /// A `stream.subscribe` / reconnect established (or re-established)
    /// liveness. Every entry into `State::Ready`, and every coalesced-through-
    /// `Interrupted` transition, re-baselines the affected active rooms.
    Reconnect,
    /// A position discontinuity was observed directly, or a `gap` frame
    /// arrived. Carries the wire cause so `backpressure` / `retention` /
    /// `subscription_lapse` stay distinguishable. A locally observed jump (no
    /// wire frame) is classified `subscription_lapse` with an open upper bound,
    /// the honest generic cause for "the live stream skipped positions".
    Gap {
        /// The wire (or synthesized) gap cause.
        reason: GapReason,
        /// The upper end of the gap, if known.
        to: GapTo,
    },
    /// The reconciler's own subscription lagged the bounded fan-out
    /// ([`crate::ClientEvent::Lagged`]), or its per-room reconcile buffer
    /// overflowed: local loss, not a wire gap.
    LocalOverflow {
        /// How many live pushes were lost.
        dropped: u64,
    },
    /// A `resync_required` reply named a position that can no longer be served;
    /// discard back to `from_pos` and re-read from there.
    ResyncRequiredByDaemon {
        /// The position to re-read from.
        from_pos: u64,
    },
    /// Android process resume (or any adapter resume) — `DirectClient`'s path,
    /// with **no** fabricated socket reconnect (#169 §R11).
    Resume,
}

impl ResyncReason {
    /// The coalescing rank (§R9). When two triggers coalesce into the single
    /// pending re-run, the reconciler keeps the **most authoritative** reason,
    /// because the stronger cause implies the weaker's recovery: a
    /// `ResyncRequiredByDaemon` or `Reconnect`/`Resume` outranks a `Gap`, which
    /// outranks a `LocalOverflow`. `Bootstrap` is the initial cause and never
    /// participates in coalescing, so it ranks lowest.
    pub(crate) fn priority(&self) -> u8 {
        match self {
            ResyncReason::Bootstrap => 0,
            ResyncReason::LocalOverflow { .. } => 1,
            ResyncReason::Gap { .. } => 2,
            ResyncReason::Reconnect | ResyncReason::Resume => 3,
            ResyncReason::ResyncRequiredByDaemon { .. } => 4,
        }
    }

    /// Keep whichever of `self` and `other` outranks the other, preferring the
    /// incoming `other` on a tie so the freshest evidence wins.
    pub(crate) fn coalesce(self, other: ResyncReason) -> ResyncReason {
        if other.priority() >= self.priority() {
            other
        } else {
            self
        }
    }

    /// Whether this cause implicates presence, so the reconciliation must also
    /// re-read `room.members` / `room.peers` (§R2, Open Q3). Reconnect, resume,
    /// and a daemon-forced resync can all coincide with membership/presence
    /// having changed while the client could not observe it; a pure position
    /// `gap` or local overflow is events-only.
    pub(crate) fn implicates_presence(&self) -> bool {
        matches!(
            self,
            ResyncReason::Bootstrap
                | ResyncReason::Reconnect
                | ResyncReason::Resume
                | ResyncReason::ResyncRequiredByDaemon { .. }
        )
    }
}

/// The reconciler-facing `ResyncRequired { generation, reason }` record
/// (architecture Decision 4). `generation` is the reconciler-local monotonic
/// epoch the reconciliation is fenced by (§R4); `reason` is its observable
/// cause. Surfaced to consumers via [`RoomUpdate::Resyncing`](crate::RoomUpdate)
/// at the start of every reconciliation.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ResyncRequired {
    /// The reconciler-local epoch this re-baseline is fenced by.
    pub generation: u64,
    /// Why the re-baseline is being taken.
    pub reason: ResyncReason,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priority_orders_daemon_and_reconnect_above_gap_above_overflow() {
        let daemon = ResyncReason::ResyncRequiredByDaemon { from_pos: 7 };
        let reconnect = ResyncReason::Reconnect;
        let gap = ResyncReason::Gap {
            reason: GapReason::Retention,
            to: GapTo::Open,
        };
        let overflow = ResyncReason::LocalOverflow { dropped: 3 };
        assert!(daemon.priority() >= reconnect.priority());
        assert!(reconnect.priority() > gap.priority());
        assert!(gap.priority() > overflow.priority());
        assert!(overflow.priority() > ResyncReason::Bootstrap.priority());
    }

    #[test]
    fn coalesce_keeps_the_stronger_cause() {
        let gap = ResyncReason::Gap {
            reason: GapReason::Backpressure,
            to: GapTo::Open,
        };
        let overflow = ResyncReason::LocalOverflow { dropped: 1 };
        // A gap outranks an overflow, in either coalescing order.
        assert_eq!(gap.clone().coalesce(overflow.clone()), gap);
        assert_eq!(overflow.coalesce(gap.clone()), gap);
    }

    #[test]
    fn presence_is_implicated_only_by_liveness_causes() {
        assert!(ResyncReason::Reconnect.implicates_presence());
        assert!(ResyncReason::Resume.implicates_presence());
        assert!(ResyncReason::Bootstrap.implicates_presence());
        assert!(ResyncReason::ResyncRequiredByDaemon { from_pos: 0 }.implicates_presence());
        assert!(!ResyncReason::Gap {
            reason: GapReason::SubscriptionLapse,
            to: GapTo::Open
        }
        .implicates_presence());
        assert!(!ResyncReason::LocalOverflow { dropped: 9 }.implicates_presence());
    }
}
