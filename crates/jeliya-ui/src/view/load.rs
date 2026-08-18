//! [`LoadState`] — the truthful states every #180 read surface renders (§5.3).
//!
//! A pane's read lifecycle collapses to exactly one of these; the RSX component
//! matches all arms, so a booting pane shows **Loading** (never a spurious
//! Empty), a zero answer shows **Empty** (only after the daemon answered), a
//! transient disconnect shows **Offline** (a way back), and a genuine refusal
//! shows **Failed** with friendly copy plus the raw scrubbed detail routed to
//! Diagnostics. `Unauthorized` is itself, never an empty room.
//!
//! This is the pane analogue of `app.rs`'s `room.list` discipline lifted into a
//! reusable enum, so each pane's `use_future` folds its `CallError` outcomes
//! into one value the render is total over.

use jeliya_client::{CallError, Execution};

use crate::l10n::FriendlyError;

/// The truthful state of one bounded read.
///
/// `T` is the pane's already-folded view model (a `RosterView`, `FleetView`,
/// …), not the raw wire reply, so a component never re-folds in render.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum LoadState<T> {
    /// The read is in flight and has not answered yet — shown before the first
    /// reply, never collapsed to Empty.
    Loading,
    /// The daemon answered with content.
    Loaded(T),
    /// The daemon answered, and the answer is empty (a real "nothing here").
    Empty,
    /// The connection dropped mid-read; the pane can recover on reconnect. A
    /// distinct state from `Failed` so the copy may honestly promise a retry.
    Offline,
    /// A previously loaded value is being shown while a refresh could not
    /// complete — the data is real but possibly out of date (used by the
    /// resume-once Fleet poll and any refresh path).
    Stale(T),
    /// A terminal failure: friendly copy for primary display plus the raw,
    /// secret-scrubbed detail for the Diagnostics disclosure.
    Failed(FriendlyError),
    /// The daemon refused the read for authorization reasons — rendered as
    /// itself, never as an empty roster/fleet.
    Unauthorized,
}

impl<T> LoadState<T> {
    /// The value when loaded (fresh or stale), else `None`.
    pub fn value(&self) -> Option<&T> {
        match self {
            LoadState::Loaded(v) | LoadState::Stale(v) => Some(v),
            _ => None,
        }
    }

    /// Whether this state carries a real, displayable value.
    pub fn has_value(&self) -> bool {
        self.value().is_some()
    }
}

/// How a failed read should be classified when it is an **idempotent read**
/// (every #180 read is): a `Disconnected`/`Cancelled` that never committed is a
/// recoverable [`ReadOutcome::Offline`]; anything else is terminal.
///
/// Kept as a separate pure classifier (not folded into `LoadState`) so a pane's
/// `use_future` can decide whether to **park-and-retry** (Offline) or **record
/// once** (Failed/Unauthorized) exactly as `app.rs` does for `room.list`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ReadOutcome {
    /// A transient disconnect on an idempotent read — park on a fresh
    /// subscription and retry after the next `Ready`.
    Offline,
    /// A terminal failure — record once, never retried.
    Failed,
    /// A terminal authorization refusal — record once as `Unauthorized`.
    Unauthorized,
}

impl ReadOutcome {
    /// Classify a read error. A `Disconnected` whose execution is not
    /// `Definitely` is a recoverable offline (the read never committed, so a
    /// retry is safe and correct); an authorization wire refusal is
    /// `Unauthorized`; everything else is a terminal `Failed`.
    ///
    /// `is_unauthorized` is supplied by the caller because the closed
    /// `ApiError` authorization codes live in `jeliya-api` and the seam carries
    /// the typed payload through [`CallError::as_wire`]; this keeps the
    /// classifier free of a hard dependency on the error-code spelling.
    pub fn classify(error: &CallError, is_unauthorized: bool) -> ReadOutcome {
        match error {
            // A dropped idempotent read that did not definitely commit is
            // recoverable; if it somehow committed (`Definitely`) there is no
            // recovery owed here — treat as terminal.
            CallError::Disconnected { execution } | CallError::Cancelled { execution }
                if *execution != Execution::Definitely =>
            {
                ReadOutcome::Offline
            }
            _ if is_unauthorized => ReadOutcome::Unauthorized,
            _ => ReadOutcome::Failed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loading_is_never_a_value() {
        let s: LoadState<u8> = LoadState::Loading;
        assert!(!s.has_value());
        assert_eq!(s.value(), None);
        assert_eq!(LoadState::Empty::<u8>.value(), None);
    }

    #[test]
    fn loaded_and_stale_expose_the_value() {
        assert_eq!(LoadState::Loaded(7u8).value(), Some(&7));
        assert_eq!(LoadState::Stale(7u8).value(), Some(&7));
        assert!(LoadState::Loaded(7u8).has_value());
        assert!(LoadState::Stale(7u8).has_value());
    }

    #[test]
    fn a_transient_disconnect_on_a_read_is_offline() {
        let offline = ReadOutcome::classify(
            &CallError::Disconnected {
                execution: Execution::Unknown,
            },
            false,
        );
        assert_eq!(offline, ReadOutcome::Offline);
        // DefinitelyNot is likewise recoverable (never committed).
        let unsent = ReadOutcome::classify(
            &CallError::Disconnected {
                execution: Execution::DefinitelyNot,
            },
            false,
        );
        assert_eq!(unsent, ReadOutcome::Offline);
    }

    #[test]
    fn an_authorization_refusal_is_unauthorized_not_empty() {
        let out = ReadOutcome::classify(&CallError::Timeout, true);
        assert_eq!(out, ReadOutcome::Unauthorized);
    }

    #[test]
    fn other_errors_are_terminal_failed() {
        let out = ReadOutcome::classify(&CallError::Timeout, false);
        assert_eq!(out, ReadOutcome::Failed);
        // A Disconnected that DID commit is not a recoverable read here.
        let committed = ReadOutcome::classify(
            &CallError::Disconnected {
                execution: Execution::Definitely,
            },
            false,
        );
        assert_eq!(committed, ReadOutcome::Failed);
    }

    #[test]
    fn a_cancelled_definitely_is_terminal_failed_not_offline() {
        // Cancelled::Definitely means the mutation may have committed; treating
        // it as Offline (park-and-retry) would risk a duplicate idempotent read
        // answer on reconnect.  It must be terminal Failed.
        let out = ReadOutcome::classify(
            &CallError::Cancelled {
                execution: Execution::Definitely,
            },
            false,
        );
        assert_eq!(out, ReadOutcome::Failed);
    }

    #[test]
    fn a_cancelled_not_definitely_is_recoverable_offline() {
        // Not-definitely-executed cancellations are safe to retry.
        for exec in [Execution::Unknown, Execution::DefinitelyNot] {
            let out = ReadOutcome::classify(&CallError::Cancelled { execution: exec }, false);
            assert_eq!(
                out,
                ReadOutcome::Offline,
                "Cancelled({exec:?}) should be Offline (retry-safe)"
            );
        }
    }
}
