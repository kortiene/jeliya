//! The typed affordance gate (§8, D6, contract invariant 5).
//!
//! Each destructive/sensitive action maps to the one [`CapabilityToken`] that
//! authorizes it. A pane renders an action control **only** when the room's own
//! capability list contains that token, so an unauthorized action is **absent**
//! from the accessibility tree, not merely disabled — the UI never advertises an
//! action the daemon would refuse, and "who may act" is one typed lookup rather
//! than role logic scattered through render code.

use jeliya_api::CapabilityToken;

/// A capability-gated action a #180 pane may offer.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Action {
    /// Issue an invitation (`invite.mint`).
    Invite,
    /// Revoke an outstanding invitation (`invite.revoke`).
    Revoke,
    /// Remove a member (`member.remove`, destructive).
    Remove,
    /// Leave the room (`room.leave`, destructive).
    Leave,
    /// Activate a Closed room so presence can be read (`room.activate`).
    Activate,
}

impl Action {
    /// The single [`CapabilityToken`] that authorizes this action.
    ///
    /// An exhaustive match, so a new action cannot be added without naming its
    /// authorizing token (the compiler-total form of the gate).
    pub fn token(self) -> CapabilityToken {
        match self {
            Action::Invite => CapabilityToken::InviteMint,
            Action::Revoke => CapabilityToken::InviteRevoke,
            Action::Remove => CapabilityToken::MemberRemove,
            Action::Leave => CapabilityToken::RoomLeave,
            Action::Activate => CapabilityToken::RoomActivate,
        }
    }
}

/// Whether `action` is authorized by the room's capability list — a pure
/// lookup. A control is rendered only when this is `true`; otherwise it is
/// absent (never a disabled affordance).
pub fn authorized(action: Action, caps: &[CapabilityToken]) -> bool {
    caps.contains(&action.token())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_action_is_authorized_only_by_its_own_token() {
        let caps = [CapabilityToken::InviteMint, CapabilityToken::MemberRemove];
        assert!(authorized(Action::Invite, &caps));
        assert!(authorized(Action::Remove, &caps));
        // Absent tokens => unauthorized => the affordance is absent.
        assert!(!authorized(Action::Revoke, &caps));
        assert!(!authorized(Action::Leave, &caps));
        assert!(!authorized(Action::Activate, &caps));
    }

    #[test]
    fn an_empty_capability_list_authorizes_nothing() {
        for action in [
            Action::Invite,
            Action::Revoke,
            Action::Remove,
            Action::Leave,
            Action::Activate,
        ] {
            assert!(!authorized(action, &[]));
        }
    }

    #[test]
    fn the_action_token_map_is_the_expected_pairing() {
        assert_eq!(Action::Invite.token(), CapabilityToken::InviteMint);
        assert_eq!(Action::Revoke.token(), CapabilityToken::InviteRevoke);
        assert_eq!(Action::Remove.token(), CapabilityToken::MemberRemove);
        assert_eq!(Action::Leave.token(), CapabilityToken::RoomLeave);
        assert_eq!(Action::Activate.token(), CapabilityToken::RoomActivate);
    }
}
