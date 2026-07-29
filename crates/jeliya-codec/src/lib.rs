//! Protocol-v2 JSON/WebSocket codec — the edge between wire frames and
//! `jeliya-api` types.
//!
//! The codec owns the protocol's *only* JSON. Everything wire-side of this
//! crate is bytes; everything domain-side is a typed `jeliya-api` value. No
//! JSON or method-string dispatch escapes it.
//!
//! What the codec does, in the order the record fixes:
//!
//! 1. **Generation gate** — the upgrade is checked for loopback `Host` and
//!    `Origin`, a present-and-supported `v`, a present-and-equal `sg`, a
//!    credential, and connection capacity, in that order, **before** the
//!    upgrade is performed and before any frame is parsed or dispatched.
//! 2. **Bounded envelopes** — a frame over `max_frame_bytes` closes `4005`
//!    unparsed; nesting depth, `op` name length, and array lengths are
//!    bounded, and exceeding any is `invalid_argument`, never a panic and
//!    never an unbounded allocation.
//! 3. **Exhaustive request routing** — every approved operation decodes
//!    into its `jeliya-api` request type; an unknown `op` is
//!    `unknown_operation`, and an unknown request field is
//!    `invalid_argument` with `unrecognised_field`.
//! 4. **Bounded malformed-input handling** — a frame whose `id` cannot be
//!    recovered closes `4007` (`malformed_frame`); a frame that decodes far
//!    enough to correlate always gets a correlated error reply instead, so
//!    one bad request never strands the others in flight.
//!
//! What the codec deliberately does not do:
//!
//! - **No v1 facade and no dual codec.** One generation at a time.
//! - **No domain logic.** Authorization, execution, and storage live in the
//!   engine; the codec decodes, gates, and encodes.
//! - **No `hint` and no prose in errors.** v2 errors carry a machine-readable
//!   `code` plus typed fields; localization is the client's job. (The issue
//!   text's "errors retain code/message/hint" is v1 phrasing the canonical
//!   record overrides.)
//! - **No JSON key ordering guarantees.** `serde_json` with `preserve_order`
//!   keeps insertion order for golden vectors, but the wire contract is
//!   key-order-insensitive.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod error;
mod frame;
mod gate;
mod routing;

pub use error::{CodecError, GateRejection};
pub use frame::push_to_bytes;
pub use frame::{Frame, Reply};
pub use gate::{gate, GateDecision, GateParams, SessionPrincipal};

use jeliya_api::{ApiError, Operation};

/// The protocol generation this codec speaks. One generation at a time.
pub const PROTOCOL: u64 = 2;

/// The minimum supported protocol generation. v2 supports exactly one.
pub const MIN_PROTOCOL: u64 = 2;

/// The codec's own bounds, independent of the served limits: a frame is
/// parsed only after it is known to fit, and these cap the work a malformed
/// frame can cause. The *served* `max_frame_bytes` is the wire-visible limit;
/// these are the codec's hard ceilings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodecBounds {
    /// Hard ceiling on frame bytes, independent of the served limit.
    pub max_frame_bytes: usize,
    /// Maximum JSON nesting depth.
    pub max_depth: usize,
    /// Maximum `op` name length in bytes.
    pub max_op_len: usize,
    /// Maximum array length in any decoded value.
    pub max_array_len: usize,
}

impl Default for CodecBounds {
    fn default() -> Self {
        Self {
            // The shared-file policy fixes 100 MiB as the largest file; a
            // frame must fit one at-limit body plus envelope overhead.
            max_frame_bytes: 128 * 1024 * 1024,
            max_depth: 64,
            max_op_len: 64,
            max_array_len: 1 << 20,
        }
    }
}

/// Decode one frame's bytes into a typed [`Frame`], bounded.
///
/// Returns a [`Frame`] on success, or a [`CodecError`] describing the
/// refusal. A frame whose `id` cannot be recovered is
/// [`CodecError::UnrecoverableId`] and closes the connection `4007`; any
/// other malformed frame is [`CodecError::Malformed`] with a correlated
/// error reply to send instead, so one bad request never strands the others.
pub fn decode(bytes: &[u8], bounds: &CodecBounds) -> Result<Frame, CodecError> {
    if bytes.len() > bounds.max_frame_bytes {
        return Err(CodecError::FrameTooLarge {
            limit_bytes: bounds.max_frame_bytes as u64,
        });
    }
    frame::decode_frame(bytes, bounds)
}

/// Encode a reply frame for a completed operation.
///
/// `id` echoes the request's `id` exactly. `out` is the operation's typed
/// output; `err` is a typed [`ApiError`]. One of the two is present, never
/// both — `ok` discriminates.
pub fn encode_reply<O: Operation>(id: u64, result: Result<O::Output, ApiError>) -> Reply {
    Reply::from_result::<O>(id, result)
}

/// Encode a push frame.
pub fn encode_push(push: &jeliya_api::Push) -> Result<serde_json::Value, serde_json::Error> {
    serde_json::to_value(push)
}
