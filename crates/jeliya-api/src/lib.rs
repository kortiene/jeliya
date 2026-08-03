//! Clean-slate typed API for Jeliya protocol v2.
//!
//! One crate holds the typed request for every approved v2 operation,
//! compile-time paired with its output, plus pushes, errors, and the
//! lifecycle-visible view models — and nothing else. The authoritative
//! statement of every shape is `docs/protocol-v2.md`; where this crate and
//! that record disagree, the record is right and this crate has a bug.
//!
//! Boundaries this crate holds by construction:
//!
//! - **No `serde_json::Value` in any public type.** JSON exists only at the
//!   WebSocket edge (#164's codec); untrusted values are never hidden in raw
//!   JSON here.
//! - **No Iroh, WebSocket, Dioxus, or platform dependency.** `wasm32` stays
//!   free of native transports so the browser client can link this crate.
//! - **Requests are paired with outputs at compile time.** Every operation
//!   implements [`Operation`], binding its [`Operation::Output`] and its
//!   [`Operation::PATH`] name; there is no unpaired downcast.
//! - **Closed vocabularies reject unknown values.** Protocol v2 never
//!   silently reclassifies an unrecognized wire value, so enums here have no
//!   `Unknown` arm: deserialization of an out-of-vocabulary value fails,
//!   which is the wire behavior the record requires.
//! - **No null carries meaning.** Absence is a tagged variant everywhere.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod ids;
mod ops;
mod push;
mod shared;
mod types;

pub use ids::{
    DeviceId, EventId, FileId, InviteId, OpId, PipeId, RequestId, RequestIdOutOfRange, RoomId,
    SubjectId, MAX_REQUEST_ID,
};
pub use ops::*;
pub use push::*;
pub use shared::*;
pub use types::*;

use serde::{de::DeserializeOwned, Serialize};

/// One approved protocol-v2 operation, binding the request type to the
/// output type the daemon pairs with it.
///
/// The pairing is the point of #163: a caller cannot hold a request without
/// knowing, at compile time, the exact reply shape the daemon commits to.
pub trait Operation: Serialize + DeserializeOwned + Sized {
    /// The reply shape this operation is paired with by the specification.
    type Output: Serialize + DeserializeOwned;

    /// The operation's wire name — a capability token is the name of the
    /// operation it authorizes, so this string *is* the capability
    /// vocabulary.
    const PATH: &'static str;

    /// Whether the operation mutates. Mutating operations accept and
    /// deduplicate an envelope `op_id` per the record's idempotency rules.
    const MUTATING: bool;
}

/// A request paired with its envelope metadata, ready for the codec (#164)
/// to serialize. `op_id` lives at the envelope level, never inside the
/// operation's `in` object.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Envelope<O: Operation> {
    /// Correlates the reply to this request; unique per connection while
    /// outstanding.
    pub id: RequestId,
    /// The operation's wire name — serialized as `op` so the envelope is
    /// dispatchable as-is. It is `O::PATH`, always.
    pub op: &'static str,
    /// Client-generated deduplication key. Accepted on every operation,
    /// ignored by those that do not deduplicate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub op_id: Option<OpId>,
    /// The operation's `in` object. Every field of every `in` is required by
    /// the record; `Option` appears nowhere in any request type in this crate.
    #[serde(rename = "in")]
    pub input: O,
}

impl<O: Operation> Envelope<O> {
    /// The wire name of the operation this envelope invokes.
    pub const OP: &'static str = O::PATH;

    /// Builds an envelope for a request, pinning `op` to the operation's
    /// wire name and rejecting ids outside the browser-safe range.
    pub fn new(id: u64, op_id: Option<OpId>, input: O) -> Result<Self, RequestIdOutOfRange> {
        Ok(Self {
            id: RequestId::new(id)?,
            op: O::PATH,
            op_id,
            input,
        })
    }
}
