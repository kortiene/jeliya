//! The historical-roster fold (#91 D3): reconstruct a room's membership from
//! the signed events an archive loaded, **not** from `room.members`.
//!
//! The protocol's validation order refuses every room-scoped op with
//! `membership_ended` for a departed caller **except** `room.archive` and
//! `room.list` (`docs/protocol-v2.md` step 5), so a departed archive has no
//! `room.members` to read. The roster is therefore folded from the signed
//! membership events carried in `RoomArchiveOut.events` — authoritative signed
//! data, honestly **historical**, never presented as current. It carries **no**
//! presence and **no** reachability: those are live facts a departed archive
//! has no basis to assert.

use jeliya_api::{Author, Event, EventKindContent, Role, Standing, SubjectId};

/// One member of the historical roster, as of the loaded log. Carries a role
/// and an **ended-or-active-at-departure** standing — and deliberately nothing
/// live (no presence, no reachability).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoricalMember {
    /// The member's subject.
    pub subject_id: SubjectId,
    /// The role they last held in the loaded log.
    pub role: Role,
    /// Their standing as of the loaded log (`active` / `left` / `removed`).
    pub standing: Standing,
}

/// Fold one signed event into the historical roster (D3). The four
/// membership-bearing kinds refine the roster; every other kind leaves it
/// untouched:
///
/// - `RoomCreated` seeds the room's authority from the event's signed author.
/// - `MemberJoined{subject_id, role}` adds/updates the member at `active`.
/// - `MemberLeft{subject_id}` sets that subject's standing to `left`.
/// - `MemberRemoved{subject_id, by}` sets that subject's standing to `removed`.
///
/// Applying events in position order (as a forward-from-`Start` read yields
/// them) means a member's `MemberJoined` is always seen before their departure,
/// so a departure always finds the member to update. `apply` is nonetheless
/// defensive: a departure for an as-yet-unseen subject still records the signed
/// fact rather than dropping it.
pub fn apply(roster: &mut Vec<HistoricalMember>, event: &Event) {
    match &event.kind {
        EventKindContent::RoomCreated { .. } => {
            // The creator is the authority; take their subject and role from the
            // signed author. An unresolved author asserts nothing (no fabricated
            // membership), matching `Author::Unresolved`.
            if let Author::Resolved {
                subject_id, role, ..
            } = &event.author
            {
                upsert(roster, subject_id, *role, Standing::Active);
            }
        }
        EventKindContent::MemberJoined { subject_id, role } => {
            upsert(roster, subject_id, *role, Standing::Active);
        }
        EventKindContent::MemberLeft { subject_id } => {
            set_standing(roster, subject_id, Standing::Left);
        }
        EventKindContent::MemberRemoved { subject_id, .. } => {
            set_standing(roster, subject_id, Standing::Removed);
        }
        // No other signed kind changes membership.
        _ => {}
    }
}

/// The room authority's subject, read from the signed author of the
/// `RoomCreated` event, or `None` when no genesis event is loaded or its author
/// is unresolved. Never inferred from anything but the signed genesis.
pub fn authority_of(events: &[Event]) -> Option<SubjectId> {
    events.iter().find_map(|event| match &event.kind {
        EventKindContent::RoomCreated { .. } => match &event.author {
            Author::Resolved { subject_id, .. } => Some(subject_id.clone()),
            Author::Unresolved => None,
        },
        _ => None,
    })
}

/// Add the member, or update their role and standing if already present. The
/// last signed event wins, which is correct for an in-order fold (a rejoin
/// after a departure re-activates the member exactly at that event).
fn upsert(
    roster: &mut Vec<HistoricalMember>,
    subject_id: &SubjectId,
    role: Role,
    standing: Standing,
) {
    if let Some(member) = roster.iter_mut().find(|m| &m.subject_id == subject_id) {
        member.role = role;
        member.standing = standing;
    } else {
        roster.push(HistoricalMember {
            subject_id: subject_id.clone(),
            role,
            standing,
        });
    }
}

/// Set a member's ended standing. If the member is not yet in the roster (their
/// join is outside the loaded window), record the signed departure anyway with
/// the ordinary `member` role rather than dropping an authoritative fact.
fn set_standing(roster: &mut Vec<HistoricalMember>, subject_id: &SubjectId, standing: Standing) {
    if let Some(member) = roster.iter_mut().find(|m| &m.subject_id == subject_id) {
        member.standing = standing;
    } else {
        roster.push(HistoricalMember {
            subject_id: subject_id.clone(),
            role: Role::Member,
            standing,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jeliya_api::{EventId, Timestamp};

    fn event(pos: u64, author: Author, kind: EventKindContent) -> Event {
        Event {
            pos,
            event_id: EventId::new(format!("e-{pos}")),
            at: Timestamp::new(time::OffsetDateTime::UNIX_EPOCH),
            author,
            kind,
        }
    }

    fn resolved(subject: &str, role: Role, standing: Standing) -> Author {
        Author::Resolved {
            subject_id: SubjectId::new(subject),
            role,
            standing,
        }
    }

    #[test]
    fn genesis_join_leave_remove_fold_to_the_right_standings() {
        let creator = resolved("blake3:creator", Role::Authority, Standing::Active);
        let events = [
            event(
                0,
                creator,
                EventKindContent::RoomCreated { name: "R".into() },
            ),
            event(
                1,
                Author::Unresolved,
                EventKindContent::MemberJoined {
                    subject_id: SubjectId::new("blake3:alice"),
                    role: Role::Member,
                },
            ),
            event(
                2,
                Author::Unresolved,
                EventKindContent::MemberJoined {
                    subject_id: SubjectId::new("blake3:bob"),
                    role: Role::Member,
                },
            ),
            event(
                3,
                Author::Unresolved,
                EventKindContent::MemberLeft {
                    subject_id: SubjectId::new("blake3:alice"),
                },
            ),
            event(
                4,
                Author::Unresolved,
                EventKindContent::MemberRemoved {
                    subject_id: SubjectId::new("blake3:bob"),
                    by: SubjectId::new("blake3:creator"),
                },
            ),
        ];
        let mut roster = Vec::new();
        for e in &events {
            apply(&mut roster, e);
        }
        assert_eq!(
            roster,
            vec![
                HistoricalMember {
                    subject_id: SubjectId::new("blake3:creator"),
                    role: Role::Authority,
                    standing: Standing::Active,
                },
                HistoricalMember {
                    subject_id: SubjectId::new("blake3:alice"),
                    role: Role::Member,
                    standing: Standing::Left,
                },
                HistoricalMember {
                    subject_id: SubjectId::new("blake3:bob"),
                    role: Role::Member,
                    standing: Standing::Removed,
                },
            ]
        );
    }

    #[test]
    fn authority_comes_from_the_signed_genesis_author_only() {
        let creator = resolved("blake3:creator", Role::Authority, Standing::Active);
        let with_author = [event(
            0,
            creator,
            EventKindContent::RoomCreated { name: "R".into() },
        )];
        assert_eq!(
            authority_of(&with_author),
            Some(SubjectId::new("blake3:creator"))
        );

        // An unresolved genesis author asserts no authority.
        let unresolved = [event(
            0,
            Author::Unresolved,
            EventKindContent::RoomCreated { name: "R".into() },
        )];
        assert_eq!(authority_of(&unresolved), None);

        // No genesis loaded → no authority.
        let none = [event(
            7,
            Author::Unresolved,
            EventKindContent::MemberLeft {
                subject_id: SubjectId::new("blake3:alice"),
            },
        )];
        assert_eq!(authority_of(&none), None);
    }

    #[test]
    fn a_rejoin_after_a_departure_reactivates_the_member() {
        let alice = SubjectId::new("blake3:alice");
        let events = [
            event(
                0,
                Author::Unresolved,
                EventKindContent::MemberJoined {
                    subject_id: alice.clone(),
                    role: Role::Member,
                },
            ),
            event(
                1,
                Author::Unresolved,
                EventKindContent::MemberLeft {
                    subject_id: alice.clone(),
                },
            ),
            event(
                2,
                Author::Unresolved,
                EventKindContent::MemberJoined {
                    subject_id: alice.clone(),
                    role: Role::Member,
                },
            ),
        ];
        let mut roster = Vec::new();
        for e in &events {
            apply(&mut roster, e);
        }
        assert_eq!(roster.len(), 1);
        assert_eq!(roster[0].standing, Standing::Active);
    }

    #[test]
    fn a_departure_for_an_unseen_subject_records_the_signed_fact_defensively() {
        // Spec (#91 §roster.rs): "a departure for an as-yet-unseen subject
        // still records the signed fact rather than dropping it." When
        // MemberLeft arrives before MemberJoined (the join is outside the
        // loaded window), the roster records the departure with Role::Member.
        let unknown = SubjectId::new("blake3:unknown");
        let mut roster = Vec::new();
        apply(
            &mut roster,
            &event(
                5,
                Author::Unresolved,
                EventKindContent::MemberLeft {
                    subject_id: unknown.clone(),
                },
            ),
        );
        assert_eq!(roster.len(), 1);
        assert_eq!(roster[0].subject_id, unknown);
        assert_eq!(roster[0].role, Role::Member);
        assert_eq!(roster[0].standing, Standing::Left);
    }

    #[test]
    fn a_removal_for_an_unseen_subject_records_the_signed_removal_defensively() {
        // Same guarantee for MemberRemoved: the signed fact is kept even
        // without a prior join in the loaded window.
        let unknown = SubjectId::new("blake3:unknown");
        let mut roster = Vec::new();
        apply(
            &mut roster,
            &event(
                3,
                Author::Unresolved,
                EventKindContent::MemberRemoved {
                    subject_id: unknown.clone(),
                    by: SubjectId::new("blake3:creator"),
                },
            ),
        );
        assert_eq!(roster.len(), 1);
        assert_eq!(roster[0].subject_id, unknown);
        assert_eq!(roster[0].standing, Standing::Removed);
    }

    #[test]
    fn non_membership_events_leave_roster_unchanged() {
        // Message events (and other non-membership kinds) must not add
        // entries or change standings — only the four membership-bearing
        // kinds affect the roster (#91 D3 / `apply` contract).
        let alice = SubjectId::new("blake3:alice");
        let events = [
            event(
                0,
                Author::Unresolved,
                EventKindContent::MemberJoined {
                    subject_id: alice.clone(),
                    role: Role::Member,
                },
            ),
            event(
                1,
                Author::Unresolved,
                EventKindContent::Message {
                    body: "hello".into(),
                },
            ),
            event(
                2,
                Author::Unresolved,
                EventKindContent::Message {
                    body: "world".into(),
                },
            ),
        ];
        let mut roster = Vec::new();
        for e in &events {
            apply(&mut roster, e);
        }
        assert_eq!(
            roster.len(),
            1,
            "only the MemberJoined contributes a roster entry"
        );
        assert_eq!(
            roster[0].standing,
            Standing::Active,
            "Message events must not change standings"
        );
    }
}
