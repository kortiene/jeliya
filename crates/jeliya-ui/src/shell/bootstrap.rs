//! The bootstrap / onboarding state machine (#178 §5.D), a pure fold from
//! daemon truth to a view.
//!
//! Bootstrap **narrates daemon truth** and never shows an infinite spinner. The
//! fold's central rule (contract §"Bootstrap and onboarding"): **booting is
//! *unknown*, not *zero***. While the client is not yet usable, or the identity
//! snapshot is not yet known, or the room list has not yet answered, the view is
//! [`BootView::Booting`] — never an empty timeline, never "you are alone", never
//! "0 rooms" rendered as an answer.
//!
//! ## The D2 connection snapshot (identity + storage generation)
//!
//! "Does an identity exist?" and "what storage generation is this?" are reads,
//! not mutations: they arrive on the daemon's `Hello` frame as
//! [`jeliya_api::SubjectState`] and a `storage_generation`. The current
//! `ClientHandle` seam does not yet surface them (D2 / R1); this module models
//! them as a [`ConnectionSnapshot`] the composition supplies — scripted against
//! the deterministic mock today, fed from the real `Hello` once #171/#270 land.
//! Keeping the fold a pure function of `(State, snapshot, rooms)` is what makes
//! the seam swap a one-line composition change with no shell edit.
//!
//! Recovery (corrupt/unsupported new-format preference state, §6.4) is a
//! **non-blocking, visible** concern, not a fold state that replaces the app: it
//! rides alongside the view as [`Boot::recovery`] so the shell stays usable with
//! a banner rather than being hidden behind an error page (D7 — "never a blank
//! panel").

use jeliya_api::SubjectState;
use jeliya_client::State;

use crate::prefs::SchemaAnomaly;

/// The read-only connection snapshot captured from `Hello` (D2): whether an
/// identity exists and which storage generation the daemon is on.
///
/// Additive and coordinated with #270 on the real seam; local here until then.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ConnectionSnapshot {
    /// The local subject, or its stated absence.
    pub subject: SubjectState,
    /// The daemon's storage generation. Drives the native fresh-state/reset
    /// policy (#185/#173); the browser owns no data directory, so it is carried
    /// but not acted on here (§5.D).
    pub storage_generation: u64,
}

/// What the shell knows about the room list — an *answer*, never a guess.
///
/// [`RoomsKnowledge::Unknown`] (the pre-answer state) is why booting reads as
/// unknown, not zero: an empty `Vec` before the first reply must not render as
/// "no rooms".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RoomsKnowledge {
    /// `room.list` has not answered yet.
    Unknown,
    /// `room.list` answered with `count` rooms.
    Loaded {
        /// How many rooms the answer carried.
        count: usize,
    },
    /// `room.list` failed terminally — the shell shows the error, which is *not*
    /// the same as an empty account.
    Failed,
}

/// The onboarding step, when the fold resolves to onboarding.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OnboardStep {
    /// Create or connect the local identity (subject `Absent`).
    Identity,
    /// Create or join the first room (subject `Present`, zero rooms).
    Rooms,
}

/// A terminal/failed bootstrap, each with an honest label (not a generic
/// "connecting" over a dead client).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FailureView {
    /// The client is draining accepted work before stopping.
    Stopping,
    /// The client has stopped.
    Stopped,
    /// The client failed and will not retry.
    Failed,
}

/// The lifecycle-truth view the shell renders (before the recovery overlay).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BootView {
    /// Not enough is known yet: render the route's loading state, never zero.
    Booting,
    /// First-run onboarding at `step`.
    Onboarding(OnboardStep),
    /// The mounted global shell.
    Shell,
    /// A terminal/failed bootstrap with a way forward (retry / diagnostics /
    /// reset), never an infinite spinner.
    Failed(FailureView),
}

/// The visible recovery affordance for corrupt/unsupported new-format
/// preference state that was reset (§6.4). Non-blocking: it overlays whatever
/// [`BootView`] the fold produced.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RecoveryView {
    /// How many preference keys were reset (for the banner's plain explanation).
    pub reset_count: usize,
}

impl RecoveryView {
    /// A recovery view from the schema anomalies found at boot, or `None` when
    /// none were found. `reason`s are not surfaced to the user (they are
    /// diagnostics); only the count of reset keys is.
    pub fn from_anomalies(anomalies: &[SchemaAnomaly]) -> Option<RecoveryView> {
        if anomalies.is_empty() {
            None
        } else {
            Some(RecoveryView {
                reset_count: anomalies.len(),
            })
        }
    }
}

/// The composed boot state: the lifecycle-truth [`BootView`] plus the optional,
/// non-blocking recovery overlay.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Boot {
    /// The primary view.
    pub view: BootView,
    /// The visible recovery banner, if preference state was reset.
    pub recovery: Option<RecoveryView>,
}

/// Derive the lifecycle-truth [`BootView`] from daemon truth (§5.D).
///
/// Rules:
/// - `Stopping`/`Stopped`/`Failed` → [`BootView::Failed`] with the honest label.
/// - `Idle`/`Connecting`, or an unknown snapshot → [`BootView::Booting`]
///   (unknown ≠ zero).
/// - `Ready`/`Interrupted` + subject `Absent` → onboarding identity step.
/// - subject `Present` + rooms `Unknown` → [`BootView::Booting`] (never "0
///   rooms" before an answer).
/// - subject `Present` + rooms `Loaded{0}` → onboarding rooms step.
/// - subject `Present` + rooms `Loaded{>=1}` **or** `Failed` → the mounted shell
///   (a failed `room.list` is a shell-time error, not an empty account, and not
///   onboarding).
pub fn derive_boot_view(
    lifecycle: State,
    snapshot: Option<&ConnectionSnapshot>,
    rooms: RoomsKnowledge,
) -> BootView {
    match lifecycle {
        State::Stopping => return BootView::Failed(FailureView::Stopping),
        State::Stopped => return BootView::Failed(FailureView::Stopped),
        State::Failed => return BootView::Failed(FailureView::Failed),
        // Not yet usable: booting is unknown, not zero.
        State::Idle | State::Connecting => return BootView::Booting,
        // Ready, or Interrupted (was Ready, recovering — keep the shell path).
        State::Ready | State::Interrupted => {}
    }
    let Some(snapshot) = snapshot else {
        // Usable but the identity truth has not arrived — still unknown.
        return BootView::Booting;
    };
    match &snapshot.subject {
        SubjectState::Absent => BootView::Onboarding(OnboardStep::Identity),
        SubjectState::Present { .. } => match rooms {
            RoomsKnowledge::Unknown => BootView::Booting,
            RoomsKnowledge::Loaded { count: 0 } => BootView::Onboarding(OnboardStep::Rooms),
            RoomsKnowledge::Loaded { .. } | RoomsKnowledge::Failed => BootView::Shell,
        },
    }
}

/// Derive the full [`Boot`] — the view plus the non-blocking recovery overlay.
pub fn derive_boot(
    lifecycle: State,
    snapshot: Option<&ConnectionSnapshot>,
    rooms: RoomsKnowledge,
    anomalies: &[SchemaAnomaly],
) -> Boot {
    Boot {
        view: derive_boot_view(lifecycle, snapshot, rooms),
        recovery: RecoveryView::from_anomalies(anomalies),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jeliya_api::{DeviceId, SubjectId};

    fn present() -> ConnectionSnapshot {
        ConnectionSnapshot {
            subject: SubjectState::Present {
                subject_id: SubjectId::new("blake3:subject"),
                device_id: DeviceId::new("blake3:device"),
            },
            storage_generation: 1,
        }
    }

    fn absent() -> ConnectionSnapshot {
        ConnectionSnapshot {
            subject: SubjectState::Absent,
            storage_generation: 1,
        }
    }

    #[test]
    fn booting_is_unknown_not_zero() {
        // Not yet usable.
        assert_eq!(
            derive_boot_view(State::Idle, None, RoomsKnowledge::Unknown),
            BootView::Booting
        );
        assert_eq!(
            derive_boot_view(State::Connecting, Some(&present()), RoomsKnowledge::Unknown),
            BootView::Booting
        );
        // Ready but no snapshot yet — still unknown, never "0 rooms".
        assert_eq!(
            derive_boot_view(State::Ready, None, RoomsKnowledge::Unknown),
            BootView::Booting
        );
        // Ready + present subject but room list unanswered — unknown, not zero.
        assert_eq!(
            derive_boot_view(State::Ready, Some(&present()), RoomsKnowledge::Unknown),
            BootView::Booting
        );
    }

    #[test]
    fn absent_subject_starts_identity_onboarding() {
        assert_eq!(
            derive_boot_view(State::Ready, Some(&absent()), RoomsKnowledge::Unknown),
            BootView::Onboarding(OnboardStep::Identity)
        );
    }

    #[test]
    fn present_subject_with_zero_rooms_starts_rooms_onboarding() {
        assert_eq!(
            derive_boot_view(
                State::Ready,
                Some(&present()),
                RoomsKnowledge::Loaded { count: 0 }
            ),
            BootView::Onboarding(OnboardStep::Rooms)
        );
    }

    #[test]
    fn present_subject_with_rooms_or_failed_read_mounts_the_shell() {
        assert_eq!(
            derive_boot_view(
                State::Ready,
                Some(&present()),
                RoomsKnowledge::Loaded { count: 3 }
            ),
            BootView::Shell
        );
        // A failed room.list is a shell-time error, not onboarding, not zero.
        assert_eq!(
            derive_boot_view(State::Ready, Some(&present()), RoomsKnowledge::Failed),
            BootView::Shell
        );
        // Interrupted keeps the shell mounted.
        assert_eq!(
            derive_boot_view(
                State::Interrupted,
                Some(&present()),
                RoomsKnowledge::Loaded { count: 1 }
            ),
            BootView::Shell
        );
    }

    #[test]
    fn terminal_states_fail_with_a_way_forward() {
        assert_eq!(
            derive_boot_view(State::Stopping, None, RoomsKnowledge::Unknown),
            BootView::Failed(FailureView::Stopping)
        );
        assert_eq!(
            derive_boot_view(State::Stopped, None, RoomsKnowledge::Unknown),
            BootView::Failed(FailureView::Stopped)
        );
        assert_eq!(
            derive_boot_view(State::Failed, Some(&present()), RoomsKnowledge::Unknown),
            BootView::Failed(FailureView::Failed)
        );
    }

    #[test]
    fn recovery_overlays_without_replacing_the_view() {
        let anomalies = vec![SchemaAnomaly {
            key: "jeliya.dx.v1.selfLabel".to_owned(),
            reason: crate::prefs::AnomalyReason::Corrupt,
        }];
        let boot = derive_boot(
            State::Ready,
            Some(&present()),
            RoomsKnowledge::Loaded { count: 2 },
            &anomalies,
        );
        // The shell stays usable; the recovery banner rides alongside it.
        assert_eq!(boot.view, BootView::Shell);
        assert_eq!(boot.recovery, Some(RecoveryView { reset_count: 1 }));
    }

    #[test]
    fn no_recovery_when_there_are_no_anomalies() {
        let boot = derive_boot(
            State::Ready,
            Some(&present()),
            RoomsKnowledge::Loaded { count: 0 },
            &[],
        );
        assert_eq!(boot.recovery, None);
        assert_eq!(boot.view, BootView::Onboarding(OnboardStep::Rooms));
    }

    #[test]
    fn interrupted_with_absent_subject_still_onboards_identity() {
        // `Interrupted` (was Ready, recovering) keeps the shell-capable path,
        // so an absent subject still starts onboarding — not a booting unknown.
        assert_eq!(
            derive_boot_view(State::Interrupted, Some(&absent()), RoomsKnowledge::Unknown),
            BootView::Onboarding(OnboardStep::Identity),
        );
    }

    #[test]
    fn interrupted_with_failed_room_list_mounts_the_shell() {
        // A failed room.list during recovery is a shell-time error, not onboarding.
        assert_eq!(
            derive_boot_view(State::Interrupted, Some(&present()), RoomsKnowledge::Failed),
            BootView::Shell,
        );
    }

    #[test]
    fn multiple_anomalies_produce_correct_reset_count() {
        let anomalies = vec![
            SchemaAnomaly {
                key: "jeliya.dx.v1.selfLabel".to_owned(),
                reason: crate::prefs::AnomalyReason::Corrupt,
            },
            SchemaAnomaly {
                key: "jeliya.dx.v1.textLocale".to_owned(),
                reason: crate::prefs::AnomalyReason::UnsupportedNewer,
            },
            SchemaAnomaly {
                key: "jeliya.dx.v1.lastRoom".to_owned(),
                reason: crate::prefs::AnomalyReason::UnsupportedOlder,
            },
        ];
        let rv = RecoveryView::from_anomalies(&anomalies);
        assert_eq!(rv, Some(RecoveryView { reset_count: 3 }));
    }

    #[test]
    fn stopping_wins_over_any_identity_or_room_state() {
        // A terminal state must win regardless of snapshot/rooms content.
        assert_eq!(
            derive_boot_view(
                State::Stopping,
                Some(&present()),
                RoomsKnowledge::Loaded { count: 5 }
            ),
            BootView::Failed(FailureView::Stopping),
        );
        assert_eq!(
            derive_boot_view(
                State::Failed,
                Some(&absent()),
                RoomsKnowledge::Loaded { count: 1 }
            ),
            BootView::Failed(FailureView::Failed),
        );
    }
}
