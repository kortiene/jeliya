//! Codec error types. A refusal is either a bare error body (pre-upgrade,
//! at the gate) or a correlated error reply (post-upgrade), and the two are
//! never confused: a refused upgrade has no request `id` to correlate.

use jeliya_api::ApiError;
use thiserror::Error;

/// The ways a frame or an upgrade can be refused at the codec edge.
#[derive(Debug, Error)]
pub enum CodecError {
    /// The frame exceeds the hard byte ceiling and closes `4005` unparsed.
    #[error("frame exceeds {limit_bytes} bytes")]
    FrameTooLarge {
        /// The served frame limit.
        limit_bytes: u64,
    },

    /// The frame's `id` cannot be recovered, so no reply can be correlated.
    /// Closes the connection `4007` (`malformed_frame`) — there is nothing
    /// to correlate a reply to.
    #[error("malformed frame with no recoverable id: {0}")]
    UnrecoverableId(String),

    /// The frame decodes far enough to carry an `id`, so it gets a
    /// correlated error reply instead of a close — and the transport needs
    /// that id to build the `{id, ok:false, err}` reply without reparsing
    /// JSON outside the codec. Carries the recovered id and the error.
    #[error("malformed frame (id {id}): {error:?}")]
    Malformed {
        /// The recovered request id, for the correlated reply.
        id: u64,
        /// The typed error to reply with.
        error: ApiError,
    },

    /// The upgrade failed the generation gate. Carries the bare error body
    /// to return with the matching HTTP status.
    #[error("gate refusal: {0:?}")]
    GateRefused(GateRejection),
}

/// A gate rejection: the bare `err` object and the HTTP status to return it
/// with. One error format, everywhere.
#[derive(Debug, Clone)]
pub struct GateRejection {
    /// The bare error body (the `err` object, not an RPC envelope — no
    /// request id exists yet).
    pub body: ApiError,
    /// The HTTP status: 403, 401, 426, or 503.
    pub status: u16,
}
