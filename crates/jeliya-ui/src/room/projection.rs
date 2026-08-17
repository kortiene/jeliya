//! The total, exhaustive projection from the signed timeline to render rows
//! (#179 §5, D1/D2).
//!
//! `RoomView.timeline` (position-ordered, gap-free, dedup-by-`event_id`) plus
//! the always-shown pending sends become ordered [`RenderUnit`]s, then decorated
//! with day dividers, 5-minute same-sender compacting, and a conversational
//! side. Every step is **reversible view state** (D1): filtering and folding
//! never drop a signed fact, and the honest activity total counts the unfolded,
//! unfiltered items ([`activity_total`]).
//!
//! Pure (`jeliya-api` seam types + std): the *decisions* live here (what row a
//! kind becomes, when two units group, which side a unit takes); the component
//! only renders them and feeds DOM measurements to [`crate::room::scroll`].

use jeliya_api::{Author, Event, EventKindContent, SubjectId};

use crate::room::runs::{fold_runs, passes_filter, ActivityCategory, AgentRun, Grouped};
use crate::room::send::SendEntry;

/// One projected timeline item. `Pending` is always present (never filtered), so
/// a failed send's Retry stays reachable (§5.1).
#[derive(Clone, PartialEq, Debug)]
pub enum RenderUnit {
    /// One standalone signed event.
    Event(Event),
    /// A folded same-author agent-status run.
    Run(AgentRun),
    /// A local send not yet reconciled against a committed row.
    Pending(SendEntry),
}

/// The conversational side of a unit: own (self or pending), remote (another
/// author's message/status/file/pipe), or system (syslines).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    /// Authored by the self subject, or a local pending send.
    Own,
    /// Authored by another subject (message, status, file, or pipe reference).
    Remote,
    /// A membership/lifecycle sysline; runs never group into it.
    System,
}

/// A calendar day (UTC) for the day-divider decision. The component formats it
/// (Today/Yesterday/localized date); the projection only decides the boundary.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DayKey {
    /// The calendar year.
    pub year: i32,
    /// The calendar month, `1..=12`.
    pub month: u8,
    /// The day of month, `1..=31`.
    pub day: u8,
}

/// One decorated timeline row: the unit plus its rendering decisions.
#[derive(Clone, PartialEq, Debug)]
pub struct TimelineRow {
    /// The day divider to render before this row, when a new calendar day
    /// begins here (the first row of a day, or the very first row).
    pub divider: Option<DayKey>,
    /// The conversational side.
    pub side: Side,
    /// Whether this row renders compact (avatar/meta suppressed): a message (or
    /// own-pending) within 5 minutes of the previous same-sender message on the
    /// same day. Runs and syslines never compact.
    pub compact: bool,
    /// The projected unit.
    pub unit: RenderUnit,
}

/// The honest activity total: the count of unfolded, unfiltered signed events.
/// Folding and filtering are views (D1), so neither changes this — the counter
/// and scroll accounting count *this* (§5.1, the React `buildRenderUnits`
/// invariant). Pending sends are local, not signed activity, so they are not
/// counted here.
pub fn activity_total(timeline: &[Event]) -> usize {
    timeline.len()
}

/// Build the ordered [`RenderUnit`] list from the signed `timeline`, the
/// always-shown `pending` sends, and the active view filter (§5.1):
///
/// 1. Filter the signed events by the active categories (empty = everything);
///    pending are **never** filtered.
/// 2. Fold maximal same-author `AgentStatus` runs over the filtered events.
/// 3. Merge the pending entries in by author-dated instant, stable — a pending
///    sinks to its `at` slot but never before an equal-`at` committed unit (the
///    committed row is authoritative).
pub fn build_render_units(
    timeline: &[Event],
    pending: &[SendEntry],
    active_filters: &[ActivityCategory],
) -> Vec<RenderUnit> {
    let filtered: Vec<Event> = timeline
        .iter()
        .filter(|event| passes_filter(event, active_filters))
        .cloned()
        .collect();
    let committed: Vec<RenderUnit> = fold_runs(&filtered)
        .into_iter()
        .map(|grouped| match grouped {
            Grouped::Single(event) => RenderUnit::Event(event),
            Grouped::Run(run) => RenderUnit::Run(run),
        })
        .collect();

    // Pending, stable-ordered by (at, client_id). They are the newest local
    // work, so they normally settle at the tail; the merge below still places
    // each at its `at` slot for correctness.
    let mut pend: Vec<&SendEntry> = pending.iter().collect();
    pend.sort_by(|a, b| a.at.cmp(&b.at).then(a.client_id.cmp(&b.client_id)));

    let mut out = Vec::with_capacity(committed.len() + pend.len());
    let mut pi = 0;
    for unit in committed {
        let unit_at = unit_at(&unit);
        while pi < pend.len() && pend[pi].at < unit_at {
            out.push(RenderUnit::Pending(pend[pi].clone()));
            pi += 1;
        }
        out.push(unit);
    }
    while pi < pend.len() {
        out.push(RenderUnit::Pending(pend[pi].clone()));
        pi += 1;
    }
    out
}

/// Decorate ordered units with day dividers, compacting, and sides (§5.3).
/// `self_id` resolves own vs. remote sides and the compacting sender key.
pub fn decorate(units: &[RenderUnit], self_id: Option<&SubjectId>) -> Vec<TimelineRow> {
    let mut rows: Vec<TimelineRow> = Vec::with_capacity(units.len());
    let mut prev_day: Option<DayKey> = None;
    let mut prev_sender: Option<SenderKey> = None;
    let mut prev_at: Option<i64> = None;

    for unit in units {
        let day = day_key(unit);
        // A pending send carries no signed day: its `at` is a local ordering key
        // (the newest signed instant, or the epoch fallback in a room with no
        // events yet — §6/D6), never a signed timestamp. A pending unit therefore
        // never OPENS a day divider, so an empty room's first local send cannot
        // fabricate a "1 January 1970" divider out of that epoch fallback; a
        // pending after committed content still renders under that day's divider.
        let is_pending = matches!(unit, RenderUnit::Pending(_));
        let divider = if !is_pending && prev_day != Some(day) {
            Some(day)
        } else {
            None
        };
        let at = unit_unix(unit);
        let sender = message_sender(unit, self_id);
        // Compact only a message/own-pending directly under the same sender,
        // within 5 minutes, with no day boundary between. Runs and syslines have
        // `sender = None`, so they never compact and never let a neighbour group
        // into them (the React `unitMessageSender = null` rule).
        let compact = divider.is_none()
            && sender.is_some()
            && sender == prev_sender
            && matches!((prev_at, at), (Some(p), c) if (c - p).abs() <= FIVE_MINUTES);

        rows.push(TimelineRow {
            divider,
            side: side_of(unit, self_id),
            compact,
            unit: unit.clone(),
        });

        prev_day = Some(day);
        prev_sender = sender;
        prev_at = Some(at);
    }
    rows
}

/// Five minutes, in seconds — the same-sender compacting window (§5.3).
const FIVE_MINUTES: i64 = 5 * 60;

/// The sender identity used for compacting: `Own` for a self message or any
/// pending, `Subject(id)` for another member's message, `None` for anything that
/// must not group (runs, syslines, file/pipe references, unresolved messages).
#[derive(Clone, PartialEq, Eq, Debug)]
enum SenderKey {
    Own,
    Subject(SubjectId),
}

fn message_sender(unit: &RenderUnit, self_id: Option<&SubjectId>) -> Option<SenderKey> {
    match unit {
        RenderUnit::Pending(_) => Some(SenderKey::Own),
        RenderUnit::Event(event) => match &event.kind {
            EventKindContent::Message { .. } => match &event.author {
                Author::Resolved { subject_id, .. } => {
                    if Some(subject_id) == self_id {
                        Some(SenderKey::Own)
                    } else {
                        Some(SenderKey::Subject(subject_id.clone()))
                    }
                }
                Author::Unresolved => None,
            },
            _ => None,
        },
        RenderUnit::Run(_) => None,
    }
}

fn side_of(unit: &RenderUnit, self_id: Option<&SubjectId>) -> Side {
    match unit {
        RenderUnit::Pending(_) => Side::Own,
        RenderUnit::Run(run) => author_side(&run.summary.latest.author, self_id),
        RenderUnit::Event(event) => match &event.kind {
            EventKindContent::Message { .. }
            | EventKindContent::AgentStatus { .. }
            | EventKindContent::FileShared { .. }
            | EventKindContent::PipePublished { .. } => author_side(&event.author, self_id),
            EventKindContent::RoomCreated { .. }
            | EventKindContent::MemberJoined { .. }
            | EventKindContent::MemberLeft { .. }
            | EventKindContent::MemberRemoved { .. }
            | EventKindContent::InviteRevoked { .. }
            | EventKindContent::PipeRevoked { .. } => Side::System,
        },
    }
}

fn author_side(author: &Author, self_id: Option<&SubjectId>) -> Side {
    match author {
        Author::Resolved { subject_id, .. } if Some(subject_id) == self_id => Side::Own,
        _ => Side::Remote,
    }
}

/// The representative author-dated instant a unit sorts and dividers by: an
/// event's `at`, a run's latest `at`, a pending's local `at`.
fn unit_at(unit: &RenderUnit) -> jeliya_api::Timestamp {
    match unit {
        RenderUnit::Event(event) => event.at,
        RenderUnit::Run(run) => run.summary.last_at,
        RenderUnit::Pending(entry) => entry.at,
    }
}

fn unit_unix(unit: &RenderUnit) -> i64 {
    unit_at(unit).into_inner().unix_timestamp()
}

fn day_key(unit: &RenderUnit) -> DayKey {
    let (year, month, day) = unit_at(unit).into_inner().date().to_calendar_date();
    DayKey {
        year,
        month: u8::from(month),
        day,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::room::send::{SendEntry, SendId};
    use jeliya_api::{EventId, FileId, InviteId, Progress, Role, Standing, StatusLabel, Timestamp};

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

    fn message(pos: u64, subject: &str, at: i64, body: &str) -> Event {
        Event {
            pos,
            event_id: EventId::new(format!("e{pos}")),
            at: ts(at),
            author: resolved(subject),
            kind: EventKindContent::Message { body: body.into() },
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

    fn sysline(pos: u64, at: i64) -> Event {
        Event {
            pos,
            event_id: EventId::new(format!("e{pos}")),
            at: ts(at),
            author: resolved("authority"),
            kind: EventKindContent::MemberLeft {
                subject_id: SubjectId::new("gone"),
            },
        }
    }

    fn pending(id: u64, at: i64) -> SendEntry {
        SendEntry::new(SendId(id), "draft".into(), ts(at))
    }

    #[test]
    fn activity_total_counts_unfolded_unfiltered_events() {
        let timeline = vec![
            status(0, "bot", 1),
            status(1, "bot", 2),
            message(2, "a", 3, "hi"),
        ];
        // Folding into a run and filtering must not change the honest total.
        assert_eq!(activity_total(&timeline), 3);
        let units = build_render_units(&timeline, &[], &[ActivityCategory::Conversation]);
        // Only the message survives the filter, but the honest total is still 3.
        assert_eq!(activity_total(&timeline), 3);
        assert_eq!(units.len(), 1);
    }

    #[test]
    fn pending_are_never_filtered_and_sink_to_their_at_slot() {
        let timeline = vec![message(0, "a", 10, "hi")];
        let pend = vec![pending(0, 20)];
        // A filter that excludes conversation still keeps the pending entry.
        let units = build_render_units(&timeline, &pend, &[ActivityCategory::Files]);
        assert_eq!(units.len(), 1);
        assert!(matches!(units[0], RenderUnit::Pending(_)));

        // With no filter, the pending sinks after the earlier committed message.
        let units = build_render_units(&timeline, &pend, &[]);
        assert_eq!(units.len(), 2);
        assert!(matches!(units[0], RenderUnit::Event(_)));
        assert!(matches!(units[1], RenderUnit::Pending(_)));
    }

    #[test]
    fn a_pending_before_a_committed_unit_sorts_ahead_of_it() {
        let timeline = vec![message(0, "a", 100, "later")];
        let pend = vec![pending(0, 50)];
        let units = build_render_units(&timeline, &pend, &[]);
        assert!(matches!(units[0], RenderUnit::Pending(_)));
        assert!(matches!(units[1], RenderUnit::Event(_)));
    }

    #[test]
    fn day_divider_precedes_the_first_unit_of_each_day() {
        // 1970-01-01 and 1970-01-02 (86400s apart).
        let timeline = vec![
            message(0, "a", 1, "day one"),
            message(1, "a", 86_401, "day two"),
        ];
        let units = build_render_units(&timeline, &[], &[]);
        let rows = decorate(&units, Some(&SubjectId::new("a")));
        assert_eq!(rows.len(), 2);
        assert!(rows[0].divider.is_some(), "first row always opens a day");
        assert!(
            rows[1].divider.is_some(),
            "a new calendar day opens a divider"
        );
        assert_ne!(rows[0].divider, rows[1].divider);
    }

    #[test]
    fn a_lone_pending_send_never_opens_a_fabricated_day_divider() {
        // An empty converged room with one local pending send: the composer
        // stamps it with the epoch fallback `at` (no signed event supplies a real
        // instant, and a renderer-agnostic core has no wall clock — §6/D6). The
        // pending must NOT open a day divider, or the row fabricates a
        // "1 January 1970" date out of that ordering-only epoch value.
        let pend = vec![pending(0, 0)];
        let units = build_render_units(&[], &pend, &[]);
        let rows = decorate(&units, Some(&SubjectId::new("me")));
        assert_eq!(rows.len(), 1);
        assert!(matches!(rows[0].unit, RenderUnit::Pending(_)));
        assert!(
            rows[0].divider.is_none(),
            "a pending send with no signed day must not fabricate a day divider"
        );
    }

    #[test]
    fn a_pending_after_committed_content_needs_no_divider_and_keeps_the_committed_one() {
        // A real committed message (day 1970-01-01) followed by a local pending
        // send stamped with the newest signed instant (`base_at`, as the composer
        // does): the committed message still opens the day divider; the pending,
        // sharing that day and sorting after it, opens none — no fabrication.
        let timeline = vec![message(0, "me", 100, "hello")];
        let pend = vec![pending(0, 100)];
        let units = build_render_units(&timeline, &pend, &[]);
        let rows = decorate(&units, Some(&SubjectId::new("me")));
        assert_eq!(rows.len(), 2);
        assert!(
            rows[0].divider.is_some(),
            "the committed message opens the day"
        );
        assert!(matches!(rows[1].unit, RenderUnit::Pending(_)));
        assert!(
            rows[1].divider.is_none(),
            "the pending shares the day, no new divider"
        );
    }

    #[test]
    fn five_minute_same_sender_messages_compact_but_a_gap_does_not() {
        let self_id = SubjectId::new("a");
        let timeline = vec![
            message(0, "a", 100, "first"),
            message(1, "a", 200, "within five minutes"),
            message(2, "a", 700, "more than five minutes later"),
        ];
        let units = build_render_units(&timeline, &[], &[]);
        let rows = decorate(&units, Some(&self_id));
        assert!(!rows[0].compact, "the first message is never compact");
        assert!(
            rows[1].compact,
            "a same-sender message within 5 minutes compacts"
        );
        assert!(!rows[2].compact, "a >5-minute gap breaks compacting");
    }

    #[test]
    fn a_different_sender_and_a_sysline_break_compacting() {
        let self_id = SubjectId::new("a");
        let timeline = vec![
            message(0, "a", 100, "mine"),
            message(1, "b", 150, "theirs"),
            message(2, "b", 160, "theirs again"),
            sysline(3, 170),
            message(4, "b", 175, "after the sysline"),
        ];
        let units = build_render_units(&timeline, &[], &[]);
        let rows = decorate(&units, Some(&self_id));
        assert!(!rows[1].compact, "a sender change breaks compacting");
        assert!(
            rows[2].compact,
            "the same remote sender within 5 minutes compacts"
        );
        assert!(!rows[3].compact, "a sysline never compacts");
        assert!(
            !rows[4].compact,
            "a message after a sysline does not group into it"
        );
    }

    #[test]
    fn sides_classify_own_remote_and_system() {
        let self_id = SubjectId::new("me");
        let mut timeline = vec![
            message(0, "me", 1, "mine"),
            message(1, "other", 2, "theirs"),
            sysline(2, 3),
            Event {
                pos: 3,
                event_id: EventId::new("e3"),
                at: ts(4),
                author: resolved("other"),
                kind: EventKindContent::FileShared {
                    file_id: FileId::new("f"),
                    name: "n".into(),
                    bytes: 1,
                    digest: "d".into(),
                },
            },
        ];
        timeline.push(Event {
            pos: 4,
            event_id: EventId::new("e4"),
            at: ts(5),
            author: Author::Unresolved,
            kind: EventKindContent::InviteRevoked {
                invite_id: InviteId::new("i"),
            },
        });
        let units = build_render_units(&timeline, &[pending(0, 6)], &[]);
        let rows = decorate(&units, Some(&self_id));
        // message(me) -> Own, message(other) -> Remote, sysline -> System,
        // file(other) -> Remote, invite sysline -> System, pending -> Own.
        assert_eq!(rows[0].side, Side::Own);
        assert_eq!(rows[1].side, Side::Remote);
        assert_eq!(rows[2].side, Side::System);
        assert_eq!(rows[3].side, Side::Remote);
        assert_eq!(rows[4].side, Side::System);
        assert_eq!(rows[5].side, Side::Own);
        assert!(matches!(rows[5].unit, RenderUnit::Pending(_)));
    }

    #[test]
    fn an_unresolved_message_never_compacts() {
        let timeline = vec![
            Event {
                pos: 0,
                event_id: EventId::new("e0"),
                at: ts(100),
                author: Author::Unresolved,
                kind: EventKindContent::Message { body: "a".into() },
            },
            Event {
                pos: 1,
                event_id: EventId::new("e1"),
                at: ts(120),
                author: Author::Unresolved,
                kind: EventKindContent::Message { body: "b".into() },
            },
        ];
        let units = build_render_units(&timeline, &[], &[]);
        let rows = decorate(&units, None);
        // Nothing is asserted about an unresolved author, so two unresolved
        // messages must not be claimed to share a sender.
        assert!(!rows[1].compact);
    }

    #[test]
    fn a_day_boundary_breaks_compacting_even_within_five_minutes() {
        let self_id = SubjectId::new("a");
        // 86399 = 23:59:59 UTC on 1970-01-01; 86400 = 00:00:00 on 1970-01-02.
        // Both are from the same sender and only 1 second apart (well within 5
        // minutes), but the calendar day turns between them.
        let timeline = vec![
            message(0, "a", 86_399, "end of day one"),
            message(1, "a", 86_400, "start of day two"),
        ];
        let units = build_render_units(&timeline, &[], &[]);
        let rows = decorate(&units, Some(&self_id));
        assert_eq!(rows.len(), 2);
        assert!(!rows[0].compact, "the first row never compacts");
        assert!(
            rows[1].divider.is_some(),
            "a new calendar day must open a divider"
        );
        assert!(
            !rows[1].compact,
            "a day divider prevents compacting even within five minutes"
        );
    }

    #[test]
    fn equal_at_pending_entries_are_sorted_stably_by_client_id() {
        // Two pending entries at the same instant: client_id order must be stable.
        let pend = vec![pending(1, 50), pending(0, 50)];
        let units = build_render_units(&[], &pend, &[]);
        // client_id 0 should appear before client_id 1.
        match (&units[0], &units[1]) {
            (RenderUnit::Pending(a), RenderUnit::Pending(b)) => {
                assert!(
                    a.client_id < b.client_id,
                    "equal-at pendins are sorted by client_id"
                );
            }
            other => panic!("expected two Pending units, got {other:?}"),
        }
    }
}
