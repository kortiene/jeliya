//! The call error taxonomy (#167 §D7).
//!
//! This is the correctness heart of the seam: *separate wire errors from
//! queue / timeout / cancel / gap / local failures, and preserve whether a
//! failed mutation may have executed.*
//!
//! A [`CallError`] answers two questions a UI must not conflate:
//!
//! 1. **Did the daemon reach operation semantics and say no?** Only
//!    [`CallError::Wire`] means that. Queue pressure, timeout, cancellation,
//!    disconnect, and local encode/decode are never dressed up as a wire
//!    error, so a component can branch on "the daemon said no (localizable,
//!    actionable)" versus "the round trip could not complete."
//! 2. **May a failed mutation have taken effect?** [`CallError::execution`]
//!    is a total function returning an [`Execution`] for every variant, so the
//!    "preserve whether a failed mutation may have executed" guarantee is a
//!    type-level fact rather than a comment. #168's "connection loss
//!    distinguishes never-sent work from work that may have executed" flows
//!    through this one classification.

use jeliya_api::ApiError;

/// Whether a failed mutation may have taken effect on the daemon.
///
/// Only [`Execution::Unknown`] permits a retry, and only under an explicit,
/// tested dedup guarantee (#168) — the seam never retries on the caller's
/// behalf.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Execution {
    /// The request provably never executed: it was never sent, or the daemon
    /// answered a typed refusal that commits no effect.
    DefinitelyNot,
    /// The request may or may not have executed; the client cannot tell. A
    /// retry is safe only under an explicit, tested dedup guarantee.
    Unknown,
    /// The request provably executed — the daemon replied and only the reply
    /// was unreadable locally.
    Definitely,
}

/// A client-local failure that never involved a daemon verdict.
#[derive(Clone, Copy, PartialEq, Eq, Debug, thiserror::Error)]
pub enum LocalError {
    /// The request could not be encoded. `execution()` is
    /// [`Execution::DefinitelyNot`] — nothing left the seam.
    #[error("request could not be encoded")]
    EncodeRequest,
    /// A reply was received but could not be decoded into `O::Output`.
    /// `execution()` is [`Execution::Definitely`] — the daemon *did* run the
    /// operation; only the local decode failed.
    #[error("reply could not be decoded into the operation's output type")]
    DecodeReply,
    /// A backend-internal invariant failed (a bug surface). `execution()` is
    /// [`Execution::Unknown`], because a bug near the send/receive boundary
    /// cannot honestly claim otherwise.
    #[error("backend invariant failed")]
    Backend,
}

/// Why one [`ClientHandle::call`](crate::ClientHandle::call) did not return a
/// typed output.
///
/// The variants are the closed set of failure classes the seam distinguishes.
/// See [`CallError::execution`] for the may-have-executed classification each
/// carries.
#[derive(Clone, PartialEq, Debug, thiserror::Error)]
pub enum CallError {
    /// The daemon reached operation semantics and answered a typed protocol
    /// error, carried verbatim as a [`jeliya_api::ApiError`]. `execution()` is
    /// [`Execution::DefinitelyNot`]: a typed refusal commits no effect (a
    /// pre-`END` stream failure authors no `file.share` event, an
    /// `op_id_conflict` performed no second effect, and so on).
    #[error("daemon returned a typed error: {0:?}")]
    Wire(ApiError),
    /// The bounded outbound queue refused the request before it was sent
    /// (#168's backpressure, surfaced rather than absorbed). `execution()` is
    /// [`Execution::DefinitelyNot`].
    #[error("outbound queue full: {resource} at limit {limit}")]
    QueueFull {
        /// The exhausted resource (a served limit name).
        resource: &'static str,
        /// The limit that was reached.
        limit: u64,
    },
    /// No reply arrived within the call deadline. `execution()` is
    /// [`Execution::Unknown`] — a request that timed out may still land.
    #[error("call deadline elapsed with no reply")]
    Timeout,
    /// The caller cancelled, or [`stop`](crate::ClientHandle::stop) drained the
    /// request. `execution()` is the carried [`Execution`]:
    /// [`Execution::DefinitelyNot`] if it was never sent,
    /// [`Execution::Unknown`] if bytes had gone out.
    #[error("call cancelled")]
    Cancelled {
        /// Whether the cancelled request may have executed.
        execution: Execution,
    },
    /// The connection was lost before a reply. `execution()` is the carried
    /// [`Execution`]: [`Execution::DefinitelyNot`] if the request never left
    /// the queue, [`Execution::Unknown`] if bytes had gone out.
    #[error("connection lost before reply")]
    Disconnected {
        /// Whether the in-flight request may have executed.
        execution: Execution,
    },
    /// A client-local failure that never reached a daemon verdict.
    #[error("local failure: {0}")]
    Local(LocalError),
}

impl CallError {
    /// The may-have-executed classification for this error. Total: every
    /// variant answers, so callers can reason about mutation safety without a
    /// fallible match.
    pub fn execution(&self) -> Execution {
        match self {
            // A typed daemon verdict commits no effect.
            CallError::Wire(_) => Execution::DefinitelyNot,
            // Refused before a byte was sent.
            CallError::QueueFull { .. } => Execution::DefinitelyNot,
            // Sent, then silence — it may have landed.
            CallError::Timeout => Execution::Unknown,
            // The carried value is authoritative for cancel and disconnect.
            CallError::Cancelled { execution } => *execution,
            CallError::Disconnected { execution } => *execution,
            CallError::Local(local) => match local {
                LocalError::EncodeRequest => Execution::DefinitelyNot,
                LocalError::DecodeReply => Execution::Definitely,
                LocalError::Backend => Execution::Unknown,
            },
        }
    }

    /// The wire error, if this was a daemon verdict. `None` for every
    /// non-[`CallError::Wire`] variant, so a component can localize a daemon
    /// refusal without misreading a transport failure as one.
    pub fn as_wire(&self) -> Option<&ApiError> {
        match self {
            CallError::Wire(error) => Some(error),
            _ => None,
        }
    }
}

impl From<LocalError> for CallError {
    fn from(error: LocalError) -> Self {
        CallError::Local(error)
    }
}
