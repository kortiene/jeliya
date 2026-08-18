//! The truthful status display seam (§6, D8): every closed API vocabulary mapped
//! to catalog copy by an **exhaustive `match`**, so `rustc` guarantees no arm is
//! left unlabelled (the compiler-total form of AC-7), and the **absent** arms
//! (`LatestStatus::Absent`, `LastSeen::Absent`, a Closed-room presence) map to a
//! localized "no status yet" / "never seen" / presence-absent label — never a
//! blank and never a fabricated liveness.
//!
//! Because the API enums are **closed** (protocol v2 "never silently
//! reclassify"), a daemon value newer than this build fails the typed read and
//! surfaces as [`LoadState::Failed`](crate::view::load::LoadState::Failed), not a
//! silent drop — the honest reconciliation of "unknown statuses have a safe
//! localized fallback" with "closed vocabularies never reclassify".
//!
//! It is a sibling of `l10n/wire.rs` (kept separate only to avoid one 400-line
//! file): the raw-string role/status/path functions there keep their forward-
//! compatible passthrough; here the typed enums are total.

use jeliya_api::{
    LastSeen, LatestStatus, Link, LinkReason, Liveness, Reachability, Redeemability, Role,
    Severity, Standing, StatusLabel, Timestamp,
};

use crate::components::StatusTone;
use crate::l10n::{Catalog, Formats};
use crate::view::alias::ResolvedName;

/// The room-session fact: **Open** (`live`) / **Closed**.
pub fn room_session_label(strings: &dyn Catalog, live: bool) -> &'static str {
    if live {
        strings.room_open()
    } else {
        strings.room_closed()
    }
}

/// Signed membership standing: **Member** (`Active`) / **Left** / **Removed**.
/// The retired v1 "Active" display word is held retired — `Active` renders
/// **Member** via the existing wire catalog (contract §"Status vocabulary").
pub fn standing_label(strings: &dyn Catalog, standing: Standing) -> &'static str {
    match standing {
        Standing::Active => strings.wire_status_active(),
        Standing::Left => strings.wire_status_left(),
        Standing::Removed => strings.wire_status_removed(),
    }
}

/// A member role: **Authority** / **Member** (reuses the existing wire catalog).
pub fn role_label(strings: &dyn Catalog, role: Role) -> &'static str {
    match role {
        Role::Authority => strings.wire_role_authority(),
        Role::Member => strings.wire_role_member(),
    }
}

/// Whole-room reachability. `Alone` renders "No peers connected" — the absence
/// of a link is not solitude (contract: no "Alone in this room").
pub fn reachability_label(strings: &dyn Catalog, reachability: Reachability) -> &'static str {
    match reachability {
        Reachability::Connecting => strings.reachability_connecting(),
        Reachability::Connected => strings.reachability_connected(),
        Reachability::Alone => strings.reachability_alone(),
        Reachability::Offline => strings.reachability_offline(),
    }
}

/// Agent liveness: **Working** / **Online** / **Offline** / **Stale**. The
/// **Live** filter (Working+Online) is not a liveness value; it is the filter
/// label (`fleet_live_filter`).
pub fn liveness_label(strings: &dyn Catalog, liveness: Liveness) -> &'static str {
    match liveness {
        Liveness::Working => strings.liveness_working(),
        Liveness::OnlineIdle => strings.liveness_online(),
        Liveness::Offline => strings.liveness_offline(),
        Liveness::Stale => strings.liveness_stale(),
    }
}

/// The latest posted status label. `Blocked` reads "needs a person".
pub fn status_label_word(strings: &dyn Catalog, label: StatusLabel) -> &'static str {
    match label {
        StatusLabel::Online => strings.status_label_online(),
        StatusLabel::Idle => strings.status_label_idle(),
        StatusLabel::Claiming => strings.status_label_claiming(),
        StatusLabel::Working => strings.status_label_working(),
        StatusLabel::Done => strings.status_label_done(),
        StatusLabel::Failed => strings.status_label_failed(),
        StatusLabel::Blocked => strings.status_label_blocked(),
    }
}

/// The latest status as a rendered word — the label when present, the localized
/// "no status yet" when [`LatestStatus::Absent`] (never a blank, never a
/// fabricated liveness).
pub fn latest_status_label(strings: &dyn Catalog, latest: &LatestStatus) -> &'static str {
    match latest {
        LatestStatus::Present { label, .. } => status_label_word(strings, *label),
        LatestStatus::Absent => strings.status_none_yet(),
    }
}

/// Invite redeemability: **Outstanding** / **Expired** / **Revoked** /
/// **Redeemed**.
pub fn redeemability_label(strings: &dyn Catalog, redeemability: Redeemability) -> &'static str {
    match redeemability {
        Redeemability::Outstanding => strings.redeemability_outstanding(),
        Redeemability::Expired => strings.redeemability_expired(),
        Redeemability::Revoked => strings.redeemability_revoked(),
        Redeemability::Redeemed => strings.redeemability_redeemed(),
    }
}

/// The reason a per-device link is absent.
pub fn link_reason_label(strings: &dyn Catalog, reason: LinkReason) -> &'static str {
    match reason {
        LinkReason::NeverDialed => strings.link_reason_never_dialed(),
        LinkReason::DialFailed => strings.link_reason_dial_failed(),
        LinkReason::NoRoute => strings.link_reason_no_route(),
        LinkReason::Closed => strings.link_reason_closed(),
    }
}

/// A per-device link as a rendered string. `direct`/`relay` are Tier-2 tokens,
/// rendered verbatim in every locale (via the existing wire catalog); a
/// `NotConnected` renders the localized "not connected" plus its reason.
pub fn link_label(strings: &dyn Catalog, link: &Link) -> String {
    match link {
        Link::Direct { .. } => strings.wire_path_direct().to_string(),
        Link::Relay { .. } => strings.wire_path_relay().to_string(),
        Link::NotConnected { reason } => {
            format!(
                "{} ({})",
                strings.link_not_connected(),
                link_reason_label(strings, *reason)
            )
        }
    }
}

/// The last-observed instant as a rendered date, or the localized "never seen"
/// when [`LastSeen::Absent`].
pub fn last_seen_label(strings: &dyn Catalog, formats: Formats, last_seen: &LastSeen) -> String {
    match last_seen {
        LastSeen::Present { at } => date_of(formats, *at),
        LastSeen::Absent => strings.last_seen_never().to_string(),
    }
}

/// Format a [`Timestamp`] as a formatting-locale date (day/month/year).
pub fn date_of(formats: Formats, ts: Timestamp) -> String {
    let t = ts.into_inner();
    formats.date(t.day() as u32, t.month() as u32, t.year())
}

/// The dot tone for a served [`Severity`] (attention), never re-derived from the
/// label (D10).
pub fn severity_tone(severity: Severity) -> StatusTone {
    match severity {
        Severity::Ok => StatusTone::Positive,
        Severity::Failed => StatusTone::Danger,
        Severity::Review => StatusTone::Warn,
    }
}

/// The dot tone for a [`Liveness`] — a distinct fact from the status severity
/// (D10), so a `Stale` agent's dot never reads as healthy.
pub fn liveness_tone(liveness: Liveness) -> StatusTone {
    match liveness {
        Liveness::Working | Liveness::OnlineIdle => StatusTone::Positive,
        Liveness::Stale => StatusTone::Warn,
        Liveness::Offline => StatusTone::Neutral,
    }
}

/// Resolve a [`ResolvedName`] to its visible string, choosing the fallback
/// wording from the catalog: the unlabelled self is "You"; an unlabelled peer is
/// its short id (already carried in the variant).
pub fn resolved_name(strings: &dyn Catalog, name: &ResolvedName) -> String {
    match name {
        ResolvedName::SelfAlias(alias) | ResolvedName::PeerAlias(alias) => alias.clone(),
        ResolvedName::SelfDefault => strings.self_you().to_string(),
        ResolvedName::PeerShort(short) => short.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::l10n::{En, Fr};

    /// Every closed-enum arm renders a non-empty, distinct label in EN and FR.
    /// (Exhaustive-match totality is compile-time; this asserts distinctness and
    /// non-emptiness and the absent-arm fallbacks.)
    #[test]
    fn every_vocabulary_arm_is_nonempty_and_distinct() {
        for cat in [&En as &dyn Catalog, &Fr as &dyn Catalog] {
            let reach = [
                reachability_label(cat, Reachability::Connecting),
                reachability_label(cat, Reachability::Connected),
                reachability_label(cat, Reachability::Alone),
                reachability_label(cat, Reachability::Offline),
            ];
            assert_distinct_nonempty(&reach);

            let live = [
                liveness_label(cat, Liveness::Working),
                liveness_label(cat, Liveness::OnlineIdle),
                liveness_label(cat, Liveness::Offline),
                liveness_label(cat, Liveness::Stale),
            ];
            assert_distinct_nonempty(&live);

            let labels = [
                status_label_word(cat, StatusLabel::Online),
                status_label_word(cat, StatusLabel::Idle),
                status_label_word(cat, StatusLabel::Claiming),
                status_label_word(cat, StatusLabel::Working),
                status_label_word(cat, StatusLabel::Done),
                status_label_word(cat, StatusLabel::Failed),
                status_label_word(cat, StatusLabel::Blocked),
            ];
            assert_distinct_nonempty(&labels);

            let redeem = [
                redeemability_label(cat, Redeemability::Outstanding),
                redeemability_label(cat, Redeemability::Expired),
                redeemability_label(cat, Redeemability::Revoked),
                redeemability_label(cat, Redeemability::Redeemed),
            ];
            assert_distinct_nonempty(&redeem);

            // Absent-arm fallbacks are non-empty and are NOT a fabricated status.
            assert!(!latest_status_label(cat, &LatestStatus::Absent).is_empty());
            assert!(!cat.last_seen_never().is_empty());
            // Open/Closed differ.
            assert_ne!(
                room_session_label(cat, true),
                room_session_label(cat, false)
            );
        }
    }

    #[test]
    fn tier2_link_tokens_are_verbatim_in_every_locale() {
        use jeliya_api::Timestamp;
        use time::OffsetDateTime;
        let ts = Timestamp::new(OffsetDateTime::UNIX_EPOCH);
        for cat in [&En as &dyn Catalog, &Fr as &dyn Catalog] {
            assert_eq!(link_label(cat, &Link::Direct { since: ts }), "direct");
            assert_eq!(link_label(cat, &Link::Relay { since: ts }), "relay");
        }
    }

    #[test]
    fn severity_and_liveness_tones_are_distinct_facts() {
        // A Stale agent's liveness tone is not the healthy tone.
        assert_ne!(
            liveness_tone(Liveness::Stale),
            liveness_tone(Liveness::Working)
        );
        assert_eq!(severity_tone(Severity::Failed), StatusTone::Danger);
        assert_eq!(severity_tone(Severity::Review), StatusTone::Warn);
    }

    fn assert_distinct_nonempty(labels: &[&str]) {
        for (i, a) in labels.iter().enumerate() {
            assert!(!a.is_empty(), "empty label at {i}");
            for b in &labels[i + 1..] {
                assert_ne!(a, b, "labels must be distinct: {a:?}");
            }
        }
    }
}
