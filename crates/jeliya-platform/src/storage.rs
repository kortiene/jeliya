//! The storage capabilities (#174 §D6, AC-2): preferences, secret custody, and
//! protected-directory ownership — three **separate** concerns, deliberately
//! not merged, each with honest durability.
//!
//! - [`Preferences`] — a namespaced, non-secret key/value store for
//!   device-local UI state, keyed by a typed [`PreferenceKey`] (a component
//!   cannot write an arbitrary or legacy key). A write reports its
//!   [`WriteOutcome`], so the UI can honestly say "applies this session, not
//!   saved" instead of implying a false success, and [`Preferences::durability`]
//!   is a queryable platform fact the UI reads rather than guesses.
//! - [`SecretStore`] — custody of the only browser-held secrets (the
//!   tab-scoped session credential and its tickets), kept separate so credential
//!   material never lands in the non-secret store and a preferences export can
//!   never read secrets. The daemon token never enters this store on the
//!   untrusted side (§K5).
//! - [`PrivateDirectory`] — the protected/backup-excluded directory the daemon
//!   owns, exposed as capability **facts**, never as a raw path the UI can
//!   traverse (moving daemon filesystem authority into UI code is a non-goal).
//!
//! Clean-slate (§K12): [`PreferenceKey`] and [`SecretKey`] are typed enums that
//! name no legacy storage key (`jeliya.lastRoom`, `app_prefs.json`,
//! `jeliya.aliases.v1`, …). The concrete on-disk namespace belongs to each
//! target implementation (#178 web, #185 desktop, #173 Android), not to this
//! contract.

use jeliya_api::RoomId;

use crate::error::{Availability, CapabilityError};

/// A typed, allowlisted device-local preference key.
///
/// The key set is closed: a component cannot write an arbitrary or legacy key,
/// which is how "keys are a validated newtype over an allowlisted key set" is
/// enforced. Per-room preferences carry the [`RoomId`] in the key, so a draft
/// or a pin flag is scoped to its room without a second lookup dimension.
///
/// The enum names no legacy storage string; the concrete namespace is the
/// target implementation's business (§K12).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum PreferenceKey {
    /// The last room the user had open (restored once per launch, from the bare
    /// root only — see [`crate::navigation`]).
    LastRoom,
    /// The unsent composer draft for one room.
    Draft {
        /// The room the draft belongs to.
        room_id: RoomId,
    },
    /// The user's per-peer alias map (serialized by the caller).
    Aliases,
    /// The user's own self-label.
    SelfLabel,
    /// Whether one room is pinned.
    Pinned {
        /// The pinned room.
        room_id: RoomId,
    },
    /// Whether one room is archived.
    Archived {
        /// The archived room.
        room_id: RoomId,
    },
    /// The last-seen marker for one room.
    LastSeen {
        /// The room the marker is for.
        room_id: RoomId,
    },
    /// The text/formatting locale override.
    Locale,
}

/// The result of a preference write.
///
/// A write that does not reach durable storage still applies **for this
/// session** — the setter's in-memory change stands — but the caller learns it
/// was not written, so the UI can say "applies this session, not saved" rather
/// than imply a false success. This is the `PrefsStore.lastWriteOk` honesty
/// lifted into the type.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WriteOutcome {
    /// The value reached durable storage.
    Durable,
    /// The value applies for this session only; it did not reach durable
    /// storage.
    SessionOnly,
}

impl WriteOutcome {
    /// `true` when the write reached durable storage.
    pub fn is_durable(self) -> bool {
        matches!(self, WriteOutcome::Durable)
    }
}

/// Whether device-local preference storage survives a restart.
///
/// A queryable platform fact so the UI never pretends a browser reload restored
/// state: the product contract forbids the UI from claiming otherwise, and
/// making durability a fact the UI reads is how that is enforceable.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Durability {
    /// An ordinary browser tab — nothing survives reload; held in memory, gone
    /// with the tab.
    SessionScoped,
    /// Packaged desktop / Android — survives restart.
    Persistent,
}

/// The namespaced, non-secret device-local preference store.
pub trait Preferences {
    /// Read a preference, or `None` if unset.
    fn get(&self, key: &PreferenceKey) -> Option<String>;

    /// Write a preference, reporting whether it reached durable storage. The
    /// in-memory value applies for this session regardless of the outcome.
    fn set(&self, key: PreferenceKey, value: &str) -> WriteOutcome;

    /// Remove a preference, reporting the same durability outcome as
    /// [`Preferences::set`].
    fn remove(&self, key: &PreferenceKey) -> WriteOutcome;

    /// Whether this store survives a restart. A platform fact, not a guess.
    fn durability(&self) -> Durability;
}

/// A secret value under [`SecretStore`] custody. Its `Debug` is redacted so the
/// secret never lands in a log or an error string; read it explicitly with
/// [`Secret::expose`].
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// Wrap a secret value.
    pub fn new(secret: impl Into<String>) -> Self {
        Self(secret.into())
    }

    /// Read the secret. The method name is deliberately explicit so every read
    /// site is greppable.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// Which secret a [`SecretStore`] operation names.
///
/// A closed set: the only browser-held secrets are the tab-scoped session
/// credential and its invite tickets. The daemon token is **not** here (§K5).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum SecretKey {
    /// The tab-scoped session credential.
    SessionCredential,
    /// An invite ticket, identified by an opaque caller-chosen tag.
    InviteTicket {
        /// A caller-chosen tag distinguishing one ticket from another.
        tag: String,
    },
}

/// Custody of the browser-held secrets, kept separate from [`Preferences`] so
/// credential material never lands in the non-secret store.
pub trait SecretStore {
    /// Read a secret, or `None` if unset.
    fn get(&self, key: &SecretKey) -> Option<Secret>;

    /// Store a secret, reporting whether it reached durable storage (on the
    /// browser these are session-scoped and die with the tab).
    fn set(&self, key: SecretKey, secret: Secret) -> WriteOutcome;

    /// Remove a secret.
    fn remove(&self, key: &SecretKey) -> WriteOutcome;

    /// Whether this store survives a restart. On the browser,
    /// [`Durability::SessionScoped`].
    fn durability(&self) -> Durability;
}

/// Facts about the protected/backup-excluded directory the daemon owns.
///
/// Exposed as capability facts, **not** as a raw path the UI can traverse —
/// moving daemon filesystem authority into UI code is an explicit non-goal. On
/// the browser this capability is [`Availability::Unavailable`], and every fact
/// getter returns [`CapabilityError::Unavailable`].
pub trait PrivateDirectory {
    /// Whether this capability is present on the platform.
    fn availability(&self) -> Availability;

    /// Whether the directory is excluded from OS backups (Android
    /// `<noBackupFilesDir>/engine`; desktop app-support). `Err(Unavailable)`
    /// where the capability is absent.
    fn is_backup_excluded(&self) -> Result<bool, CapabilityError>;

    /// Whether the directory is owned by the daemon (never traversed by the
    /// UI). `Err(Unavailable)` where the capability is absent.
    fn is_owned_by_daemon(&self) -> Result<bool, CapabilityError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_never_renders_its_value() {
        let secret = Secret::new("super-secret-token");
        assert_eq!(secret.expose(), "super-secret-token");
        let rendered = format!("{secret:?}");
        assert!(!rendered.contains("super-secret-token"), "{rendered}");
        assert!(rendered.contains("redacted"));
    }

    #[test]
    fn per_room_preference_keys_are_distinct_by_room() {
        let a = PreferenceKey::Draft {
            room_id: RoomId::new("r-1"),
        };
        let b = PreferenceKey::Draft {
            room_id: RoomId::new("r-2"),
        };
        assert_ne!(a, b);
        assert_eq!(
            a,
            PreferenceKey::Draft {
                room_id: RoomId::new("r-1"),
            }
        );
    }

    #[test]
    fn write_outcome_durability_reads_honestly() {
        assert!(WriteOutcome::Durable.is_durable());
        assert!(!WriteOutcome::SessionOnly.is_durable());
    }
}
