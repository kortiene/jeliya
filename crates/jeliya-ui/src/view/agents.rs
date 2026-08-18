//! The room-scoped Agents & Runs fold (§7.2, D9): the agents *in one room* are
//! `fleet.list` filtered by `room_id` (agent-ness is a single derived fact, not
//! a second op), and a selected agent's run history is `status.history`.

use jeliya_api::{
    FleetListOut, Progress, RoomId, Severity, StatusHistoryOut, StatusLabel, SubjectId, Timestamp,
    Truncated,
};

use super::fleet::{fold_fleet, FleetRowView};

/// The agents in one room, reusing the fleet row view (D9/D10).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AgentsView {
    /// The agent rows scoped to the open room.
    pub rows: Vec<FleetRowView>,
}

impl AgentsView {
    /// Whether no agents are in this room yet (a real empty answer).
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// Filter `fleet.list` down to the agents in `room_id`.
pub fn fold_room_agents(fleet: &FleetListOut, room_id: &RoomId) -> AgentsView {
    AgentsView {
        rows: fold_fleet(fleet)
            .rows
            .into_iter()
            .filter(|row| &row.room_id == room_id)
            .collect(),
    }
}

/// The subjects classified as agents in `room_id` — the derived agent marker the
/// People roster reads (D9). One `fleet.list` per room open serves both People
/// (the marker) and Agents (the list).
pub fn agent_subjects_in_room(fleet: &FleetListOut, room_id: &RoomId) -> Vec<SubjectId> {
    fleet
        .agents
        .iter()
        .filter(|row| &row.room_id == room_id)
        .map(|row| row.subject_id.clone())
        .collect()
}

/// One run-history entry.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RunEntryView {
    /// When it was posted.
    pub at: Timestamp,
    /// Its status label.
    pub label: StatusLabel,
    /// Its served severity.
    pub severity: Severity,
    /// Its progress.
    pub progress: Progress,
}

/// The folded, bounded run history for one agent.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RunHistoryView {
    /// The entries, newest-first order as returned (chronological within the
    /// bounded page).
    pub entries: Vec<RunEntryView>,
    /// Whether the bounded first page left more entries unread.
    pub has_more: bool,
}

impl RunHistoryView {
    /// Whether the agent has posted nothing (a real empty answer).
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Fold `status.history` into a bounded [`RunHistoryView`].
pub fn fold_run_history(history: &StatusHistoryOut) -> RunHistoryView {
    RunHistoryView {
        entries: history
            .entries
            .iter()
            .map(|entry| RunEntryView {
                at: entry.at,
                label: entry.label,
                severity: entry.severity,
                progress: entry.progress.clone(),
            })
            .collect(),
        has_more: matches!(history.truncated, Truncated::More { .. }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jeliya_api::{FleetRow, LastSeen, LatestStatus, Liveness};
    use time::OffsetDateTime;

    fn ts() -> Timestamp {
        Timestamp::new(OffsetDateTime::UNIX_EPOCH)
    }

    fn fleet_row(subject: &str, room: &str) -> FleetRow {
        FleetRow {
            subject_id: SubjectId::new(subject),
            room_id: RoomId::new(room),
            liveness: Liveness::Working,
            latest_status: LatestStatus::Absent,
            last_seen: LastSeen::Absent,
        }
    }

    #[test]
    fn room_agents_are_the_fleet_filtered_by_room() {
        let fleet = FleetListOut {
            agents: vec![
                fleet_row("a", "r-1"),
                fleet_row("b", "r-2"),
                fleet_row("c", "r-1"),
            ],
        };
        let view = fold_room_agents(&fleet, &RoomId::new("r-1"));
        assert_eq!(view.rows.len(), 2);
        let subjects = agent_subjects_in_room(&fleet, &RoomId::new("r-1"));
        assert_eq!(subjects, vec![SubjectId::new("a"), SubjectId::new("c")]);
    }

    #[test]
    fn run_history_carries_severity_and_progress_and_more() {
        let history = StatusHistoryOut {
            room_id: RoomId::new("r-1"),
            subject_id: SubjectId::new("a"),
            entries: vec![jeliya_api::StatusEntry {
                at: ts(),
                label: StatusLabel::Working,
                severity: Severity::Ok,
                progress: Progress::Reported { percent: 42 },
            }],
            truncated: Truncated::More {
                cursor: jeliya_api::Cursor::At { pos: 1 },
            },
        };
        let view = fold_run_history(&history);
        assert_eq!(view.entries.len(), 1);
        assert_eq!(view.entries[0].progress, Progress::Reported { percent: 42 });
        assert!(view.has_more);
    }
}
