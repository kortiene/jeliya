//! The archive capability gate (#91 D2): capability absence is **structural**,
//! not decorative.
//!
//! A capability token *is* the name of the operation it authorizes, present on a
//! `room.list` row **iff** the op would not be refused on
//! membership/standing/lifecycle/liveness (`docs/protocol-v2.md`). For a
//! departed room every standing-gated op is refused, so the served array is
//! effectively `["room.archive"]`. The archive pane trusts that served set and
//! renders each affordance **iff** its token is present — but because every live
//! action is forbidden for a departed caller, the pane simply contains **no**
//! composer/Invite/Leave/file/Pipe affordance at all (they are not
//! present-and-disabled).
//!
//! This module carries the [`FORBIDDEN_IN_ARCHIVE`] set and the [`grants`]
//! predicate so a test can prove the served capabilities and the pane's
//! composition agree; the pane itself needs no per-affordance check, since the
//! affordances are absent by construction.

use jeliya_api::CapabilityToken;

/// Every live/mutating room-scoped capability an archive must never offer. A
/// departed room's served `capabilities` must contain none of these, and the
/// archive composition must dispatch none of them — the whole live-action
/// surface, gated ABSENT at once (#91 D2, the #180 "affordances ABSENT not
/// disabled" invariant applied wholesale).
///
/// Deliberately excluded (not room-scoped live actions a departed caller is
/// refused): `room.archive` itself, and the account-scoped `subject.ensure` /
/// `daemon.stop` / `room.create` / `room.list` / `fleet.list` / `invite.redeem`
/// (redeem needs a capability the departed user does not hold, so it is not an
/// in-room affordance the pane could offer).
pub const FORBIDDEN_IN_ARCHIVE: &[CapabilityToken] = &[
    CapabilityToken::MessageSend,
    CapabilityToken::StatusPost,
    CapabilityToken::RoomLeave,
    CapabilityToken::RoomActivate,
    CapabilityToken::RoomDeactivate,
    CapabilityToken::InviteMint,
    CapabilityToken::InviteList,
    CapabilityToken::InviteRevoke,
    CapabilityToken::MemberRemove,
    CapabilityToken::FileShare,
    CapabilityToken::FileList,
    CapabilityToken::FileFetch,
    CapabilityToken::FileRead,
    CapabilityToken::TransferCancel,
    CapabilityToken::PipePublish,
    CapabilityToken::PipeList,
    CapabilityToken::PipeConnect,
    CapabilityToken::PipeRelease,
    CapabilityToken::PipeRevoke,
    CapabilityToken::RoomTimeline,
    CapabilityToken::RoomMembers,
    CapabilityToken::RoomPeers,
    CapabilityToken::StatusHistory,
    CapabilityToken::StreamSubscribe,
    CapabilityToken::StreamUnsubscribe,
    CapabilityToken::StreamResync,
];

/// Whether the served `capabilities` grant `token`. A pure membership check: the
/// gate trusts the daemon-served array (authoritative), never inferring a
/// capability the daemon did not grant (#91 §10).
pub fn grants(caps: &[CapabilityToken], token: CapabilityToken) -> bool {
    caps.contains(&token)
}

/// Whether the served `capabilities` are consistent with a departed room's
/// archive surface: they contain **no** forbidden live/mutating token. A helper
/// so the seam test can assert the served set the gate trusts is honest.
pub fn is_archive_only(caps: &[CapabilityToken]) -> bool {
    !caps
        .iter()
        .any(|token| FORBIDDEN_IN_ARCHIVE.contains(token))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_forbidden_set_excludes_room_archive_and_room_list() {
        // The one op an archive dispatches — and the enumeration op that lists
        // departed rooms — are NOT forbidden.
        assert!(!FORBIDDEN_IN_ARCHIVE.contains(&CapabilityToken::RoomArchive));
        assert!(!FORBIDDEN_IN_ARCHIVE.contains(&CapabilityToken::RoomList));
    }

    #[test]
    fn the_forbidden_set_covers_every_live_room_action() {
        for token in [
            CapabilityToken::MessageSend,
            CapabilityToken::RoomLeave,
            CapabilityToken::RoomActivate,
            CapabilityToken::InviteMint,
            CapabilityToken::MemberRemove,
            CapabilityToken::FileShare,
            CapabilityToken::FileFetch,
            CapabilityToken::PipePublish,
            CapabilityToken::RoomTimeline,
            CapabilityToken::RoomMembers,
            CapabilityToken::StreamSubscribe,
        ] {
            assert!(
                FORBIDDEN_IN_ARCHIVE.contains(&token),
                "{token:?} must be forbidden in an archive"
            );
        }
    }

    #[test]
    fn grants_is_a_pure_membership_check() {
        let caps = [CapabilityToken::RoomArchive];
        assert!(grants(&caps, CapabilityToken::RoomArchive));
        assert!(!grants(&caps, CapabilityToken::MessageSend));
        assert!(!grants(&[], CapabilityToken::RoomArchive));
    }

    #[test]
    fn a_departed_rooms_served_capabilities_are_archive_only() {
        // The protocol-conformant departed-room capabilities.
        assert!(is_archive_only(&[CapabilityToken::RoomArchive]));
        assert!(is_archive_only(&[]));
        // A served array that (wrongly) still carried a live token is rejected —
        // the gate would then be trusting an impossible capability.
        assert!(!is_archive_only(&[
            CapabilityToken::RoomArchive,
            CapabilityToken::MessageSend,
        ]));
    }
}
