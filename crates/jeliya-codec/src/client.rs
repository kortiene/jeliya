//! The **client edge**: the mirror of the server edge, for the client adapters
//! (#171 `WsWeb`, #172 `WsNative`, #173 `DirectClient`).
//!
//! The codec owns the protocol's *only* JSON, in both directions. The server
//! edge ([`crate::decode`] / [`crate::encode_reply`]) turns client→daemon
//! bytes into typed requests and typed outputs back into reply bytes. This
//! module is the opposite direction a client speaks: it **encodes an outbound
//! request envelope** and **decodes an inbound reply / push / hello frame**, so
//! an adapter never hand-rolls the wire's JSON.
//!
//! Envelope discrimination is fixed and identical to the server edge: a
//! **reply carries `id` and never `t`**; a **push carries `t` and never `id`**;
//! `hello` is a push-shaped frame tagged `t == "hello"`. A frame carrying both,
//! or neither, is [`ClientInbound::Malformed`].

use jeliya_api::{ApiError, Hello, InvalidReason, OpId, Push, MAX_REQUEST_ID};
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::error::CodecError;
use crate::{frame, CodecBounds};

/// Encode one outbound request envelope:
/// `{"id":…,"op":"…"[,"op_id":"…"],"in":<input_json>}`.
///
/// `input_json` is the caller's already-serialized `in` object (the kernel's
/// `RawJson`); it is spliced **verbatim** — never re-parsed or re-encoded — so
/// the client's exact bytes reach the wire and byte-accounting stays faithful.
/// The `op` name and `op_id` are JSON-string-escaped so the envelope is always
/// well-formed.
///
/// Fails closed rather than silently truncating: an `id` past [`MAX_REQUEST_ID`]
/// is [`CodecError::UnrecoverableId`], and an `op` longer than
/// `bounds.max_op_len` is [`CodecError::Malformed`]. In practice the kernel
/// only ever supplies a validated [`jeliya_api::RequestId`] and a static
/// operation path, so these are defence-in-depth.
pub fn encode_request(
    id: u64,
    op: &str,
    op_id: Option<&OpId>,
    input_json: &str,
    bounds: &CodecBounds,
) -> Result<Vec<u8>, CodecError> {
    if id > MAX_REQUEST_ID {
        return Err(CodecError::UnrecoverableId(format!(
            "request id exceeds the browser-safe maximum {MAX_REQUEST_ID}"
        )));
    }
    if op.len() > bounds.max_op_len {
        return Err(CodecError::Malformed {
            id,
            error: ApiError::InvalidArgument {
                field: "op".into(),
                reason: InvalidReason::Bound {
                    min: 1,
                    max: bounds.max_op_len as u64,
                },
            },
        });
    }

    let mut out = Vec::with_capacity(input_json.len() + op.len() + 32);
    out.extend_from_slice(b"{\"id\":");
    out.extend_from_slice(id.to_string().as_bytes());
    out.extend_from_slice(b",\"op\":");
    push_json_string(&mut out, op);
    if let Some(op_id) = op_id {
        out.extend_from_slice(b",\"op_id\":");
        push_json_string(&mut out, op_id.as_str());
    }
    out.extend_from_slice(b",\"in\":");
    // Verbatim splice: this is already-serialized JSON, not re-encoded.
    out.extend_from_slice(input_json.as_bytes());
    out.push(b'}');
    Ok(out)
}

/// Append `s` as a correctly-escaped JSON string, delegating quoting to
/// `serde_json` so no escape case is hand-rolled.
fn push_json_string(out: &mut Vec<u8>, s: &str) {
    let encoded = serde_json::to_string(s).expect("a string is always serializable");
    out.extend_from_slice(encoded.as_bytes());
}

/// One decoded inbound frame, from the client's point of view.
///
/// The three protocol shapes plus a bounded catch-all. The success `out` is
/// captured as `Box<RawValue>` — the daemon's **exact** reply bytes — so the
/// kernel's erased payload carries them unchanged and the operation's typed
/// `Output` decodes faithfully.
pub enum ClientInbound {
    /// The daemon's first-frame `hello` (`t == "hello"`). Consumed by the dial
    /// sequence's protocol validation, never delivered as a steady-state
    /// inbound.
    Hello(Hello),
    /// A reply correlated by `id`.
    Reply {
        /// The correlation id the reply answers.
        id: u64,
        /// The reply body: the exact success `out` bytes, or a typed daemon
        /// error.
        result: Result<Box<RawValue>, ApiError>,
    },
    /// A live push (`t` present and not `hello`).
    Push(Push),
    /// A frame that is neither a well-formed reply, push, nor hello — it
    /// correlates to nothing and can strand no call.
    Malformed,
}

/// The borrowed view of a reply frame, used only to capture `out` as the
/// daemon's exact bytes without an intermediate tree.
#[derive(Deserialize)]
struct ReplyView<'a> {
    ok: bool,
    #[serde(borrow, default)]
    out: Option<&'a RawValue>,
    #[serde(default)]
    err: Option<ApiError>,
}

/// Decode one inbound frame's bytes into a [`ClientInbound`], bounded.
///
/// Applies the same frame-size, depth, and array-length bounds as the server
/// [`crate::decode`]; anything past them, or any frame that fails
/// discrimination, is [`ClientInbound::Malformed`] rather than a panic or an
/// unbounded allocation.
pub fn decode_client_frame(bytes: &[u8], bounds: &CodecBounds) -> ClientInbound {
    if bytes.len() > bounds.max_frame_bytes {
        return ClientInbound::Malformed;
    }

    // Structural parse to discriminate on `t`/`id` and to apply the shared
    // depth/array bounds. The reply's `out` is re-read from the original bytes
    // (below) so its exact form is preserved.
    let value: serde_json::Value = match serde_json::from_slice(bytes) {
        Ok(value) => value,
        Err(_) => return ClientInbound::Malformed,
    };
    if frame::check_bounds(&value, bounds, 0).is_err() {
        return ClientInbound::Malformed;
    }

    let has_t = value.get("t").is_some();
    let raw_id = value.get("id");

    match (has_t, raw_id) {
        // A push-shaped frame: `t` present, no `id`.
        (true, None) => {
            let is_hello = value.get("t").and_then(|t| t.as_str()) == Some("hello");
            if is_hello {
                match serde_json::from_slice::<Hello>(bytes) {
                    Ok(hello) => ClientInbound::Hello(hello),
                    Err(_) => ClientInbound::Malformed,
                }
            } else {
                match serde_json::from_slice::<Push>(bytes) {
                    Ok(push) => ClientInbound::Push(push),
                    Err(_) => ClientInbound::Malformed,
                }
            }
        }
        // A reply: `id` present, no `t`.
        (false, Some(id_value)) => {
            let Some(id) = id_value.as_u64() else {
                return ClientInbound::Malformed;
            };
            if id > MAX_REQUEST_ID {
                return ClientInbound::Malformed;
            }
            let Ok(view) = serde_json::from_slice::<ReplyView>(bytes) else {
                return ClientInbound::Malformed;
            };
            let result = if view.ok {
                // `ok` with an absent `out` means a unit output; represent it
                // as the literal `null` so the kernel's payload is well-formed.
                let raw = match view.out {
                    Some(raw) => raw.to_owned(),
                    None => {
                        RawValue::from_string(String::from("null")).expect("null is valid json")
                    }
                };
                Ok(raw)
            } else {
                match view.err {
                    Some(err) => Err(err),
                    None => return ClientInbound::Malformed,
                }
            };
            ClientInbound::Reply { id, result }
        }
        // Both `t` and `id`, or neither: unrepresentable in v2.
        _ => ClientInbound::Malformed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds() -> CodecBounds {
        CodecBounds::default()
    }

    #[test]
    fn encode_request_splices_input_verbatim() {
        let bytes = encode_request(7, "room.create", None, "{\"name\":\"x\"}", &bounds()).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert_eq!(
            text,
            "{\"id\":7,\"op\":\"room.create\",\"in\":{\"name\":\"x\"}}"
        );
    }

    #[test]
    fn encode_request_includes_op_id_when_present() {
        let op_id = OpId::new("dedup-1");
        let bytes = encode_request(1, "message.send", Some(&op_id), "{}", &bounds()).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert_eq!(
            text,
            "{\"id\":1,\"op\":\"message.send\",\"op_id\":\"dedup-1\",\"in\":{}}"
        );
    }

    #[test]
    fn encode_request_rejects_oversize_id() {
        let err = encode_request(MAX_REQUEST_ID + 1, "op", None, "{}", &bounds()).unwrap_err();
        assert!(matches!(err, CodecError::UnrecoverableId(_)));
    }

    #[test]
    fn decode_reply_captures_exact_out_bytes() {
        let frame = b"{\"id\":9,\"ok\":true,\"out\":{\"rooms\":[]}}";
        match decode_client_frame(frame, &bounds()) {
            ClientInbound::Reply {
                id,
                result: Ok(raw),
            } => {
                assert_eq!(id, 9);
                assert_eq!(raw.get(), "{\"rooms\":[]}");
            }
            _ => panic!("expected a reply"),
        }
    }

    #[test]
    fn decode_error_reply_carries_the_typed_error() {
        let frame = b"{\"id\":3,\"ok\":false,\"err\":{\"code\":\"not_ready\"}}";
        match decode_client_frame(frame, &bounds()) {
            ClientInbound::Reply {
                id,
                result: Err(err),
            } => {
                assert_eq!(id, 3);
                assert_eq!(err, ApiError::NotReady);
            }
            _ => panic!("expected an error reply"),
        }
    }

    #[test]
    fn decode_hello_is_discriminated_from_pushes() {
        let hello = br#"{"t":"hello","protocol":2,"storage_generation":5,"limits":{"max_shared_file_bytes":1,"max_message_body_bytes":1,"max_frame_bytes":1,"max_inflight_requests":1,"max_subscriptions_per_connection":1,"max_connections":1,"max_concurrent_transfers":1,"max_transfer_bytes_inflight":1,"transfer_connect_allowance_ms":1,"transfer_floor_bits_per_second":1,"transfer_stall_ms":1,"timeline_page_max":1,"idle_timeout_ms":1,"pairing_code_ttl_ms":1,"pairing_code_max_attempts":1,"browser_session_ttl_ms":1},"subject":{"state":"absent"},"resume":{"state":"fresh"}}"#;
        match decode_client_frame(hello, &bounds()) {
            ClientInbound::Hello(hello) => {
                assert_eq!(hello.protocol, 2);
                assert_eq!(hello.storage_generation, 5);
            }
            _ => panic!("expected a hello"),
        }
    }

    #[test]
    fn a_frame_with_both_id_and_t_is_malformed() {
        let frame = b"{\"id\":1,\"t\":\"hello\"}";
        assert!(matches!(
            decode_client_frame(frame, &bounds()),
            ClientInbound::Malformed
        ));
    }

    #[test]
    fn a_frame_with_neither_id_nor_t_is_malformed() {
        let frame = b"{\"ok\":true}";
        assert!(matches!(
            decode_client_frame(frame, &bounds()),
            ClientInbound::Malformed
        ));
    }

    #[test]
    fn an_oversize_reply_id_is_malformed() {
        let frame = b"{\"id\":9007199254740992,\"ok\":true,\"out\":null}";
        assert!(matches!(
            decode_client_frame(frame, &bounds()),
            ClientInbound::Malformed
        ));
    }

    // ---- encode_request boundary cases ----------------------------------------

    #[test]
    fn encode_request_rejects_op_name_exceeding_max_len() {
        // max_op_len = 64 (default); 65 bytes → Malformed.
        let long_op = "x".repeat(65);
        let err = encode_request(1, &long_op, None, "{}", &bounds()).unwrap_err();
        assert!(
            matches!(err, CodecError::Malformed { id: 1, .. }),
            "expected Malformed, got {err:?}"
        );
    }

    #[test]
    fn encode_request_accepts_exactly_max_request_id() {
        // MAX_REQUEST_ID is the last valid id — must succeed, not reject.
        let bytes = encode_request(MAX_REQUEST_ID, "op", None, "{}", &bounds()).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        // The id appears verbatim in the envelope.
        assert!(
            text.contains(&MAX_REQUEST_ID.to_string()),
            "MAX_REQUEST_ID absent from envelope: {text}"
        );
    }

    #[test]
    fn encode_request_json_string_escapes_op_name() {
        // An op name containing a double-quote must be escaped so the envelope
        // is always well-formed JSON; the round-trip must recover the original.
        let op_with_quote = "op/with\"quote";
        let bytes = encode_request(1, op_with_quote, None, "{}", &bounds()).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        let parsed: serde_json::Value =
            serde_json::from_str(&text).expect("envelope must be valid JSON after escaping");
        assert_eq!(
            parsed["op"].as_str(),
            Some(op_with_quote),
            "round-tripped op name does not match"
        );
    }

    // ---- decode_client_frame boundary cases -----------------------------------

    #[test]
    fn decode_client_frame_exceeding_max_bytes_is_malformed() {
        // Tight bounds: any frame over 10 bytes is refused before parsing.
        let tight = CodecBounds {
            max_frame_bytes: 10,
            ..CodecBounds::default()
        };
        let frame = b"{\"id\":1,\"ok\":true,\"out\":null}"; // 30 bytes
        assert!(matches!(
            decode_client_frame(frame, &tight),
            ClientInbound::Malformed
        ));
    }

    #[test]
    fn decode_ok_reply_with_absent_out_field_yields_null() {
        // Unit output: `ok:true` with no `out` field must produce `null` so the
        // kernel's RawJson payload is always well-formed.
        let frame = b"{\"id\":5,\"ok\":true}";
        match decode_client_frame(frame, &bounds()) {
            ClientInbound::Reply {
                id,
                result: Ok(raw),
            } => {
                assert_eq!(id, 5);
                assert_eq!(raw.get(), "null", "absent out must deserialise as null");
            }
            _ => panic!("expected Ok reply with null out"),
        }
    }

    #[test]
    fn decode_error_reply_without_err_field_is_malformed() {
        // `ok:false` with no `err` field is structurally unsound: the
        // error code cannot be recovered, so the frame is Malformed.
        let frame = b"{\"id\":5,\"ok\":false}";
        assert!(matches!(
            decode_client_frame(frame, &bounds()),
            ClientInbound::Malformed
        ));
    }

    #[test]
    fn decode_non_hello_push_routes_to_push_branch() {
        // A frame with `t != "hello"` and no `id` must route to the Push
        // branch, not Hello or Reply — even if Push deserialization fails
        // (unknown variant → Malformed), proving the discriminator is correct.
        // "unknown_t" is not in Push's closed set, so deserialization fails.
        let frame = b"{\"t\":\"unknown_t\",\"room_id\":\"r1\"}";
        // Must NOT be Hello or Reply — it must reach the Push branch:
        assert!(
            !matches!(
                decode_client_frame(frame, &bounds()),
                ClientInbound::Hello(_) | ClientInbound::Reply { .. }
            ),
            "a non-hello t-tagged frame must not be classified as Hello or Reply"
        );
    }
}
