//! The People roster fold (§7.1, D1): `room.members` (signed standing + role)
//! and `room.peers` (observed links, live rooms only) rendered as **two
//! separate facts**, never blended.
//!
//! Membership comes from `room.members` and is available whether or not the room
//! is live. Presence comes from `room.peers` and exists only for a live room; a
//! Closed room yields presence **Unknown** (not zero), and a live room where a
//! member has no peer row yields presence **Absent** (a stated fact), never a
//! fabricated "online". Agent-ness is a derived marker (a member that authored a
//! status, from `fleet.list`), never a role.

use jeliya_api::{
    Link, MemberRow, Reachability, Role, RoomMembersOut, RoomPeersOut, Standing, SubjectId,
    Timestamp,
};

/// One member's presence, kept strictly separate from their membership standing.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum MemberPresence {
    /// The room session is Closed — presence is genuinely unavailable, not zero.
    Unknown,
    /// The room is live; these are this member's per-device transport links.
    /// An **empty** vector is the honest "no peer link" fact (Absent), never
    /// inferred as offline-from-membership.
    Links(Vec<Link>),
}

impl MemberPresence {
    /// Whether presence is unavailable because the session is Closed.
    pub fn is_unknown(&self) -> bool {
        matches!(self, MemberPresence::Unknown)
    }

    /// Whether the room is live but this member has no observed peer link.
    pub fn is_absent(&self) -> bool {
        matches!(self, MemberPresence::Links(links) if links.is_empty())
    }
}

/// One roster row: signed membership facts plus the separate presence fact and
/// the derived agent marker.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RosterRow {
    /// The member's subject.
    pub subject_id: SubjectId,
    /// Their signed role.
    pub role: Role,
    /// Their signed standing.
    pub standing: Standing,
    /// When they joined.
    pub joined_at: Timestamp,
    /// Derived: this member authored ≥1 status in this room (from `fleet.list`).
    /// A classification, never a role.
    pub is_agent: bool,
    /// The member's presence — a **separate** fact from standing (D1).
    pub presence: MemberPresence,
}

/// The folded People roster: the rows plus the two room-level facts (session
/// Open/Closed and, for a live room, the aggregate reachability).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RosterView {
    /// One row per member, in signed order.
    pub rows: Vec<RosterRow>,
    /// The room session fact (`RoomRow.live`): Open (`true`) / Closed (`false`).
    pub session_open: bool,
    /// The aggregate reachability from `room.peers`, present only for a live
    /// room. `None` when Closed — presence is unavailable, not "Alone".
    pub reachability: Option<Reachability>,
}

impl RosterView {
    /// Whether the roster carries no members (a real empty answer).
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// Fold the roster from its separate reads.
///
/// - `members` is the signed roster (`room.members`).
/// - `peers` is `room.peers`, supplied **only** for a live room (`None`
///   otherwise, or when the room is Closed): its presence, if any.
/// - `agent_subjects` are the subjects classified as agents in this room
///   (derived from `fleet.list`, D9).
/// - `live` is `RoomRow.live` — the authority for whether presence is knowable.
///
/// A Closed room (`live == false`) always yields `MemberPresence::Unknown` and
/// `reachability: None`, even if a stale `peers` were passed, so membership is
/// never silently promoted to presence.
pub fn fold_roster(
    members: &RoomMembersOut,
    peers: Option<&RoomPeersOut>,
    agent_subjects: &[SubjectId],
    live: bool,
) -> RosterView {
    let live_peers = if live { peers } else { None };
    let rows = members
        .members
        .iter()
        .map(|member| fold_row(member, live_peers, agent_subjects))
        .collect();
    RosterView {
        rows,
        session_open: live,
        reachability: live_peers.map(|p| p.reachability),
    }
}

fn fold_row(
    member: &MemberRow,
    live_peers: Option<&RoomPeersOut>,
    agent_subjects: &[SubjectId],
) -> RosterRow {
    let presence = match live_peers {
        // Closed / no peers read → presence unavailable (not zero).
        None => MemberPresence::Unknown,
        Some(peers) => MemberPresence::Links(
            peers
                .peers
                .iter()
                .filter(|p| p.subject_id == member.subject_id)
                .map(|p| p.link.clone())
                .collect(),
        ),
    };
    RosterRow {
        subject_id: member.subject_id.clone(),
        role: member.role,
        standing: member.standing,
        joined_at: member.joined_at,
        is_agent: agent_subjects.contains(&member.subject_id),
        presence,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jeliya_api::{DeviceId, LinkReason, PeerRow};
    use time::OffsetDateTime;

    fn ts() -> Timestamp {
        Timestamp::new(OffsetDateTime::UNIX_EPOCH)
    }

    fn member(id: &str) -> MemberRow {
        MemberRow {
            subject_id: SubjectId::new(id),
            role: Role::Member,
            standing: Standing::Active,
            joined_at: ts(),
        }
    }

    fn members(ids: &[&str]) -> RoomMembersOut {
        RoomMembersOut {
            room_id: jeliya_api::RoomId::new("r-1"),
            members: ids.iter().map(|id| member(id)).collect(),
        }
    }

    #[test]
    fn a_closed_room_yields_presence_unknown_not_zero() {
        let view = fold_roster(&members(&["a", "b"]), None, &[], false);
        assert!(!view.session_open);
        assert_eq!(view.reachability, None);
        for row in &view.rows {
            assert!(row.presence.is_unknown(), "closed room must be Unknown");
            assert!(!row.presence.is_absent());
        }
    }

    #[test]
    fn a_stale_peers_read_is_ignored_for_a_closed_room() {
        // Even if peers are handed in, a Closed room never promotes them.
        let peers = RoomPeersOut {
            room_id: jeliya_api::RoomId::new("r-1"),
            reachability: Reachability::Connected,
            peers: vec![PeerRow {
                subject_id: SubjectId::new("a"),
                device_id: DeviceId::new("d"),
                link: Link::Direct { since: ts() },
            }],
        };
        let view = fold_roster(&members(&["a"]), Some(&peers), &[], false);
        assert_eq!(view.reachability, None);
        assert!(view.rows[0].presence.is_unknown());
    }

    #[test]
    fn a_live_member_with_no_peer_row_is_absent_not_unknown() {
        let peers = RoomPeersOut {
            room_id: jeliya_api::RoomId::new("r-1"),
            reachability: Reachability::Alone,
            peers: vec![],
        };
        let view = fold_roster(&members(&["a"]), Some(&peers), &[], true);
        assert!(view.session_open);
        assert_eq!(view.reachability, Some(Reachability::Alone));
        assert!(view.rows[0].presence.is_absent(), "live + no link = Absent");
        assert!(!view.rows[0].presence.is_unknown());
    }

    #[test]
    fn presence_links_are_collected_per_member() {
        let peers = RoomPeersOut {
            room_id: jeliya_api::RoomId::new("r-1"),
            reachability: Reachability::Connected,
            peers: vec![
                PeerRow {
                    subject_id: SubjectId::new("a"),
                    device_id: DeviceId::new("d1"),
                    link: Link::Direct { since: ts() },
                },
                PeerRow {
                    subject_id: SubjectId::new("a"),
                    device_id: DeviceId::new("d2"),
                    link: Link::Relay { since: ts() },
                },
                PeerRow {
                    subject_id: SubjectId::new("b"),
                    device_id: DeviceId::new("d3"),
                    link: Link::NotConnected {
                        reason: LinkReason::Closed,
                    },
                },
            ],
        };
        let view = fold_roster(
            &members(&["a", "b"]),
            Some(&peers),
            &[SubjectId::new("b")],
            true,
        );
        // Member a: two device links.
        match &view.rows[0].presence {
            MemberPresence::Links(links) => assert_eq!(links.len(), 2),
            _ => panic!("expected links"),
        }
        // Member b is marked an agent (derived), and carries its one link.
        assert!(view.rows[1].is_agent);
        assert!(!view.rows[0].is_agent);
    }
}
