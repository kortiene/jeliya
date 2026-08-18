//! The invitations fold (§7.1, D2/D3): `invite.list` as a **separate fact** from
//! roster standing.
//!
//! An outstanding invite is never a roster standing (there is no "Invited"
//! member — D2). Each invite carries a served [`Redeemability`]; the fold derives
//! which capability-gated action a row offers — **Revoke** for an `Outstanding`
//! invite, **Re-invite** for an `Expired` one — and the re-invite target (the
//! same `subject_id` + role a fresh capability is minted for, D3). The minted
//! capability itself is never in `invite.list`, so it appears nowhere here.

use jeliya_api::{
    InviteId, InviteListOut, InviteRow, Redeemability, Role, SubjectId, Timestamp, Truncated,
};

/// One invitation row, with the derived affordance flags.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct InviteRowView {
    /// The invite id.
    pub invite_id: InviteId,
    /// The bound identity.
    pub subject_id: SubjectId,
    /// The granted role.
    pub role: Role,
    /// Absolute expiry.
    pub expires_at: Timestamp,
    /// The served redeemability classification.
    pub redeemability: Redeemability,
    /// Whether a **Revoke** affordance applies (an `Outstanding` invite).
    pub can_revoke: bool,
    /// Whether a **Re-invite** affordance applies (an `Expired` invite — mint a
    /// fresh capability for the same identity and revoke this stale one, D3).
    pub can_reinvite: bool,
}

impl InviteRowView {
    /// The re-invite target: the identity and role a fresh `invite.mint` binds
    /// to (the expiry is chosen anew by the operator). `None` unless this row is
    /// re-invitable.
    pub fn reinvite_target(&self) -> Option<(SubjectId, Role)> {
        self.can_reinvite
            .then(|| (self.subject_id.clone(), self.role))
    }

    fn from_row(row: &InviteRow) -> Self {
        InviteRowView {
            invite_id: row.invite_id.clone(),
            subject_id: row.subject_id.clone(),
            role: row.role,
            expires_at: row.expires_at,
            redeemability: row.redeemability,
            can_revoke: row.redeemability == Redeemability::Outstanding,
            can_reinvite: row.redeemability == Redeemability::Expired,
        }
    }
}

/// The folded invitations list.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct InviteView {
    /// One row per invite.
    pub rows: Vec<InviteRowView>,
    /// Whether a bounded first page left more invites unread (a "show more"
    /// continuation, never an unbounded read).
    pub has_more: bool,
}

impl InviteView {
    /// Whether there are no invites (a real empty answer).
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// Fold `invite.list` into an [`InviteView`].
pub fn fold_invites(list: &InviteListOut) -> InviteView {
    InviteView {
        rows: list.invites.iter().map(InviteRowView::from_row).collect(),
        has_more: matches!(list.truncated, Truncated::More { .. }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jeliya_api::{Cursor, RoomId};
    use time::OffsetDateTime;

    fn ts() -> Timestamp {
        Timestamp::new(OffsetDateTime::UNIX_EPOCH)
    }

    fn invite(id: &str, subject: &str, r: Redeemability) -> InviteRow {
        InviteRow {
            invite_id: InviteId::new(id),
            subject_id: SubjectId::new(subject),
            role: Role::Member,
            expires_at: ts(),
            redeemability: r,
        }
    }

    fn list(rows: Vec<InviteRow>, truncated: Truncated) -> InviteListOut {
        InviteListOut {
            room_id: RoomId::new("r-1"),
            invites: rows,
            truncated,
        }
    }

    #[test]
    fn outstanding_offers_revoke_expired_offers_reinvite() {
        let view = fold_invites(&list(
            vec![
                invite("i1", "a", Redeemability::Outstanding),
                invite("i2", "b", Redeemability::Expired),
                invite("i3", "c", Redeemability::Revoked),
                invite("i4", "d", Redeemability::Redeemed),
            ],
            Truncated::Complete,
        ));
        assert!(view.rows[0].can_revoke && !view.rows[0].can_reinvite);
        assert!(view.rows[1].can_reinvite && !view.rows[1].can_revoke);
        // Revoked / Redeemed offer neither.
        assert!(!view.rows[2].can_revoke && !view.rows[2].can_reinvite);
        assert!(!view.rows[3].can_revoke && !view.rows[3].can_reinvite);
    }

    #[test]
    fn the_reinvite_target_is_the_same_identity() {
        let view = fold_invites(&list(
            vec![invite("i2", "blake3:same", Redeemability::Expired)],
            Truncated::Complete,
        ));
        let (subject, role) = view.rows[0].reinvite_target().expect("re-invitable");
        assert_eq!(subject, SubjectId::new("blake3:same"));
        assert_eq!(role, Role::Member);
        // A non-re-invitable row has no target.
        let outstanding = fold_invites(&list(
            vec![invite("i1", "a", Redeemability::Outstanding)],
            Truncated::Complete,
        ));
        assert_eq!(outstanding.rows[0].reinvite_target(), None);
    }

    #[test]
    fn a_truncated_page_reports_more() {
        let complete = fold_invites(&list(vec![], Truncated::Complete));
        assert!(!complete.has_more);
        let more = fold_invites(&list(
            vec![invite("i1", "a", Redeemability::Outstanding)],
            Truncated::More {
                cursor: Cursor::At { pos: 1 },
            },
        ));
        assert!(more.has_more);
    }
}
