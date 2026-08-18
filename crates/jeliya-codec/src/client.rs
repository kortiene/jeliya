//! The **client direction** of the protocol-v2 codec: the encoder for an
//! outbound request envelope and the decoder for the Text messages a daemon
//! sends a client.
//!
//! The rest of the codec speaks the *daemon* direction only
//! ([`crate::decode`] → [`crate::Frame::Request`], [`crate::encode_reply`],
//! [`crate::push_to_bytes`]). A native/browser adapter (#172/#171) is the wire
//! peer on the other side, and it must never touch `serde_json::Value` (a
//! `jeliya-client` boundary rule). So request-envelope assembly and
//! reply/push/hello decoding live here, where the protocol's only JSON is
//! allowed, and the adapter receives typed [`ClientFrame`] values plus the
//! success `out` re-serialized back to a text string it can hand its kernel.
//!
//! The three inbound shapes are told apart exactly as the daemon decoder tells
//! a request from a push: a `hello` carries `t: "hello"`, any other push
//! carries `t` and never `id`, and a reply carries `id` and never `t`. A frame
//! matching none of these decodes to [`ClientFrame::Malformed`] — it can
//! correlate to nothing, so it strands no call.

use jeliya_api::{ApiError, Hello, OpId, Push};

use crate::frame::{check_bounds, Reply};
use crate::{CodecBounds, CodecError};

/// Encode one outbound request envelope as protocol-v2 Text bytes.
///
/// `in_json` is the already-serialized `in` object (the kernel's erased JSON
/// text), embedded verbatim; the codec assembles
/// `{ "id", "op", ["op_id"], "in": <in_json> }`, JSON-escaping `op`/`op_id` and
/// rendering `id` as a bare integer. The bytes are always valid UTF-8 JSON.
///
/// `op_id` is emitted only when present: an explicit JSON `null` op_id is
/// forbidden everywhere in v2, and omission is the sole way the envelope's one
/// optional field is absent (matching the daemon decoder's rule).
pub fn encode_request(id: u64, op: &str, op_id: Option<&OpId>, in_json: &str) -> Vec<u8> {
    let mut buf = String::with_capacity(in_json.len() + op.len() + 48);
    buf.push_str("{\"id\":");
    buf.push_str(itoa(id).as_str());
    buf.push_str(",\"op\":");
    push_json_string(op, &mut buf);
    if let Some(op_id) = op_id {
        buf.push_str(",\"op_id\":");
        push_json_string(op_id.as_str(), &mut buf);
    }
    buf.push_str(",\"in\":");
    buf.push_str(in_json);
    buf.push('}');
    buf.into_bytes()
}

/// Render a `u64` as decimal without pulling an integer-formatting dependency.
fn itoa(value: u64) -> String {
    value.to_string()
}

/// Append `s` as a correctly-escaped JSON string token. `serde_json` guarantees
/// the escaping; a plain string always serializes, so the `expect` is
/// unreachable (mirrors [`crate::frame::Reply::to_bytes`]).
fn push_json_string(s: &str, buf: &mut String) {
    buf.push_str(&serde_json::to_string(s).expect("a string always serializes"));
}

/// One inbound Text message a daemon may send a client, decoded into a typed
/// value the adapter forwards to its kernel.
#[derive(Debug)]
pub enum ClientFrame {
    /// The Layer-2 `hello` — the daemon's first frame after upgrade
    /// (`t: "hello"`).
    Hello(Hello),
    /// A reply correlated by `id`. The success `out` is re-serialized to a
    /// text string so `jeliya-client` never touches `serde_json::Value`; a
    /// typed daemon verdict arrives as `Err`.
    Reply {
        /// The echoed request id.
        id: u64,
        /// The re-serialized `out` text, or the typed daemon error.
        result: Result<String, ApiError>,
    },
    /// A live push (`event` / `gap` / `peer` / `transfer`).
    Push(Push),
    /// The frame decodes to no usable hello/reply/push. It correlates to
    /// nothing and strands no call; the adapter maps it to the kernel's
    /// malformed-inbound path.
    Malformed(ApiError),
}

/// Decode one inbound Text message, bounded.
///
/// Returns [`CodecError::FrameTooLarge`] for a frame past the hard byte
/// ceiling (the only hard refusal on this path); every other unparseable or
/// unrecognized frame decodes to [`ClientFrame::Malformed`], because an inbound
/// frame the client cannot understand must strand nothing rather than tear the
/// connection down blindly. The same depth/array ceilings the daemon decoder
/// applies are enforced here.
pub fn decode_client_frame(bytes: &[u8], bounds: &CodecBounds) -> Result<ClientFrame, CodecError> {
    if bytes.len() > bounds.max_frame_bytes {
        return Err(CodecError::FrameTooLarge {
            limit_bytes: bounds.max_frame_bytes as u64,
        });
    }
    let value: serde_json::Value = match serde_json::from_slice(bytes) {
        Ok(value) => value,
        Err(_) => return Ok(ClientFrame::Malformed(ApiError::MalformedFrame)),
    };
    if let Err(error) = check_bounds(&value, bounds, 0) {
        return Ok(ClientFrame::Malformed(error));
    }

    // A frame carrying `t` is a hello or another push; a reply never carries
    // `t`. This mirrors the daemon decoder's request-vs-push discriminator.
    if value.get("t").is_some() {
        return Ok(match value.get("t").and_then(|t| t.as_str()) {
            Some("hello") => match serde_json::from_value::<Hello>(value) {
                Ok(hello) => ClientFrame::Hello(hello),
                Err(_) => ClientFrame::Malformed(ApiError::MalformedFrame),
            },
            Some(_) => match serde_json::from_value::<Push>(value) {
                Ok(push) => ClientFrame::Push(push),
                Err(_) => ClientFrame::Malformed(ApiError::MalformedFrame),
            },
            // `t` present but not a string is no discriminator at all.
            None => ClientFrame::Malformed(ApiError::MalformedFrame),
        });
    }

    // No `t`: a reply, correlated by `id`.
    if value.get("id").is_some() {
        return Ok(match serde_json::from_value::<Reply>(value) {
            Ok(reply) => match reply_to_result(reply) {
                Some((id, result)) => ClientFrame::Reply { id, result },
                None => ClientFrame::Malformed(ApiError::MalformedFrame),
            },
            Err(_) => ClientFrame::Malformed(ApiError::MalformedFrame),
        });
    }

    Ok(ClientFrame::Malformed(ApiError::MalformedFrame))
}

/// Turn a decoded [`Reply`] into `(id, result)`, re-serializing a success `out`
/// back to text. `ok` discriminates; a success with an absent `out` re-reads as
/// `null` (a unit output), and a not-`ok` reply must carry `err` or it is
/// malformed.
fn reply_to_result(reply: Reply) -> Option<(u64, Result<String, ApiError>)> {
    if reply.ok {
        let out = reply.out.unwrap_or(serde_json::Value::Null);
        let text = serde_json::to_string(&out).expect("a decoded value re-serializes");
        Some((reply.id, Ok(text)))
    } else {
        reply.err.map(|err| (reply.id, Err(err)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jeliya_api::{Limits, Resume, RoomId, SubjectState};

    fn bounds() -> CodecBounds {
        CodecBounds::default()
    }

    fn limits() -> Limits {
        Limits {
            max_shared_file_bytes: 104_857_600,
            max_message_body_bytes: 65_536,
            max_frame_bytes: 128 * 1024 * 1024,
            max_inflight_requests: 256,
            max_subscriptions_per_connection: 64,
            max_connections: 64,
            max_concurrent_transfers: 8,
            max_transfer_bytes_inflight: 1 << 30,
            transfer_connect_allowance_ms: 10_000,
            transfer_floor_bits_per_second: 1_000_000,
            transfer_stall_ms: 30_000,
            timeline_page_max: 200,
            idle_timeout_ms: 60_000,
            pairing_code_ttl_ms: 300_000,
            pairing_code_max_attempts: 5,
            browser_session_ttl_ms: 3_600_000,
        }
    }

    #[test]
    fn encode_request_assembles_the_envelope_with_op_id() {
        let bytes = encode_request(
            7,
            "room.create",
            Some(&OpId::new("dedup-1")),
            r#"{"name":"r"}"#,
        );
        let text = String::from_utf8(bytes).expect("utf-8");
        assert_eq!(
            text,
            r#"{"id":7,"op":"room.create","op_id":"dedup-1","in":{"name":"r"}}"#
        );
    }

    #[test]
    fn encode_request_omits_absent_op_id() {
        let bytes = encode_request(3, "room.list", None, "{}");
        let text = String::from_utf8(bytes).expect("utf-8");
        assert_eq!(text, r#"{"id":3,"op":"room.list","in":{}}"#);
        assert!(!text.contains("op_id"));
    }

    #[test]
    fn encode_request_escapes_a_hostile_op_id() {
        let bytes = encode_request(1, "message.send", Some(&OpId::new("a\"b\n")), "{}");
        let text = String::from_utf8(bytes).expect("utf-8");
        // The embedded quote and newline are escaped, so the envelope stays
        // well-formed JSON that round-trips.
        let value: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
        assert_eq!(value["op_id"], "a\"b\n");
    }

    #[test]
    fn decode_hello_is_recognized() {
        let hello = Hello {
            incarnation: jeliya_api::Incarnation::new("codec-test-incarnation"),
            protocol: 2,
            storage_generation: 2,
            limits: limits(),
            subject: SubjectState::Absent,
            resume: Resume::Fresh,
        };
        let bytes = serde_json::to_vec(&hello).expect("serializes");
        match decode_client_frame(&bytes, &bounds()).expect("decodes") {
            ClientFrame::Hello(decoded) => {
                assert_eq!(decoded.protocol, 2);
                assert_eq!(decoded.storage_generation, 2);
            }
            other => panic!("expected hello, got {other:?}"),
        }
    }

    #[test]
    fn decode_ok_reply_reserializes_out() {
        let bytes = br#"{"id":9,"ok":true,"out":{"rooms":[]}}"#;
        match decode_client_frame(bytes, &bounds()).expect("decodes") {
            ClientFrame::Reply { id, result } => {
                assert_eq!(id, 9);
                assert_eq!(result.expect("ok"), r#"{"rooms":[]}"#);
            }
            other => panic!("expected reply, got {other:?}"),
        }
    }

    #[test]
    fn decode_error_reply_carries_the_typed_error() {
        let bytes = br#"{"id":4,"ok":false,"err":{"code":"not_ready"}}"#;
        match decode_client_frame(bytes, &bounds()).expect("decodes") {
            ClientFrame::Reply {
                id,
                result: Err(ApiError::NotReady),
            } => assert_eq!(id, 4),
            other => panic!("expected error reply, got {other:?}"),
        }
    }

    #[test]
    fn decode_push_is_recognized() {
        let push = Push::Gap {
            room_id: RoomId::new("r"),
            from_pos: 3,
            to: jeliya_api::GapTo::Open,
            reason: jeliya_api::GapReason::Backpressure,
        };
        let bytes = crate::push_to_bytes(&push);
        match decode_client_frame(&bytes, &bounds()).expect("decodes") {
            ClientFrame::Push(Push::Gap { from_pos, .. }) => assert_eq!(from_pos, 3),
            other => panic!("expected gap push, got {other:?}"),
        }
    }

    #[test]
    fn decode_garbage_is_malformed_not_an_error() {
        match decode_client_frame(b"not json", &bounds()).expect("no hard error") {
            ClientFrame::Malformed(ApiError::MalformedFrame) => {}
            other => panic!("expected malformed, got {other:?}"),
        }
    }

    #[test]
    fn decode_oversized_frame_is_a_hard_refusal() {
        let tiny = CodecBounds {
            max_frame_bytes: 4,
            ..CodecBounds::default()
        };
        let err = decode_client_frame(b"{\"id\":1}", &tiny).expect_err("too large");
        assert!(matches!(err, CodecError::FrameTooLarge { limit_bytes: 4 }));
    }

    #[test]
    fn encode_then_decode_is_not_confused_between_reply_and_push() {
        // A request envelope is not an inbound frame, but a reply built for it
        // must decode back on the client side.
        let reply = crate::encode_reply::<jeliya_api::RoomList>(
            5,
            Ok(jeliya_api::RoomListOut { rooms: vec![] }),
        );
        let bytes = reply.to_bytes();
        match decode_client_frame(&bytes, &bounds()).expect("decodes") {
            ClientFrame::Reply { id, result } => {
                assert_eq!(id, 5);
                let out: jeliya_api::RoomListOut =
                    serde_json::from_str(&result.expect("ok")).expect("out decodes");
                assert!(out.rooms.is_empty());
            }
            other => panic!("expected reply, got {other:?}"),
        }
    }

    #[test]
    fn decode_error_reply_without_err_field_is_malformed() {
        // An ok:false reply with no `err` field cannot be correlated and must
        // strand nothing rather than crash.
        let bytes = br#"{"id":1,"ok":false}"#;
        match decode_client_frame(bytes, &bounds()).expect("no hard error") {
            ClientFrame::Malformed(_) => {}
            other => panic!("expected malformed, got {other:?}"),
        }
    }

    #[test]
    fn decode_empty_object_is_malformed() {
        // A JSON object with no `id` and no `t` cannot be classified at all.
        match decode_client_frame(b"{}", &bounds()).expect("no hard error") {
            ClientFrame::Malformed(_) => {}
            other => panic!("expected malformed for empty object, got {other:?}"),
        }
    }

    #[test]
    fn decode_t_present_but_not_a_string_is_malformed() {
        // `t: 42` is not a valid discriminator for either hello or push.
        match decode_client_frame(br#"{"t":42}"#, &bounds()).expect("no hard error") {
            ClientFrame::Malformed(_) => {}
            other => panic!("expected malformed for numeric t, got {other:?}"),
        }
    }

    #[test]
    fn encode_request_max_id_is_valid_json() {
        let bytes = encode_request(u64::MAX, "room.list", None, "{}");
        let text = String::from_utf8(bytes).expect("utf-8");
        let value: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
        assert_eq!(
            value["id"],
            serde_json::json!(u64::MAX),
            "u64::MAX id must round-trip through the JSON encoder"
        );
        assert_eq!(value["op"], "room.list");
    }

    #[test]
    fn decode_null_out_in_ok_reply_reserializes_as_null_string() {
        // An ok:true reply with no `out` field deserializes `out` as null (a
        // unit output), and the re-serialized result string is "null" — a valid
        // JSON text that jeliya-client hands to the caller's deserializer.
        let bytes = br#"{"id":7,"ok":true}"#;
        match decode_client_frame(bytes, &bounds()).expect("decodes") {
            ClientFrame::Reply { id, result } => {
                assert_eq!(id, 7);
                assert_eq!(result.expect("ok"), "null");
            }
            other => panic!("expected reply, got {other:?}"),
        }
    }
}
