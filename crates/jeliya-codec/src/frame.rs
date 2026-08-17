//! Wire frames: request envelope, reply, push. A reply carries `id` and
//! never `t`; a push carries `t` and never `id`. That is the whole of how
//! the two are told apart, so a frame carrying both, or neither, is
//! `malformed_frame`.

use crate::error::CodecError;
use crate::routing;
use crate::{CodecBounds, MAX_REQUEST_ID};
use jeliya_api::{ApiError, OpId, Operation, Push};
use serde::{Deserialize, Serialize};

/// A decoded inbound frame.
#[derive(Debug)]
pub enum Frame {
    /// A request: an envelope carrying a typed operation input.
    Request(Request),
    /// An inbound frame that is not a request: it has no place in v2's
    /// client-to-daemon direction and is malformed. Carries the error.
    Malformed(ApiError),
}

/// A typed, decoded request.
#[derive(Debug)]
pub struct Request {
    /// The envelope id, echoed exactly in the reply.
    pub id: u64,
    /// The deduplication key, if the client sent one.
    pub op_id: Option<OpId>,
    /// The decoded operation input and its routing metadata.
    pub call: routing::Call,
}

/// A reply frame, ready to serialize. `ok` discriminates; `out` and `err`
/// are mutually exclusive.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reply {
    /// The echoed request id.
    pub id: u64,
    /// Whether the call succeeded.
    pub ok: bool,
    /// The operation's typed output, when `ok`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub out: Option<serde_json::Value>,
    /// The typed error, when not `ok`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub err: Option<ApiError>,
}

impl Reply {
    /// Builds a reply from an operation result.
    pub fn from_result<O: Operation>(id: u64, result: Result<O::Output, ApiError>) -> Self {
        match result {
            Ok(out) => Self {
                id,
                ok: true,
                out: serde_json::to_value(out).ok(),
                err: None,
            },
            Err(err) => Self {
                id,
                ok: false,
                out: None,
                err: Some(err),
            },
        }
    }

    /// Serializes the reply to wire bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("a reply is always serializable")
    }
}

/// Serializes a push to wire bytes.
pub fn push_to_bytes(push: &Push) -> Vec<u8> {
    serde_json::to_vec(push).expect("a push is always serializable")
}

/// The raw envelope, decoded structurally before routing. Unknown
/// *envelope* keys are tolerated (forward compatibility); unknown `in` keys
/// are `unrecognised_field` at routing.
#[derive(Debug, Deserialize)]
struct RawEnvelope {
    op: String,
    // Three states are load-bearing: the key omitted (None) is the only way
    // `op_id` is absent; an explicit JSON null is forbidden everywhere in v2
    // and must not silently become None. serde maps both to None by default,
    // so presence is checked on the raw map before this struct is used.
    #[serde(default)]
    op_id: Option<OpId>,
    #[serde(rename = "in")]
    input: serde_json::Value,
    #[serde(flatten)]
    _extra: serde_json::Map<String, serde_json::Value>,
}

/// Decodes one frame's bytes, bounded. See [`crate::decode`].
pub(crate) fn decode_frame(bytes: &[u8], bounds: &CodecBounds) -> Result<Frame, CodecError> {
    // Structural parse, bounded.
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| {
        // If we cannot parse at all, try to recover an id for correlation.
        match recover_id(bytes) {
            Some(id) => CodecError::Malformed {
                id,
                error: ApiError::MalformedFrame,
            },
            None => CodecError::UnrecoverableId(e.to_string()),
        }
    })?;

    // An `id` that is present but not a usable unsigned integer is no
    // correlation identifier at all — a null, string, negative, or float id
    // cannot be echoed, so the frame takes the unrecoverable-id close, not
    // the correlated-reply path.
    let value_ref = &value;
    let raw_id = value_ref.get("id");
    let id: u64 = match raw_id {
        Some(v) => v.as_u64().ok_or_else(|| {
            CodecError::UnrecoverableId("frame id is not a usable unsigned integer".into())
        })?,
        None => {
            return Err(CodecError::UnrecoverableId("frame carries no id".into()));
        }
    };
    if id > MAX_REQUEST_ID {
        return Err(CodecError::UnrecoverableId(format!(
            "frame id exceeds the browser-safe maximum {MAX_REQUEST_ID}"
        )));
    }

    // A frame with `t` is a push; pushes do not flow client-to-daemon. Its
    // id is already known usable, so this is the correlated path.
    if value.get("t").is_some() {
        return Err(CodecError::Malformed {
            id,
            error: ApiError::MalformedFrame,
        });
    }

    // An explicit JSON null op_id is forbidden: omission is the only way the
    // envelope's one optional field is absent, and a null must not silently
    // become None and change the request's deduplication semantics.
    if let Some(op_id_value) = value.get("op_id") {
        if op_id_value.is_null() {
            return Err(CodecError::Malformed {
                id,
                error: ApiError::InvalidArgument {
                    field: "op_id".into(),
                    reason: jeliya_api::InvalidReason::Type {
                        expected: "string".into(),
                    },
                },
            });
        }
    }

    check_bounds(value_ref, bounds, 0).map_err(|error| CodecError::Malformed { id, error })?;

    let env: RawEnvelope = serde_json::from_value(value).map_err(|_| CodecError::Malformed {
        id,
        error: ApiError::MalformedFrame,
    })?;

    if env.op.len() > bounds.max_op_len {
        return Err(CodecError::Malformed {
            id,
            error: ApiError::InvalidArgument {
                field: "op".into(),
                reason: jeliya_api::InvalidReason::Bound {
                    min: 1,
                    max: bounds.max_op_len as u64,
                },
            },
        });
    }

    let call =
        routing::route(&env.op, env.input).map_err(|error| CodecError::Malformed { id, error })?;

    Ok(Frame::Request(Request {
        id,
        op_id: env.op_id,
        call,
    }))
}

/// Tries to recover an `id` from unparseable bytes, for correlation.
fn recover_id(bytes: &[u8]) -> Option<u64> {
    // A shallow scan for `"id":<uint>` near the start of the frame. This is
    // a best-effort correlation, not a parse: a frame whose id cannot be
    // recovered closes 4007, so this must never produce a false positive.
    let text = std::str::from_utf8(bytes).ok()?;
    let start = text.find("\"id\"")? + 4;
    let rest = text[start..].trim_start_matches([':', ' ']);
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().ok().filter(|id| *id <= MAX_REQUEST_ID)
}

/// Walks a decoded value enforcing depth and array-length bounds. Shared by
/// the server request path and the client edge ([`crate::decode_client_frame`]),
/// so both directions apply one bounds policy.
pub(crate) fn check_bounds(
    value: &serde_json::Value,
    bounds: &CodecBounds,
    depth: usize,
) -> Result<(), ApiError> {
    if depth > bounds.max_depth {
        return Err(ApiError::InvalidArgument {
            field: "$".into(),
            reason: jeliya_api::InvalidReason::Bound {
                min: 0,
                max: bounds.max_depth as u64,
            },
        });
    }
    match value {
        serde_json::Value::Array(arr) => {
            if arr.len() > bounds.max_array_len {
                return Err(ApiError::InvalidArgument {
                    field: "$".into(),
                    reason: jeliya_api::InvalidReason::Bound {
                        min: 0,
                        max: bounds.max_array_len as u64,
                    },
                });
            }
            for v in arr {
                check_bounds(v, bounds, depth + 1)?;
            }
        }
        serde_json::Value::Object(map) => {
            for v in map.values() {
                check_bounds(v, bounds, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}
