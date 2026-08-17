//! Display words for protocol wire enums (§5.7; i18n.md rule 3).
//!
//! Wire value in, UI copy out. This is the only place the Dioxus crate turns a
//! role, a member status, a peer path, or a client lifecycle into a word on the
//! screen, mirroring the React `wireDisplay.ts` map so the two clients cannot
//! drift into different words for the same protocol value.
//!
//! **Raw passthrough is the contract, not a gap.** A daemon newer than this
//! client will send statuses and paths this build has never heard of; the honest
//! rendering of an unrecognized fact is the fact itself — not `Unknown` (which
//! claims the daemon said nothing) and not a crash. And a display label is never
//! the same constant as its wire value: renaming a label renames no wire value,
//! and translating one changes nothing the daemon is told.

use jeliya_api::{Role, Standing};
use jeliya_client::State;

use super::Catalog;

/// The client lifecycle as a status word, from the catalog. A closed enum, so
/// every arm is a designed label — there is no passthrough here because the
/// lifecycle is this crate's own type, not an open wire string.
pub fn status_for(strings: &dyn Catalog, state: State) -> &'static str {
    match state {
        State::Idle | State::Connecting => strings.status_connecting(),
        State::Ready => strings.status_ready(),
        State::Interrupted => strings.status_interrupted(),
        State::Stopping => strings.status_stopping(),
        State::Stopped => strings.status_stopped(),
        State::Failed => strings.status_failed(),
    }
}

/// A member role label. The closed `jeliya_api::Role` vocabulary is exactly
/// `authority` / `member` (the v1 `owner` and the non-role `agent` are retired).
/// Unknown roles pass through raw.
pub fn role_label(strings: &dyn Catalog, role: &str) -> String {
    match role {
        "authority" => strings.wire_role_authority().to_string(),
        "member" => strings.wire_role_member().to_string(),
        other => other.to_string(),
    }
}

/// A member status label. The **empty** string maps to "unknown" — the daemon
/// reported nothing. A status this build does not recognize is different: the
/// daemon did say something, so it passes through raw.
pub fn member_status_label(strings: &dyn Catalog, status: &str) -> String {
    match status {
        "active" => strings.wire_status_active().to_string(),
        "invited" => strings.wire_status_invited().to_string(),
        "left" => strings.wire_status_left().to_string(),
        "removed" => strings.wire_status_removed().to_string(),
        "" => strings.wire_status_unknown().to_string(),
        other => other.to_string(),
    }
}

/// A peer connection path label. Unknown paths pass through raw.
pub fn peer_path_label(strings: &dyn Catalog, path: &str) -> String {
    match path {
        "direct" => strings.wire_path_direct().to_string(),
        "relay" => strings.wire_path_relay().to_string(),
        other => other.to_string(),
    }
}

/// A member [`Role`] label from the closed vocabulary. Unlike the `&str`
/// [`role_label`], this takes the typed enum (used by the #91 historical
/// roster), so every arm is a designed label with no passthrough branch.
pub fn role_word(strings: &dyn Catalog, role: Role) -> &'static str {
    match role {
        Role::Authority => strings.wire_role_authority(),
        Role::Member => strings.wire_role_member(),
    }
}

/// A member [`Standing`] label from the closed vocabulary. Reuses the existing
/// wire member-status copy (`active`→"Member", `left`, `removed`), so the #91
/// historical roster names an ended standing in the same words the rest of the
/// UI uses.
pub fn standing_word(strings: &dyn Catalog, standing: Standing) -> &'static str {
    match standing {
        Standing::Active => strings.wire_status_active(),
        Standing::Left => strings.wire_status_left(),
        Standing::Removed => strings.wire_status_removed(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::l10n::{En, Fr};
    use jeliya_api::{Role, Standing};

    #[test]
    fn unknown_values_pass_through_raw() {
        // A future role/status/path the client has never heard of renders as
        // itself, never as a fabricated label or a crash. `owner`/`agent` are now
        // in this category — retired vocabulary, so they pass through raw.
        assert_eq!(role_label(&En, "observer"), "observer");
        assert_eq!(role_label(&En, "owner"), "owner");
        assert_eq!(member_status_label(&En, "suspended"), "suspended");
        assert_eq!(peer_path_label(&En, "mesh"), "mesh");
    }

    #[test]
    fn the_canonical_role_maps_to_a_designed_label() {
        // The label a known ROLE maps to is a designed word, distinct from the
        // wire constant (roles are translatable display, not Tier-2 tokens).
        assert_eq!(role_label(&En, "authority"), "Authority");
        assert_eq!(role_label(&Fr, "authority"), "Autorité");
        assert_eq!(role_label(&En, "member"), "Member");
        assert_ne!(role_label(&Fr, "authority"), "authority");
        assert_eq!(member_status_label(&En, "active"), "Member");
    }

    #[test]
    fn tier2_path_tokens_are_rendered_verbatim() {
        // `direct`/`relay` are Tier-2 protocol tokens (docs/glossary-fr.md):
        // rendered EXACTLY as the daemon reports, IDENTICAL in every language —
        // the deliberate exception to "labels differ from wire values".
        assert_eq!(peer_path_label(&En, "relay"), "relay");
        assert_eq!(peer_path_label(&Fr, "relay"), "relay");
        assert_eq!(peer_path_label(&En, "direct"), "direct");
        assert_eq!(peer_path_label(&Fr, "direct"), "direct");
    }

    #[test]
    fn empty_member_status_is_unknown_not_passthrough() {
        assert_eq!(member_status_label(&En, ""), "Unknown");
        assert_eq!(member_status_label(&Fr, ""), "Inconnu");
    }

    #[test]
    fn lifecycle_maps_every_state_to_a_word() {
        for state in [
            State::Idle,
            State::Connecting,
            State::Ready,
            State::Interrupted,
            State::Stopping,
            State::Stopped,
            State::Failed,
        ] {
            assert!(!status_for(&En, state).is_empty());
            assert!(!status_for(&Fr, state).is_empty());
        }
        assert_eq!(status_for(&En, State::Ready), "Connected");
        assert_eq!(status_for(&Fr, State::Ready), "Connecté");
    }

    #[test]
    fn role_word_maps_typed_enum_to_designed_label_in_both_locales() {
        // `role_word` takes the typed `Role` enum — no passthrough branch —
        // used by the #91 historical roster. EN and FR must each produce a
        // non-empty label, and the FR label must differ from EN to confirm
        // the catalog is actually translated.
        for role in [Role::Authority, Role::Member] {
            let en = role_word(&En, role);
            let fr = role_word(&Fr, role);
            assert!(!en.is_empty(), "{role:?} EN role_word is empty");
            assert!(!fr.is_empty(), "{role:?} FR role_word is empty");
            assert_ne!(
                en, fr,
                "{role:?} FR role_word is identical to EN — not translated"
            );
        }
    }

    #[test]
    fn standing_word_maps_all_variants_consistently_with_member_status_label() {
        // `standing_word` reuses the wire member-status copy so the #91
        // historical roster names an ended standing in the same words the
        // rest of the UI uses. The typed and string-keyed paths must agree.
        for (standing, wire_key) in [
            (Standing::Active, "active"),
            (Standing::Left, "left"),
            (Standing::Removed, "removed"),
        ] {
            let en_word = standing_word(&En, standing);
            let fr_word = standing_word(&Fr, standing);
            assert!(
                !en_word.is_empty(),
                "{standing:?} EN standing_word is empty"
            );
            assert!(
                !fr_word.is_empty(),
                "{standing:?} FR standing_word is empty"
            );
            assert_eq!(
                en_word,
                member_status_label(&En, wire_key),
                "{standing:?} EN: standing_word and member_status_label disagree"
            );
            assert_eq!(
                fr_word,
                member_status_label(&Fr, wire_key),
                "{standing:?} FR: standing_word and member_status_label disagree"
            );
        }
    }
}
