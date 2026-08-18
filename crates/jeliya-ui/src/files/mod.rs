//! Host-testable **Files** orchestration (spec §5.B): the pure decisions that
//! drive the `ClientHandle` operations and the `PlatformServices` file
//! capabilities. Every unit here is target-agnostic (desktop #184 / Android
//! #192 reuse them behind their own bindings) and holds no `web-sys` / renderer
//! type.
//!
//! - [`list`] — the file-list model with truthful, non-inferred availability (D2).
//! - [`size`] — the served size limit and preflight (D3); no baked number.
//! - [`preview`] — the safe-preview classifier (D6); `declared_content_type` is
//!   never a render authorization.
//! - [`errors`] — the typed, redacting failure classification (§6/D7).
//! - [`transfer`] — progress + the cancel-reaches-both-copy-and-wire glue (D5).
//! - [`upload`] / [`fetch`] — the two flow state machines.

pub mod errors;
pub mod fetch;
pub mod list;
pub mod preview;
pub mod size;
pub mod transfer;
pub mod upload;

pub use errors::{FlowFailure, FlowStop, Settled};
pub use fetch::run_fetch_export;
pub use list::{FileListModel, FileRowView, ProviderView};
pub use preview::{classify as classify_preview, PreviewPolicy};
pub use size::{preflight as preflight_size, OverLimit, ServedLimit, SizeLimit};
pub use transfer::Progress;
pub use upload::{run_upload, UploadDone, UploadPhase};

/// The load state of a paged read — *booting is unknown, not zero* (spec D8).
/// Before a list answers, the pane renders `Loading`, never an empty state; a
/// failed read is `Failed`, never a silent empty pane.
#[derive(Clone, PartialEq, Debug)]
pub enum Load<T> {
    /// The read has not answered yet.
    Loading,
    /// The read answered.
    Loaded(T),
    /// The read failed with a typed, redacting reason (offer Retry).
    Failed(FlowFailure),
}

impl<T> Load<T> {
    /// The loaded value, if any.
    pub fn loaded(&self) -> Option<&T> {
        match self {
            Load::Loaded(value) => Some(value),
            _ => None,
        }
    }

    /// Classify a `Result` from a list read into a load state.
    pub fn from_result<E>(result: Result<T, E>, classify: impl FnOnce(E) -> FlowFailure) -> Self {
        match result {
            Ok(value) => Load::Loaded(value),
            Err(error) => Load::Failed(classify(error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_loaded_returns_inner_value_or_none() {
        // Loading and Failed yield None; only Loaded carries a value.
        assert_eq!(Load::<u32>::Loading.loaded(), None);
        assert_eq!(Load::Loaded(42u32).loaded(), Some(&42));
        assert_eq!(Load::<u32>::Failed(FlowFailure::Wire).loaded(), None);
    }

    #[test]
    fn load_from_result_classifies_ok_and_err() {
        let ok: Load<u32> = Load::from_result(Ok(99), |_: ()| FlowFailure::Wire);
        assert_eq!(ok, Load::Loaded(99));
        let failed: Load<u32> = Load::from_result(Err(()), |_| FlowFailure::Transport);
        assert_eq!(failed, Load::Failed(FlowFailure::Transport));
    }
}
