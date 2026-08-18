//! Agent-status run folding and the view-only activity filters (#179 §5.1, D1).
//!
//! Both are **reversible view state** layered on top of the signed timeline
//! (D1): folding a maximal same-author `agent_status` run into one card hides
//! nothing permanently (expanding restores every event), and the five activity
//! filters are a multi-select over categories (empty = everything) that never
//! deletes a signed fact. The honest activity total is always the unfolded,
//! unfiltered event count, so neither operation can rewrite it.
//!
//! Pure (`jeliya-api` + std): no Dioxus, no `web-sys`, no `cfg`.

use jeliya_api::{Author, Event, EventKindContent, SubjectId, Timestamp};

/// The five view-only activity categories (contract §"Recency" order). Declared
/// once as a closed set, so a new [`EventKindContent`] kind must be classified
/// (compile-enforced through [`category_of`]).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ActivityCategory {
    /// `Message`.
    Conversation,
    /// `AgentStatus`.
    AgentRuns,
    /// `RoomCreated`, `MemberJoined`, `MemberLeft`, `MemberRemoved`,
    /// `InviteRevoked`.
    Membership,
    /// `FileShared`.
    Files,
    /// `PipePublished`, `PipeRevoked`.
    Pipes,
}

/// Every category, in the contract's presentation order — the filter chips.
pub const ACTIVITY_CATEGORIES: [ActivityCategory; 5] = [
    ActivityCategory::Conversation,
    ActivityCategory::AgentRuns,
    ActivityCategory::Membership,
    ActivityCategory::Files,
    ActivityCategory::Pipes,
];

/// The category a signed kind belongs to. Total over the closed 10-kind set:
/// adding an 11th kind will not compile until it is classified (D2).
pub fn category_of(kind: &EventKindContent) -> ActivityCategory {
    match kind {
        EventKindContent::Message { .. } => ActivityCategory::Conversation,
        EventKindContent::AgentStatus { .. } => ActivityCategory::AgentRuns,
        EventKindContent::RoomCreated { .. }
        | EventKindContent::MemberJoined { .. }
        | EventKindContent::MemberLeft { .. }
        | EventKindContent::MemberRemoved { .. }
        | EventKindContent::InviteRevoked { .. } => ActivityCategory::Membership,
        EventKindContent::FileShared { .. } => ActivityCategory::Files,
        EventKindContent::PipePublished { .. } | EventKindContent::PipeRevoked { .. } => {
            ActivityCategory::Pipes
        }
    }
}

/// Whether an event passes the active filter set. An empty set is "everything"
/// (the default, no filtering); otherwise the event's category must be selected.
/// Filtering is a view, never a deletion (D1).
pub fn passes_filter(event: &Event, active: &[ActivityCategory]) -> bool {
    active.is_empty() || active.contains(&category_of(&event.kind))
}

/// The subject a run folds by: an `AgentStatus` event's resolved author. An
/// `Unresolved` author asserts nothing (contract), so it can never be claimed to
/// be the same author as a neighbour — such events never fold.
fn run_author(event: &Event) -> Option<&SubjectId> {
    match (&event.kind, &event.author) {
        (EventKindContent::AgentStatus { .. }, Author::Resolved { subject_id, .. }) => {
            Some(subject_id)
        }
        _ => None,
    }
}

/// A folded run: `>= 2` consecutive `AgentStatus` events from the **same
/// resolved author**, plus a derived [`RunSummary`]. The full ordered event
/// list is retained so expanding restores every signed fact (D1).
#[derive(Clone, PartialEq, Debug)]
pub struct AgentRun {
    /// Every event in the run, in position order.
    pub events: Vec<Event>,
    /// The derived summary the collapsed card renders.
    pub summary: RunSummary,
}

/// Honest run evidence: how many status posts, over what author-dated span, and
/// the latest card. No fabricated aggregate — only what the events state.
#[derive(Clone, PartialEq, Debug)]
pub struct RunSummary {
    /// The number of status posts folded.
    pub count: usize,
    /// The earliest author-dated instant in the run.
    pub first_at: Timestamp,
    /// The latest author-dated instant in the run.
    pub last_at: Timestamp,
    /// The latest event — the card shown collapsed.
    pub latest: Event,
}

/// One item produced by run folding: either a standalone event or a folded run.
#[derive(Clone, PartialEq, Debug)]
pub enum Grouped {
    /// A standalone signed event (a non-status event, or a lone status post).
    Single(Event),
    /// `>= 2` consecutive same-author status posts.
    Run(AgentRun),
}

/// Fold maximal same-author `AgentStatus` runs over `events` (assumed in
/// position order), leaving every other event standalone. A run needs at least
/// two events with the same resolved author and no non-run event between them; a
/// lone status post stays a [`Grouped::Single`]. Nothing is dropped: the union
/// of all produced events equals `events` (D1).
pub fn fold_runs(events: &[Event]) -> Vec<Grouped> {
    let mut out = Vec::new();
    let mut index = 0;
    while index < events.len() {
        let event = &events[index];
        match run_author(event) {
            None => {
                out.push(Grouped::Single(event.clone()));
                index += 1;
            }
            Some(author) => {
                // Extend the run while the next event is a status post by the
                // same resolved author.
                let mut end = index + 1;
                while end < events.len() && run_author(&events[end]) == Some(author) {
                    end += 1;
                }
                let slice = &events[index..end];
                if slice.len() >= 2 {
                    out.push(Grouped::Run(summarize(slice)));
                } else {
                    out.push(Grouped::Single(slice[0].clone()));
                }
                index = end;
            }
        }
    }
    out
}

fn summarize(slice: &[Event]) -> AgentRun {
    let first = &slice[0];
    let latest = &slice[slice.len() - 1];
    AgentRun {
        events: slice.to_vec(),
        summary: RunSummary {
            count: slice.len(),
            first_at: first.at,
            last_at: latest.at,
            latest: latest.clone(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jeliya_api::{EventId, Progress, Role, Standing, StatusLabel};

    fn ts(secs: i64) -> Timestamp {
        Timestamp::new(time::OffsetDateTime::from_unix_timestamp(secs).expect("valid instant"))
    }

    fn resolved(subject: &str) -> Author {
        Author::Resolved {
            subject_id: SubjectId::new(subject),
            role: Role::Member,
            standing: Standing::Active,
        }
    }

    fn status(pos: u64, subject: &str, at: i64) -> Event {
        Event {
            pos,
            event_id: EventId::new(format!("e{pos}")),
            at: ts(at),
            author: resolved(subject),
            kind: EventKindContent::AgentStatus {
                label: StatusLabel::Working,
                progress: Progress::Absent,
            },
        }
    }

    fn message(pos: u64, subject: &str, at: i64) -> Event {
        Event {
            pos,
            event_id: EventId::new(format!("e{pos}")),
            at: ts(at),
            author: resolved(subject),
            kind: EventKindContent::Message { body: "hi".into() },
        }
    }

    fn unresolved_status(pos: u64, at: i64) -> Event {
        Event {
            pos,
            event_id: EventId::new(format!("e{pos}")),
            at: ts(at),
            author: Author::Unresolved,
            kind: EventKindContent::AgentStatus {
                label: StatusLabel::Working,
                progress: Progress::Absent,
            },
        }
    }

    #[test]
    fn every_kind_maps_to_exactly_one_category() {
        use ActivityCategory::*;
        let cases = [
            (EventKindContent::Message { body: "b".into() }, Conversation),
            (
                EventKindContent::AgentStatus {
                    label: StatusLabel::Idle,
                    progress: Progress::Absent,
                },
                AgentRuns,
            ),
            (
                EventKindContent::RoomCreated { name: "r".into() },
                Membership,
            ),
            (
                EventKindContent::MemberJoined {
                    subject_id: SubjectId::new("s"),
                    role: Role::Member,
                },
                Membership,
            ),
            (
                EventKindContent::MemberLeft {
                    subject_id: SubjectId::new("s"),
                },
                Membership,
            ),
            (
                EventKindContent::MemberRemoved {
                    subject_id: SubjectId::new("s"),
                    by: SubjectId::new("a"),
                },
                Membership,
            ),
            (
                EventKindContent::InviteRevoked {
                    invite_id: jeliya_api::InviteId::new("i"),
                },
                Membership,
            ),
            (
                EventKindContent::FileShared {
                    file_id: jeliya_api::FileId::new("f"),
                    name: "n".into(),
                    bytes: 1,
                    digest: "d".into(),
                },
                Files,
            ),
            (
                EventKindContent::PipePublished {
                    pipe_id: jeliya_api::PipeId::new("p"),
                    target: jeliya_api::Target {
                        host: "127.0.0.1".into(),
                        port: 1,
                    },
                    audience: jeliya_api::Audience::Room,
                },
                Pipes,
            ),
            (
                EventKindContent::PipeRevoked {
                    pipe_id: jeliya_api::PipeId::new("p"),
                },
                Pipes,
            ),
        ];
        for (kind, expected) in cases {
            assert_eq!(category_of(&kind), expected);
        }
    }

    #[test]
    fn empty_filter_is_everything_and_a_selection_narrows() {
        let msg = message(0, "a", 1);
        assert!(passes_filter(&msg, &[]));
        assert!(passes_filter(&msg, &[ActivityCategory::Conversation]));
        assert!(!passes_filter(&msg, &[ActivityCategory::Files]));
    }

    #[test]
    fn maximal_same_author_status_runs_fold_and_others_stay_single() {
        let events = vec![
            message(0, "a", 1),
            status(1, "bot", 2),
            status(2, "bot", 3),
            status(3, "bot", 4),
            message(4, "a", 5),
        ];
        let grouped = fold_runs(&events);
        assert_eq!(grouped.len(), 3);
        assert!(matches!(grouped[0], Grouped::Single(_)));
        match &grouped[1] {
            Grouped::Run(run) => {
                assert_eq!(run.summary.count, 3);
                assert_eq!(run.summary.first_at, ts(2));
                assert_eq!(run.summary.last_at, ts(4));
                assert_eq!(run.summary.latest.pos, 3);
                assert_eq!(run.events.len(), 3);
            }
            other => panic!("expected a run, got {other:?}"),
        }
        assert!(matches!(grouped[2], Grouped::Single(_)));
    }

    #[test]
    fn a_run_breaks_at_author_change_and_a_lone_status_stays_single() {
        let events = vec![status(0, "bot-a", 1), status(1, "bot-b", 2)];
        let grouped = fold_runs(&events);
        // Different authors: two singles, never a run.
        assert_eq!(grouped.len(), 2);
        assert!(matches!(grouped[0], Grouped::Single(_)));
        assert!(matches!(grouped[1], Grouped::Single(_)));

        // A lone status post is a single, not a one-element run.
        let lone = fold_runs(&[status(0, "bot", 1)]);
        assert_eq!(lone.len(), 1);
        assert!(matches!(lone[0], Grouped::Single(_)));
    }

    #[test]
    fn unresolved_status_never_folds() {
        // Two unresolved status posts assert nothing about a shared author, so
        // they must not fold into a run.
        let events = vec![unresolved_status(0, 1), unresolved_status(1, 2)];
        let grouped = fold_runs(&events);
        assert_eq!(grouped.len(), 2);
        assert!(grouped.iter().all(|g| matches!(g, Grouped::Single(_))));
    }

    #[test]
    fn folding_never_drops_a_signed_event() {
        let events = vec![
            status(0, "bot", 1),
            status(1, "bot", 2),
            message(2, "a", 3),
            status(3, "bot", 4),
        ];
        let grouped = fold_runs(&events);
        let mut seen = 0;
        for g in &grouped {
            match g {
                Grouped::Single(_) => seen += 1,
                Grouped::Run(run) => seen += run.events.len(),
            }
        }
        assert_eq!(
            seen,
            events.len(),
            "the union of grouped events must equal the input"
        );
    }

    #[test]
    fn two_consecutive_different_author_run_blocks_produce_independent_runs() {
        // bot-a for 2, immediately followed by bot-b for 2 — two separate runs.
        let events = vec![
            status(0, "bot-a", 1),
            status(1, "bot-a", 2),
            status(2, "bot-b", 3),
            status(3, "bot-b", 4),
        ];
        let grouped = fold_runs(&events);
        assert_eq!(grouped.len(), 2, "each author block is its own run");
        match &grouped[0] {
            Grouped::Run(run) => {
                assert_eq!(run.summary.count, 2);
                assert_eq!(run.summary.first_at, ts(1));
            }
            other => panic!("expected first Run, got {other:?}"),
        }
        match &grouped[1] {
            Grouped::Run(run) => {
                assert_eq!(run.summary.count, 2);
                assert_eq!(run.summary.first_at, ts(3));
            }
            other => panic!("expected second Run, got {other:?}"),
        }
    }

    #[test]
    fn a_run_interrupted_by_a_message_produces_run_single_single() {
        // Two bot statuses → Run; one message → Single; one lone bot status → Single.
        let events = vec![
            status(0, "bot", 1),
            status(1, "bot", 2),
            message(2, "a", 3),
            status(3, "bot", 4),
        ];
        let grouped = fold_runs(&events);
        assert_eq!(grouped.len(), 3);
        assert!(
            matches!(grouped[0], Grouped::Run(_)),
            "leading pair folds into a run"
        );
        assert!(
            matches!(grouped[1], Grouped::Single(_)),
            "message stays single"
        );
        assert!(
            matches!(grouped[2], Grouped::Single(_)),
            "lone trailing status stays single"
        );
    }

    #[test]
    fn activity_categories_constant_has_all_five_in_presentation_order() {
        use ActivityCategory::*;
        assert_eq!(
            ACTIVITY_CATEGORIES,
            [Conversation, AgentRuns, Membership, Files, Pipes]
        );
    }
}
