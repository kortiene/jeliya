//! Fleet liveness derivation — the pure half of the `agents.fleet` /
//! `agent.history` reads, exactly as `docs/agent-orchestration.md` §1.2
//! specifies.
//!
//! Liveness is **derived at read time, never stored**, from two real signals:
//! the agent's live peer-connection state (primary) and the timestamp of its
//! most recent `agent_status` event (secondary). THE RULE: a `working` latest
//! status is never sufficient — `working` requires a currently-connected peer
//! AND a fresh working-class status; peer state overrides the last posted
//! label (no "stale working forever"). Nothing here fabricates a count or a
//! heartbeat: every input is a stored event or a `PeerConnState`.

/// Freshness bound for a working-class status while the peer is connected —
/// deliberately above the runner's 15-minute task hard cap plus reporting
/// slack, so a healthy task is never misfiled as stale (contract §1.2).
pub const STALE_WORKING_MS: u64 = 20 * 60_000;

/// Default `agent.history` point cap.
pub const HISTORY_DEFAULT_LIMIT: u32 = 100;

/// The four derived liveness states. Declaration order is the aggregation /
/// sort rank (strongest presence first), so `Ord` gives both the multi-room
/// aggregate (`min`) and the `agents.fleet` ordering directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Liveness {
    /// Connected peer + fresh working-class latest status.
    Working,
    /// Connected peer, idle-class latest label (or no status yet).
    OnlineIdle,
    /// Working-class latest label whose evidence no longer supports
    /// `working`: peer gone, or connected but the status outlived
    /// [`STALE_WORKING_MS`].
    Stale,
    /// No connected peer and no working-class claim left standing.
    Offline,
}

impl Liveness {
    /// The protocol string (`docs/PROTOCOL.md` FleetAgent vocabulary).
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::OnlineIdle => "online-idle",
            Self::Stale => "stale",
            Self::Offline => "offline",
        }
    }

    /// Whether this state counts toward `agents.fleet`'s `active`.
    #[must_use]
    pub fn is_active(self) -> bool {
        matches!(self, Self::Working | Self::OnlineIdle)
    }
}

/// Working-class iff the label is exactly `working` (contract §1.1: any other
/// label — including the reserved `claiming` and unknown free-form labels —
/// classifies as idle-class).
#[must_use]
pub fn is_working_class(label: &str) -> bool {
    label == "working"
}

/// The §1.2 decision table, evaluated top to bottom (first match wins).
///
/// * `connected` — is any of the agent's devices a `PeerConnState::Connected`
///   peer on this daemon's open session for the room. A room the daemon does
///   NOT have open has no live peer state, so callers pass `false` — it is
///   dishonest to report `online-idle`/`working` without a live connection.
/// * `latest` — the newest `agent_status` by the agent in the room, as
///   `(label, ts)`, or `None` if it has never posted one.
/// * `now` — the read-time clock, only ever compared against a real event ts.
#[must_use]
pub fn derive_liveness(connected: bool, latest: Option<(&str, u64)>, now: u64) -> Liveness {
    if !connected {
        return match latest {
            // Row 2: a dead peer's last claim was "working" — stale, never
            // working (THE RULE).
            Some((label, _)) if is_working_class(label) => Liveness::Stale,
            // Rows 1 & 3: offline label, any idle-class label, or no status.
            _ => Liveness::Offline,
        };
    }
    match latest {
        Some((label, ts)) if is_working_class(label) => {
            // Rows 4 & 5: connected working-class — fresh or outlived.
            if now.saturating_sub(ts) <= STALE_WORKING_MS {
                Liveness::Working
            } else {
                Liveness::Stale
            }
        }
        // Row 6: connected, idle-class latest label or no status yet. Peer
        // state overrides the label — even a last-posted "offline".
        _ => Liveness::OnlineIdle,
    }
}

/// Multi-room aggregate (contract §1.2): the strongest per-room state by
/// presence — `working` > `online-idle` > `stale` > `offline`. An agent with
/// no per-room evidence at all is honestly `offline`.
#[must_use]
pub fn aggregate_liveness<I: IntoIterator<Item = Liveness>>(per_room: I) -> Liveness {
    per_room.into_iter().min().unwrap_or(Liveness::Offline)
}

#[cfg(test)]
mod tests {
    use super::{aggregate_liveness, derive_liveness, is_working_class, Liveness, STALE_WORKING_MS};

    const NOW: u64 = 1_783_190_000_000;

    #[test]
    fn only_the_exact_working_label_is_working_class() {
        assert!(is_working_class("working"));
        for label in ["online", "done", "failed", "idle", "offline", "claiming", "Working", "working_hard"] {
            assert!(!is_working_class(label), "{label} must be idle-class");
        }
    }

    #[test]
    fn row1_offline_label_disconnected_is_offline() {
        assert_eq!(
            derive_liveness(false, Some(("offline", NOW)), NOW),
            Liveness::Offline
        );
    }

    #[test]
    fn row2_stale_working_rule_disconnected_working_is_never_working() {
        // THE RULE: even a working status posted "just now" cannot read as
        // working when no device of the agent is a connected peer.
        assert_eq!(
            derive_liveness(false, Some(("working", NOW)), NOW),
            Liveness::Stale
        );
        assert_eq!(
            derive_liveness(false, Some(("working", NOW - 1)), NOW),
            Liveness::Stale
        );
    }

    #[test]
    fn row3_disconnected_idle_class_or_no_status_is_offline() {
        assert_eq!(derive_liveness(false, None, NOW), Liveness::Offline);
        for label in ["online", "done", "failed", "idle", "claiming"] {
            assert_eq!(
                derive_liveness(false, Some((label, NOW)), NOW),
                Liveness::Offline,
                "idle-class {label} without a peer is offline"
            );
        }
    }

    #[test]
    fn row4_connected_fresh_working_is_working() {
        assert_eq!(
            derive_liveness(true, Some(("working", NOW)), NOW),
            Liveness::Working
        );
        assert_eq!(
            derive_liveness(true, Some(("working", NOW - STALE_WORKING_MS)), NOW),
            Liveness::Working,
            "exactly at the bound is still fresh"
        );
    }

    #[test]
    fn row5_connected_outlived_working_is_stale() {
        assert_eq!(
            derive_liveness(true, Some(("working", NOW - STALE_WORKING_MS - 1)), NOW),
            Liveness::Stale
        );
    }

    #[test]
    fn row6_connected_idle_class_or_no_status_is_online_idle() {
        assert_eq!(derive_liveness(true, None, NOW), Liveness::OnlineIdle);
        for label in ["online", "done", "failed", "idle", "claiming"] {
            assert_eq!(
                derive_liveness(true, Some((label, 0)), NOW),
                Liveness::OnlineIdle
            );
        }
        // Peer state overrides even a last-posted "offline".
        assert_eq!(
            derive_liveness(true, Some(("offline", NOW)), NOW),
            Liveness::OnlineIdle
        );
    }

    #[test]
    fn clock_skew_future_status_never_panics_or_goes_stale() {
        // An event ts slightly ahead of `now` (clock skew) reads as fresh.
        assert_eq!(
            derive_liveness(true, Some(("working", NOW + 5_000)), NOW),
            Liveness::Working
        );
    }

    #[test]
    fn aggregate_takes_the_strongest_presence() {
        use Liveness::{Offline, OnlineIdle, Stale, Working};
        assert_eq!(aggregate_liveness([Offline, Working, Stale]), Working);
        assert_eq!(aggregate_liveness([Stale, OnlineIdle]), OnlineIdle);
        assert_eq!(aggregate_liveness([Offline, Stale]), Stale);
        assert_eq!(aggregate_liveness([Offline]), Offline);
        assert_eq!(aggregate_liveness([]), Offline);
    }

    #[test]
    fn rank_orders_working_before_idle_before_stale_before_offline() {
        assert!(Liveness::Working < Liveness::OnlineIdle);
        assert!(Liveness::OnlineIdle < Liveness::Stale);
        assert!(Liveness::Stale < Liveness::Offline);
    }
}
