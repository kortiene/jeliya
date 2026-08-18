//! The in-process struct↔struct router — the one residual `serde` in the direct
//! path, named honestly (§4).
//!
//! The shared [`ClientHandle`](crate::ClientHandle) erases every call to
//! [`RawJson`](crate::backend::RawJson) text at its edge; that erasure is a
//! fixed property of the seam, identical for all four adapters, and cannot be
//! removed for DirectClient without changing the shared contract for the other
//! three. So the direct path reuses the daemon's **own** in-process router to
//! turn the erased `{ op, input: RawJson }` back into a typed
//! [`TypedCall`](jeliya_core::typed::TypedCall), and re-serializes the typed
//! reply the same way the daemon does — a struct↔struct transform that produces
//! **no bytes**, touches no socket, and is exactly the transform the shared
//! handle already forces at its edge. This is what makes "adapter contract
//! tests match WS view-level outcomes" true *by construction*: the same
//! `route` → `resolve_call` → `execute_with` → `serde_json::to_string` pipeline
//! the daemon runs (`jeliyad/src/serve.rs`).
//!
//! No `serde_json::Value` type path appears in this file; the router's argument
//! type is inferred from `route`'s signature, exactly as `error.rs` infers the
//! reply of `serde_json::to_value`, so the no-`Value`-in-source boundary holds.

use jeliya_api::ApiError;
use jeliya_core::typed::{resolve_call, TypedCall, TypedReply};

/// Turn one erased `{ op, input }` request into the typed call the engine
/// executes, running exactly the daemon's routing:
///
/// 1. parse the erased `in` text — a parse failure is `MalformedFrame`;
/// 2. `route(op, value)` — an unknown op or an invalid argument is the typed
///    `ApiError` the router returns, surfaced without touching the engine;
/// 3. `resolve_call(op, call.input_any())` — a shape the typed layer cannot
///    accept is `MalformedFrame`.
///
/// The returned error is settled as `CallError::Wire` on the caller's future,
/// exactly as the same failure would be on the wire.
pub(crate) fn resolve(op: &'static str, input: &str) -> Result<TypedCall, ApiError> {
    // The bound type is inferred from `route`'s parameter, so the `Value` token
    // never appears in source (the no-`serde_json::Value` boundary, §6).
    let value = serde_json::from_str(input).map_err(|_| ApiError::MalformedFrame)?;
    let call = jeliya_codec::route(op, value)?;
    resolve_call(op, call.input_any()).ok_or(ApiError::MalformedFrame)
}

/// Encode a typed reply back into the erased success `out` text the shared
/// handle decodes into `O::Output`. `TypedReply` is `#[serde(untagged)]`, so its
/// serialization is byte-identical to the concrete `O::Output` — the very value
/// the daemon puts on the wire via `serde_json::to_value(out)` — which is why
/// the direct and WebSocket adapters produce the same reply shapes. A
/// serialization failure (unreachable for the typed outputs) degrades to
/// `MalformedFrame` rather than fabricating a success.
pub(crate) fn encode_reply(reply: &TypedReply) -> Result<String, ApiError> {
    serde_json::to_string(reply).map_err(|_| ApiError::MalformedFrame)
}

#[cfg(test)]
mod tests {
    use jeliya_api::ApiError;

    use super::resolve;

    /// A valid, known op with a well-formed request body must resolve to a
    /// `TypedCall` without touching the engine (steps 1–3, §5.3).
    #[test]
    fn resolve_known_op_with_valid_input_succeeds() {
        assert!(
            resolve("room.list", "{}").is_ok(),
            "room.list with an empty object body is a valid typed call"
        );
    }

    /// An operation name the routing table does not recognise must fail at
    /// step 2 (`route`), before the engine is consulted. The error must not
    /// be `MalformedFrame` — the frame is structurally sound; the op is
    /// simply absent.
    #[test]
    fn resolve_unknown_op_returns_routing_error_not_malformed_frame() {
        let err =
            resolve("__no_such_op_jeliya_direct_test__", "{}").expect_err("unknown op must fail");
        assert_ne!(
            err,
            ApiError::MalformedFrame,
            "an unknown op is a routing error (UnknownOperation), not MalformedFrame"
        );
    }

    /// Non-JSON input must be caught at step 1 (parse) and surfaced as
    /// `MalformedFrame`, before routing or engine dispatch. This guards the
    /// shared `ClientHandle`-side serialization round-trip: if the erased text
    /// it produces cannot be parsed, the error category must be correct.
    #[test]
    fn resolve_malformed_json_returns_malformed_frame() {
        assert!(
            matches!(
                resolve("room.list", "not { valid } json {{{"),
                Err(ApiError::MalformedFrame)
            ),
            "a non-JSON input body must be MalformedFrame, not a routing or engine error"
        );
    }

    /// Null input (valid JSON, wrong type for the op body) must also fail
    /// before the engine is consulted, not panic.
    #[test]
    fn resolve_null_input_for_typed_op_fails_cleanly() {
        // `null` is valid JSON but is not an object — the router rejects it.
        let result = resolve("room.list", "null");
        assert!(
            result.is_err(),
            "null is not a valid room.list body: {result:?}"
        );
    }
}
