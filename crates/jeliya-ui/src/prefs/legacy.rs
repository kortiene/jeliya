//! The enumerated legacy-key purge policy (#178 §6.5, AC-6).
//!
//! This module is **data only**: a closed allowlist of the retiring React
//! client's `localStorage` keys, plus the predicate that decides whether a key
//! is in it. There is **no reader** — nothing here (or anywhere in the shipped
//! shell) constructs a *read* of a legacy key, a legacy `?tab=` query, a legacy
//! draft, or a legacy profile, and interprets it as new Dioxus state. The
//! browser binding (`jeliya-platform-web`) consumes [`LEGACY_WEB_KEYS`] and
//! [`is_legacy_web_key`] to **remove** (write-only) each matching key from
//! `localStorage` on construction; removal needs no confirmation (contract
//! §"Preferences").
//!
//! Legacy keys are *never* interpreted as new state (D5): the fresh schema uses
//! a namespace no retiring client ever wrote ([`super::NAMESPACE`]), asserted
//! disjoint from this list.

/// The exact, retiring-React `localStorage` keys the boot-time purge removes.
///
/// These are the bare keys the React shell wrote (`ui/src/lib/*`): the last
/// room, the alias map, the room-flag map, the last-seen map, and the locale.
/// Per-room draft keys are matched by prefix instead (see [`LEGACY_KEY_PREFIXES`]),
/// because their room-id suffix makes them an open family rather than a fixed
/// list. The legacy `?tab=` affordance is a *query*, not a storage key, and is
/// simply never read.
pub const LEGACY_WEB_KEYS: &[&str] = &[
    "jeliya.lastRoom",
    "jeliya.aliases.v1",
    "jeliya.roomFlags",
    "jeliya.lastSeen",
    "jeliya.locale",
];

/// Legacy key *families* matched by prefix: the per-room draft keys the React
/// shell wrote as `jeliya.draft.<room_id>`. A prefix match keeps the purge
/// closed (only this documented family) while covering every room id, without
/// enumerating-and-deleting arbitrary storage.
pub const LEGACY_KEY_PREFIXES: &[&str] = &["jeliya.draft."];

/// Whether `key` is a known legacy React key the purge is authorized to remove.
///
/// True for an exact [`LEGACY_WEB_KEYS`] entry or a [`LEGACY_KEY_PREFIXES`]
/// family member. **Deliberately narrow**: it never matches the fresh
/// [`super::NAMESPACE`] and never matches an arbitrary `jeliya`-prefixed key, so
/// the purge cannot become an enumerate-and-nuke of all storage. This is a
/// classifier for a *removal*, never a gate that admits a value as state.
pub fn is_legacy_web_key(key: &str) -> bool {
    if LEGACY_WEB_KEYS.contains(&key) {
        return true;
    }
    // A legacy draft family member — but never the fresh namespace's own draft
    // keys, which live under `jeliya.dx.v1.` and are handled by the schema.
    if key.starts_with(super::NAMESPACE_PREFIX) {
        return false;
    }
    LEGACY_KEY_PREFIXES
        .iter()
        .any(|prefix| key.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_legacy_keys_match() {
        for key in LEGACY_WEB_KEYS {
            assert!(is_legacy_web_key(key), "{key} should be a legacy key");
        }
    }

    #[test]
    fn draft_family_matches_by_prefix() {
        assert!(is_legacy_web_key("jeliya.draft.room-1"));
        assert!(is_legacy_web_key("jeliya.draft.blake3%3Aabc"));
    }

    #[test]
    fn the_purge_never_matches_fresh_or_arbitrary_keys() {
        // The fresh namespace is never a legacy key — it is disjoint by design.
        assert!(!is_legacy_web_key("jeliya.dx.v1.lastRoom"));
        assert!(!is_legacy_web_key("jeliya.dx.v1.draft.room-1"));
        // Arbitrary or unrelated keys are untouched.
        assert!(!is_legacy_web_key("some.other.app"));
        assert!(!is_legacy_web_key("jeliya.unknownFutureKey"));
        assert!(!is_legacy_web_key(""));
    }
}
