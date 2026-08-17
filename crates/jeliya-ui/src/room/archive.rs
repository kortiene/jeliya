//! The read-only archive projection (#91 §5): the pure fold from paged
//! `room.archive` replies into the [`ArchiveView`] a departed room renders.
//!
//! Pure — no Dioxus, no transport — so the projection is host-tested without a
//! renderer, exactly like `shell/` and `prefs/`. The Dioxus surface
//! ([`crate::components::room_archive`]) is a thin RSX view over this.

use jeliya_api::{
    Cursor, Event, EventKindContent, RoomArchiveOut, RoomId, Standing, SubjectId, Truncated,
};

use crate::room::roster::{self, HistoricalMember};

/// The page size used for `room.archive` reads.
///
/// `room.archive` shares the `1..=timeline_page_max` paging bound of the other
/// paging ops (`docs/protocol-v2.md`; the daemon serves `timeline_page_max`,
/// currently 1024, and **refuses — never clamps** a larger `limit`). This is a
/// conservative fixed page well within that bound; once the `ClientHandle` seam
/// surfaces the served `timeline_page_max` (with #171/#270) the read can adopt
/// it and route through #179's bounded-read ceilings (#91 R3). Kept a named
/// constant so that swap is one edit.
pub const ARCHIVE_PAGE_LIMIT: u64 = 100;

/// The signed departure fact the banner states (#91 D4). Derived from the
/// caller's own signed `member_left`/`member_removed` event when it is in the
/// loaded window, and otherwise from the authoritative
/// [`RoomArchiveOut::standing`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DepartureFact {
    /// The caller authored a `member_left` — a voluntary departure.
    Left,
    /// An authority authored the caller's `member_removed`. `by` names the
    /// remover **only** when that signed event is loaded and the caller is
    /// identifiable; otherwise it is `None` — the banner states "removed"
    /// without guessing a remover (#91 R5).
    Removed {
        /// The subject who removed the caller, when known.
        by: Option<SubjectId>,
    },
}

impl DepartureFact {
    /// The departure fact implied by the caller's authoritative ended standing
    /// alone — the fallback when no naming signed event is loaded. A defensive
    /// `Active` (which never reaches an archive; the room_still_active race is
    /// handled before folding, #91 D8) maps to `Left`.
    pub fn from_standing(standing: Standing) -> Self {
        match standing {
            Standing::Removed => DepartureFact::Removed { by: None },
            Standing::Left | Standing::Active => DepartureFact::Left,
        }
    }
}

/// The read-only view a departed room renders (#91 §5). Pure; no Dioxus, no
/// transport. Accumulated across the forward paging reads via [`ArchiveView::fold`].
#[derive(Debug, Clone, PartialEq)]
pub struct ArchiveView {
    /// The room.
    pub room_id: RoomId,
    /// The room's name, from a loaded `room_created` event, else `None`.
    pub room_name: Option<String>,
    /// The caller's own ended standing — authoritative, from `RoomArchiveOut`.
    pub my_standing: Standing,
    /// The signed departure fact (#91 D4).
    pub departure: DepartureFact,
    /// The signed timeline, oldest→newest, deduplicated by position.
    pub events: Vec<Event>,
    /// The historical roster folded from the loaded membership events (#91 D3).
    pub roster: Vec<HistoricalMember>,
    /// The continuation cursor when more events remain (`Truncated::More`), else
    /// `None`.
    pub more: Option<Cursor>,
}

impl ArchiveView {
    /// Merge a freshly read page into the accumulated view.
    ///
    /// - Appends `out.events` in position order, **idempotent by `pos`** — a
    ///   re-read page never double-appends, so a retry after a transient failure
    ///   is safe even though the read itself is network-free.
    /// - Recomputes `room_name` and the historical `roster` from the full sorted
    ///   event set, so the fold is order-independent across pages (#91 D3).
    /// - Takes `my_standing` from the authoritative `out.standing`.
    /// - Derives `departure` from the caller's own signed event when
    ///   identifiable (`me`), falling back to `out.standing` (#91 D4 / R5).
    /// - Records `more` from `out.truncated`.
    ///
    /// `me` is the caller's subject when the seam can name it (from `Hello`);
    /// `None` is the honest default while that is not surfaced (#178 D2/R1), and
    /// then a `removed` banner simply does not name the remover.
    pub fn fold(
        prev: Option<ArchiveView>,
        out: RoomArchiveOut,
        me: Option<&SubjectId>,
    ) -> ArchiveView {
        let mut view = prev.unwrap_or_else(|| ArchiveView {
            room_id: out.room_id.clone(),
            room_name: None,
            my_standing: out.standing,
            departure: DepartureFact::from_standing(out.standing),
            events: Vec::new(),
            roster: Vec::new(),
            more: None,
        });

        // `out.standing` is the daemon's authoritative answer on every page.
        view.my_standing = out.standing;

        // Append only positions not already accumulated (idempotent by `pos`).
        for event in out.events {
            if view.events.iter().any(|existing| existing.pos == event.pos) {
                continue;
            }
            view.events.push(event);
        }
        // Keep the timeline in position order regardless of the order pages
        // arrived; the roster/name recompute below then reads events in-order.
        view.events.sort_by_key(|event| event.pos);

        // Recompute the derived facts from the full ordered event set so the
        // result is independent of page-arrival order.
        view.room_name = view.events.iter().find_map(|event| match &event.kind {
            EventKindContent::RoomCreated { name } => Some(name.clone()),
            _ => None,
        });
        view.roster = Vec::new();
        for event in &view.events {
            roster::apply(&mut view.roster, event);
        }

        view.departure = derive_departure(&view.events, out.standing, me);
        view.more = match out.truncated {
            Truncated::Complete => None,
            Truncated::More { cursor } => Some(cursor),
        };
        view
    }
}

/// Derive the departure fact from the caller's own signed event when loaded and
/// identifiable, else from the authoritative standing (#91 D4 / R5).
fn derive_departure(events: &[Event], standing: Standing, me: Option<&SubjectId>) -> DepartureFact {
    match standing {
        Standing::Removed => {
            // Name the remover only from the caller's OWN signed removal — never
            // from another member's removal. The last such event wins.
            let by = me.and_then(|me| {
                events.iter().rev().find_map(|event| match &event.kind {
                    EventKindContent::MemberRemoved { subject_id, by } if subject_id == me => {
                        Some(by.clone())
                    }
                    _ => None,
                })
            });
            DepartureFact::Removed { by }
        }
        // Voluntary departure carries no remover.
        Standing::Left => DepartureFact::Left,
        // Defensive: an active room never reaches the archive fold (#91 D8).
        Standing::Active => DepartureFact::Left,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jeliya_api::{Author, EventId, Role, Timestamp};

    fn event(pos: u64, author: Author, kind: EventKindContent) -> Event {
        Event {
            pos,
            event_id: EventId::new(format!("e-{pos}")),
            at: Timestamp::new(time::OffsetDateTime::UNIX_EPOCH),
            author,
            kind,
        }
    }

    fn out(standing: Standing, events: Vec<Event>, truncated: Truncated) -> RoomArchiveOut {
        RoomArchiveOut {
            room_id: RoomId::new("blake3:room"),
            standing,
            events,
            truncated,
        }
    }

    fn genesis() -> Event {
        event(
            0,
            Author::Resolved {
                subject_id: SubjectId::new("blake3:creator"),
                role: Role::Authority,
                standing: Standing::Active,
            },
            EventKindContent::RoomCreated { name: "Ops".into() },
        )
    }

    #[test]
    fn a_first_page_seeds_name_roster_and_more() {
        let events = vec![
            genesis(),
            event(
                1,
                Author::Unresolved,
                EventKindContent::MemberJoined {
                    subject_id: SubjectId::new("blake3:me"),
                    role: Role::Member,
                },
            ),
        ];
        let view = ArchiveView::fold(
            None,
            out(
                Standing::Left,
                events,
                Truncated::More {
                    cursor: Cursor::At { pos: 2 },
                },
            ),
            Some(&SubjectId::new("blake3:me")),
        );
        assert_eq!(view.room_name.as_deref(), Some("Ops"));
        assert_eq!(view.my_standing, Standing::Left);
        assert_eq!(view.departure, DepartureFact::Left);
        assert_eq!(view.roster.len(), 2);
        assert_eq!(view.more, Some(Cursor::At { pos: 2 }));
    }

    #[test]
    fn folding_a_re_read_page_is_idempotent_by_position() {
        let page = || {
            out(
                Standing::Left,
                vec![genesis()],
                Truncated::More {
                    cursor: Cursor::At { pos: 1 },
                },
            )
        };
        let first = ArchiveView::fold(None, page(), None);
        // Re-read the SAME page (same positions): no double-append.
        let again = ArchiveView::fold(Some(first.clone()), page(), None);
        assert_eq!(again.events.len(), 1);
        assert_eq!(first.events.len(), again.events.len());
    }

    #[test]
    fn a_second_page_appends_and_tracks_completion() {
        let first = ArchiveView::fold(
            None,
            out(
                Standing::Left,
                vec![genesis()],
                Truncated::More {
                    cursor: Cursor::At { pos: 1 },
                },
            ),
            None,
        );
        let merged = ArchiveView::fold(
            Some(first),
            out(
                Standing::Left,
                vec![event(
                    1,
                    Author::Unresolved,
                    EventKindContent::Message { body: "hi".into() },
                )],
                Truncated::Complete,
            ),
            None,
        );
        assert_eq!(merged.events.len(), 2);
        assert_eq!(merged.events[0].pos, 0);
        assert_eq!(merged.events[1].pos, 1);
        assert_eq!(merged.more, None);
    }

    #[test]
    fn removed_reads_the_remover_from_the_callers_own_event() {
        let me = SubjectId::new("blake3:me");
        let view = ArchiveView::fold(
            None,
            out(
                Standing::Removed,
                vec![
                    genesis(),
                    event(
                        3,
                        Author::Unresolved,
                        EventKindContent::MemberRemoved {
                            subject_id: me.clone(),
                            by: SubjectId::new("blake3:creator"),
                        },
                    ),
                ],
                Truncated::Complete,
            ),
            Some(&me),
        );
        assert_eq!(
            view.departure,
            DepartureFact::Removed {
                by: Some(SubjectId::new("blake3:creator"))
            }
        );
    }

    #[test]
    fn removed_without_the_signed_event_falls_back_to_standing_and_names_no_one() {
        // The caller's own removal is outside the loaded window (only genesis
        // loaded): standing is authoritative, remover unknown (#91 R5).
        let view = ArchiveView::fold(
            None,
            out(Standing::Removed, vec![genesis()], Truncated::Complete),
            Some(&SubjectId::new("blake3:me")),
        );
        assert_eq!(view.departure, DepartureFact::Removed { by: None });
    }

    #[test]
    fn removed_with_no_known_caller_names_no_remover() {
        // `me` unknown (seam gap): even with a MemberRemoved loaded, no remover
        // is asserted because we cannot tell it is the caller's own.
        let view = ArchiveView::fold(
            None,
            out(
                Standing::Removed,
                vec![
                    genesis(),
                    event(
                        3,
                        Author::Unresolved,
                        EventKindContent::MemberRemoved {
                            subject_id: SubjectId::new("blake3:other"),
                            by: SubjectId::new("blake3:creator"),
                        },
                    ),
                ],
                Truncated::Complete,
            ),
            None,
        );
        assert_eq!(view.departure, DepartureFact::Removed { by: None });
    }

    #[test]
    fn from_standing_all_variants_map_correctly() {
        // Left and Removed are the expected archive standings; Active is the
        // defensive path that never happens in practice (#91 D8) but must be
        // non-panicking and honest.
        assert_eq!(
            DepartureFact::from_standing(Standing::Left),
            DepartureFact::Left
        );
        assert_eq!(
            DepartureFact::from_standing(Standing::Removed),
            DepartureFact::Removed { by: None }
        );
        // The Active→Left defensive path: an active room never reaches the
        // archive fold (the room_still_active race is resolved in the pane
        // before folding), but the fallback must not panic.
        assert_eq!(
            DepartureFact::from_standing(Standing::Active),
            DepartureFact::Left
        );
    }

    #[test]
    fn another_members_removal_does_not_name_the_callers_remover() {
        // The departure fact for the caller (standing=Removed) must NOT pick
        // up a remover from a MemberRemoved event for a DIFFERENT subject.
        // Only the caller's OWN signed removal event names the remover (#91
        // §5 / R5). "me" is known but the loaded removal is for someone else.
        let me = SubjectId::new("blake3:me");
        let view = ArchiveView::fold(
            None,
            out(
                Standing::Removed,
                vec![
                    genesis(),
                    event(
                        1,
                        Author::Unresolved,
                        EventKindContent::MemberRemoved {
                            subject_id: SubjectId::new("blake3:someone_else"),
                            by: SubjectId::new("blake3:creator"),
                        },
                    ),
                ],
                Truncated::Complete,
            ),
            Some(&me),
        );
        // The caller's own removal event is not loaded — the standing fallback
        // applies, naming no remover.
        assert_eq!(view.departure, DepartureFact::Removed { by: None });
    }

    #[test]
    fn room_name_is_none_when_no_genesis_event_is_loaded() {
        // A page of only message events carries no RoomCreated, so room_name
        // stays None — the honest default for any page that does not contain
        // the genesis event (#91 §5 / fold contract).
        let view = ArchiveView::fold(
            None,
            out(
                Standing::Left,
                vec![event(
                    0,
                    Author::Unresolved,
                    EventKindContent::Message { body: "hi".into() },
                )],
                Truncated::Complete,
            ),
            None,
        );
        assert_eq!(view.room_name, None);
    }

    #[test]
    fn events_from_pages_are_sorted_into_position_order() {
        // The fold sorts events by `pos` regardless of the order pages
        // arrive, so the caller always sees oldest→newest (#91 §5: "position
        // order"). Simulate a late-arriving second page that contains earlier
        // positions (edge case: overlapping cursor reads).
        let first = ArchiveView::fold(
            None,
            out(
                Standing::Left,
                vec![
                    event(
                        2,
                        Author::Unresolved,
                        EventKindContent::Message { body: "b".into() },
                    ),
                    event(
                        4,
                        Author::Unresolved,
                        EventKindContent::Message { body: "d".into() },
                    ),
                ],
                Truncated::More {
                    cursor: Cursor::At { pos: 5 },
                },
            ),
            None,
        );
        let merged = ArchiveView::fold(
            Some(first),
            out(
                Standing::Left,
                vec![
                    event(
                        0,
                        Author::Unresolved,
                        EventKindContent::Message { body: "a".into() },
                    ),
                    event(
                        3,
                        Author::Unresolved,
                        EventKindContent::Message { body: "c".into() },
                    ),
                ],
                Truncated::Complete,
            ),
            None,
        );
        assert_eq!(merged.events.len(), 4);
        assert!(
            merged.events.windows(2).all(|w| w[0].pos < w[1].pos),
            "events must be sorted by pos after merging out-of-order pages"
        );
        assert_eq!(merged.more, None);
    }
}
