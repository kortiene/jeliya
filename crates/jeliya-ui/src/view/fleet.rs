//! The Agent Fleet fold (§7.3, D9, D10): `fleet.list` → a view grouped by
//! **attention severity**, with the **Live** filter, and the room-scoped
//! projection the Agents destination reuses.
//!
//! Liveness (`Liveness`) and the latest posted status (`LatestStatus.label`) are
//! kept as **two separate facts** (D10): a `Stale` agent whose last label was
//! `Working` is never blended into "Working" here. Attention severity is the
//! **served** `Severity` derived from the latest status label (never re-derived
//! from liveness, and `Stale`/`Offline` are liveness, not attention — §"Recency").

use jeliya_api::{
    FleetListOut, FleetRow, LastSeen, LatestStatus, Liveness, RoomId, Severity, SubjectId,
};

/// The attention bucket a fleet row falls into, from its served status severity.
/// Attention is about *work*, not liveness: an `Offline`/`Stale` agent with a
/// nominal last status is `Ok` here.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum AttentionGroup {
    /// Stopped and needs a person (`Severity::Review`, e.g. a `Blocked` status).
    NeedsPerson,
    /// Work failed (`Severity::Failed`).
    Failed,
    /// Nominal — no attention needed.
    Ok,
}

impl AttentionGroup {
    /// The bucket for a served [`Severity`].
    pub fn from_severity(severity: Severity) -> Self {
        match severity {
            Severity::Review => AttentionGroup::NeedsPerson,
            Severity::Failed => AttentionGroup::Failed,
            Severity::Ok => AttentionGroup::Ok,
        }
    }
}

/// The order attention groups are presented in — the actionable ones first.
pub const ATTENTION_ORDER: [AttentionGroup; 3] = [
    AttentionGroup::NeedsPerson,
    AttentionGroup::Failed,
    AttentionGroup::Ok,
];

/// One fleet row, with liveness and latest status held apart (D10).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FleetRowView {
    /// The agent's subject.
    pub subject_id: SubjectId,
    /// The room it is in.
    pub room_id: RoomId,
    /// Derived liveness — a fact of its own, never merged with the label.
    pub liveness: Liveness,
    /// The newest posted status, or `Absent`.
    pub latest: LatestStatus,
    /// When last observed, or `Absent`.
    pub last_seen: LastSeen,
    /// The served attention severity of the latest status (`Ok` when Absent).
    pub attention: AttentionGroup,
}

impl FleetRowView {
    /// Whether this row passes the **Live** filter — `Working` or `OnlineIdle`.
    /// `Offline`/`Stale` are not live.
    pub fn is_live(&self) -> bool {
        matches!(self.liveness, Liveness::Working | Liveness::OnlineIdle)
    }

    fn from_row(row: &FleetRow) -> Self {
        let attention = match &row.latest_status {
            LatestStatus::Present { label, .. } => AttentionGroup::from_severity(label.severity()),
            LatestStatus::Absent => AttentionGroup::Ok,
        };
        FleetRowView {
            subject_id: row.subject_id.clone(),
            room_id: row.room_id.clone(),
            liveness: row.liveness,
            latest: row.latest_status.clone(),
            last_seen: row.last_seen.clone(),
            attention,
        }
    }
}

/// The folded fleet — all rows in stable order, with grouping/filtering helpers.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FleetView {
    /// Every agent, in the order the daemon returned them.
    pub rows: Vec<FleetRowView>,
}

/// Which agents a Fleet view shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FleetFilter {
    /// Every agent.
    All,
    /// Only `Working`/`OnlineIdle` agents (the **Live** filter).
    Live,
}

impl FleetView {
    /// Whether the fleet is empty (a real "no agents" answer).
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The rows passing `filter`, in the daemon's order.
    pub fn filtered(&self, filter: FleetFilter) -> Vec<&FleetRowView> {
        self.rows
            .iter()
            .filter(|row| match filter {
                FleetFilter::All => true,
                FleetFilter::Live => row.is_live(),
            })
            .collect()
    }

    /// The rows passing `filter`, bucketed by attention in [`ATTENTION_ORDER`].
    /// Empty groups are omitted, so a fleet with no failures shows no "Failed"
    /// heading.
    pub fn grouped(&self, filter: FleetFilter) -> Vec<(AttentionGroup, Vec<&FleetRowView>)> {
        let rows = self.filtered(filter);
        ATTENTION_ORDER
            .iter()
            .filter_map(|group| {
                let members: Vec<&FleetRowView> = rows
                    .iter()
                    .copied()
                    .filter(|r| r.attention == *group)
                    .collect();
                (!members.is_empty()).then_some((*group, members))
            })
            .collect()
    }
}

/// Fold `fleet.list` into a [`FleetView`].
pub fn fold_fleet(fleet: &FleetListOut) -> FleetView {
    FleetView {
        rows: fleet.agents.iter().map(FleetRowView::from_row).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jeliya_api::{StatusLabel, Timestamp};
    use time::OffsetDateTime;

    fn ts() -> Timestamp {
        Timestamp::new(OffsetDateTime::UNIX_EPOCH)
    }

    fn row(subject: &str, room: &str, liveness: Liveness, label: Option<StatusLabel>) -> FleetRow {
        FleetRow {
            subject_id: SubjectId::new(subject),
            room_id: RoomId::new(room),
            liveness,
            latest_status: match label {
                Some(label) => LatestStatus::Present { label, at: ts() },
                None => LatestStatus::Absent,
            },
            last_seen: LastSeen::Absent,
        }
    }

    #[test]
    fn attention_uses_the_served_label_severity_never_liveness() {
        // A Stale agent whose last label was Working is Ok attention (no blend).
        let view = fold_fleet(&FleetListOut {
            agents: vec![row("a", "r", Liveness::Stale, Some(StatusLabel::Working))],
        });
        assert_eq!(view.rows[0].attention, AttentionGroup::Ok);
        // Blocked → NeedsPerson; Failed → Failed.
        let view = fold_fleet(&FleetListOut {
            agents: vec![
                row("b", "r", Liveness::Offline, Some(StatusLabel::Blocked)),
                row("c", "r", Liveness::Working, Some(StatusLabel::Failed)),
            ],
        });
        assert_eq!(view.rows[0].attention, AttentionGroup::NeedsPerson);
        assert_eq!(view.rows[1].attention, AttentionGroup::Failed);
    }

    #[test]
    fn an_absent_latest_status_needs_no_attention() {
        let view = fold_fleet(&FleetListOut {
            agents: vec![row("a", "r", Liveness::Offline, None)],
        });
        assert_eq!(view.rows[0].attention, AttentionGroup::Ok);
    }

    #[test]
    fn the_live_filter_spans_working_and_online_idle_only() {
        let view = fold_fleet(&FleetListOut {
            agents: vec![
                row("w", "r", Liveness::Working, None),
                row("i", "r", Liveness::OnlineIdle, None),
                row("o", "r", Liveness::Offline, None),
                row("s", "r", Liveness::Stale, None),
            ],
        });
        let live = view.filtered(FleetFilter::Live);
        assert_eq!(live.len(), 2);
        assert!(live.iter().all(|r| r.is_live()));
    }

    #[test]
    fn grouping_orders_attention_first_and_omits_empty_groups() {
        let view = fold_fleet(&FleetListOut {
            agents: vec![
                row("ok", "r", Liveness::Working, Some(StatusLabel::Done)),
                row(
                    "blocked",
                    "r",
                    Liveness::Offline,
                    Some(StatusLabel::Blocked),
                ),
            ],
        });
        let groups = view.grouped(FleetFilter::All);
        // NeedsPerson comes before Ok; there is no Failed group (empty, omitted).
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].0, AttentionGroup::NeedsPerson);
        assert_eq!(groups[1].0, AttentionGroup::Ok);
    }

    #[test]
    fn grouped_with_live_filter_excludes_offline_and_stale_agents() {
        // Working + NeedsPerson: live.  OnlineIdle + Failed: live.
        // Offline + NeedsPerson: not live.  Stale + Ok: not live.
        let view = fold_fleet(&FleetListOut {
            agents: vec![
                row("w", "r", Liveness::Working, Some(StatusLabel::Blocked)),
                row("i", "r", Liveness::OnlineIdle, Some(StatusLabel::Failed)),
                row("o", "r", Liveness::Offline, Some(StatusLabel::Blocked)),
                row("s", "r", Liveness::Stale, None),
            ],
        });
        let groups = view.grouped(FleetFilter::Live);
        let total: usize = groups.iter().map(|(_, rows)| rows.len()).sum();
        assert_eq!(
            total, 2,
            "only the two live agents appear under Live filter"
        );
        // The two live agents are in their correct attention buckets.
        assert!(groups
            .iter()
            .any(|(g, _)| *g == AttentionGroup::NeedsPerson));
        assert!(groups.iter().any(|(g, _)| *g == AttentionGroup::Failed));
        // No Ok group: the only Ok agent (Stale) was filtered out.
        assert!(!groups.iter().any(|(g, _)| *g == AttentionGroup::Ok));
    }

    #[test]
    fn grouped_with_live_filter_returns_empty_when_all_agents_are_offline() {
        let view = fold_fleet(&FleetListOut {
            agents: vec![
                row("o", "r", Liveness::Offline, None),
                row("s", "r", Liveness::Stale, None),
            ],
        });
        assert!(
            view.grouped(FleetFilter::Live).is_empty(),
            "no groups when the Live filter matches nothing"
        );
    }
}
