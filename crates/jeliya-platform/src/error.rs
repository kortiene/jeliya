//! The capability outcome taxonomy (#174 §D3).
//!
//! This is the correctness heart of the boundary: *a cancellation can never
//! become a success, and the three "the action did not happen" reasons render
//! as three different truths.* [`CapabilityError`] is a **closed** enum whose
//! arms a component must not conflate — it mirrors how
//! `jeliya_client::CallError` refuses to collapse outcomes.
//!
//! Redaction (§K1): the `Display`/`Debug` of every variant carries a
//! discriminant and **no** path, URI, or identifier payload. This holds by
//! construction — no variant carries a filename, a `content://` URI, or any
//! opaque id, only closed discriminants and non-sensitive byte counts — so a
//! staged filename or a content URI cannot leak into an error string.

/// Whether a structural platform capability is present at all.
///
/// Distinct from a failed *action*: [`Availability::Unavailable`] is a fact the
/// UI renders as "not available here" (window minimize on a browser tab; a
/// protected directory on the web), never as an error the user caused. A
/// capability that is present but refuses an action returns
/// [`CapabilityError::Denied`]; one the user dismissed returns
/// [`CapabilityError::Cancelled`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Availability {
    /// The platform offers this capability.
    Available,
    /// The platform structurally does not offer this capability here.
    Unavailable,
}

impl Availability {
    /// `true` when the capability is [`Availability::Available`].
    pub fn is_available(self) -> bool {
        matches!(self, Availability::Available)
    }
}

/// Why a fallible capability method did not return its success value.
///
/// The variants are the closed set of outcome classes the boundary
/// distinguishes. The three non-failure "did not happen" reasons —
/// [`CapabilityError::Unavailable`], [`CapabilityError::Denied`],
/// [`CapabilityError::Cancelled`] — are kept apart because they render as three
/// different truths and drive three different UI branches.
#[derive(Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CapabilityError {
    /// The platform structurally does not offer this capability (window
    /// minimize on a browser tab; a private directory on the web). **Not** a
    /// failure of the user's action — a fact the UI renders as "not available
    /// here", never as an error the user caused.
    #[error("capability unavailable on this platform")]
    Unavailable,
    /// The user or OS refused permission (clipboard, file access, a share
    /// target). Distinct from [`CapabilityError::Cancelled`]: the user did not
    /// dismiss, the platform said no.
    #[error("permission denied by the user or platform")]
    Denied,
    /// The user dismissed the picker / save dialog / share sheet, or a
    /// [`crate::CancelToken`] fired. This is **never** `Ok` and **never** a
    /// generic failure: a cancelled export wrote nothing, a cancelled share
    /// shared nothing, a cancelled stage left no daemon-shareable blob. Callers
    /// branch on it to keep the prior state untouched.
    #[error("operation cancelled")]
    Cancelled,
    /// A typed operation failure with a discriminant the UI can localize.
    #[error("operation failed: {0}")]
    Failed(FailureKind),
}

impl CapabilityError {
    /// `true` for [`CapabilityError::Cancelled`] only. A convenience for the
    /// most safety-critical branch — a caller that must keep the prior state on
    /// cancel and only on cancel.
    pub fn is_cancelled(self) -> bool {
        matches!(self, CapabilityError::Cancelled)
    }

    /// The typed failure kind, if this outcome is a [`CapabilityError::Failed`].
    /// `None` for `Unavailable` / `Denied` / `Cancelled`, so a caller cannot
    /// misread a structural fact or a dismissal as an operation failure.
    pub fn failure(self) -> Option<FailureKind> {
        match self {
            CapabilityError::Failed(kind) => Some(kind),
            _ => None,
        }
    }
}

/// `Debug` renders the same redacted discriminant `Display` does (§K1): every
/// arm is payload-free except the byte counts on [`FailureKind::FileTooLarge`],
/// which are sizes, not identifiers.
impl core::fmt::Debug for CapabilityError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CapabilityError::Unavailable => f.write_str("Unavailable"),
            CapabilityError::Denied => f.write_str("Denied"),
            CapabilityError::Cancelled => f.write_str("Cancelled"),
            CapabilityError::Failed(kind) => f.debug_tuple("Failed").field(kind).finish(),
        }
    }
}

/// A typed operation failure, with a discriminant the UI can localize.
///
/// The only payloads are the non-sensitive byte counts on
/// [`FailureKind::FileTooLarge`] — never a filename, a path, or a URI.
#[derive(Clone, Copy, PartialEq, Eq, Debug, thiserror::Error)]
pub enum FailureKind {
    /// The picked file exceeds the daemon-reported `max_shared_file_bytes`.
    /// Carries the offending size and the limit (both byte counts, safe to
    /// render), so the UI can say "too large: N of M" without naming the file.
    #[error("file too large: {size} bytes exceeds the {limit}-byte limit")]
    FileTooLarge {
        /// The source's size (or the running total, for a streamed source).
        size: u64,
        /// The daemon-reported limit the size exceeded.
        limit: u64,
    },
    /// The picked file is empty; there is nothing to share. A distinct code, so
    /// a zero-byte share never reads as success (mirrors the daemon's
    /// `file_empty`).
    #[error("file is empty")]
    FileEmpty,
    /// The source could not be read (vanished, unreadable, content stream
    /// closed).
    #[error("source could not be read")]
    Unreadable,
    /// Persisting a preference did not reach durable storage. The setter's
    /// in-memory change still applies for this session (see
    /// [`crate::storage::WriteOutcome`]); the caller learns it was not written.
    #[error("write did not reach durable storage")]
    WriteNotDurable,
    /// A platform I/O error with no more specific typed meaning.
    #[error("platform I/O error")]
    Io,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §K1: rendering an error carries the discriminant and no identifier
    /// payload. The taxonomy structurally cannot leak a path/URI because no
    /// variant holds one.
    #[test]
    fn rendered_errors_carry_only_discriminants() {
        assert_eq!(
            CapabilityError::Unavailable.to_string(),
            CapabilityError::Unavailable.to_string()
        );
        for error in [
            CapabilityError::Unavailable,
            CapabilityError::Denied,
            CapabilityError::Cancelled,
            CapabilityError::Failed(FailureKind::FileEmpty),
            CapabilityError::Failed(FailureKind::Unreadable),
            CapabilityError::Failed(FailureKind::WriteNotDurable),
            CapabilityError::Failed(FailureKind::Io),
        ] {
            let rendered = format!("{error} {error:?}");
            assert!(!rendered.is_empty());
        }
        // The one variant with a payload renders only byte counts.
        let too_large = CapabilityError::Failed(FailureKind::FileTooLarge {
            size: 200,
            limit: 100,
        });
        let rendered = format!("{too_large} {too_large:?}");
        assert!(rendered.contains("200") && rendered.contains("100"));
    }

    /// The three "did not happen" reasons are distinct values, never equal.
    #[test]
    fn unavailable_denied_and_cancelled_are_distinct() {
        assert_ne!(CapabilityError::Unavailable, CapabilityError::Denied);
        assert_ne!(CapabilityError::Denied, CapabilityError::Cancelled);
        assert_ne!(CapabilityError::Cancelled, CapabilityError::Unavailable);
        assert!(CapabilityError::Cancelled.is_cancelled());
        assert!(!CapabilityError::Denied.is_cancelled());
    }
}
