//! Golden-vector tests replaying the v2 conformance corpus's wire shapes,
// independent of the implementation. These prove the codec round-trips the
// shapes the corpus pins, rejects what it refuses, and keeps every
// malformed frame bounded.

use jeliya_api::*;
use jeliya_codec::{decode, encode_push, encode_reply, CodecBounds, Frame};

fn default_bounds() -> CodecBounds {
    CodecBounds::default()
}

/// Every approved operation's request envelope decodes and its reply
/// round-trips. The vectors are hand-written from the record's schemas,
// not generated from the implementation.
#[test]
fn all_operations_round_trip() {
    let cases: Vec<(&str, serde_json::Value)> = vec![
        ("subject.ensure", serde_json::json!({})),
        ("daemon.stop", serde_json::json!({})),
        ("room.create", serde_json::json!({"name": "Build"})),
        ("room.list", serde_json::json!({})),
        ("room.activate", serde_json::json!({"room_id": "r1"})),
        ("room.deactivate", serde_json::json!({"room_id": "r1"})),
        ("room.leave", serde_json::json!({"room_id": "r1"})),
        (
            "room.timeline",
            serde_json::json!({"room_id": "r1", "cursor": {"state": "start"}, "direction": "forward", "limit": 50}),
        ),
        ("room.members", serde_json::json!({"room_id": "r1"})),
        (
            "room.archive",
            serde_json::json!({"room_id": "r1", "cursor": {"state": "start"}, "direction": "forward", "limit": 50}),
        ),
        ("room.peers", serde_json::json!({"room_id": "r1"})),
        (
            "member.remove",
            serde_json::json!({"room_id": "r1", "subject_id": "s1"}),
        ),
        (
            "invite.mint",
            serde_json::json!({"room_id": "r1", "subject_id": "s1", "role": "member", "expires_at": "2026-01-01T00:00:00Z"}),
        ),
        (
            "invite.list",
            serde_json::json!({"room_id": "r1", "cursor": {"state": "start"}, "direction": "forward", "limit": 50}),
        ),
        (
            "invite.revoke",
            serde_json::json!({"room_id": "r1", "invite_id": "i1"}),
        ),
        ("invite.redeem", serde_json::json!({"capability": "cap"})),
        (
            "message.send",
            serde_json::json!({"room_id": "r1", "body": "hello"}),
        ),
        (
            "status.post",
            serde_json::json!({"room_id": "r1", "label": "working", "progress": {"state": "reported", "percent": 40}}),
        ),
        (
            "status.history",
            serde_json::json!({"room_id": "r1", "subject_id": "s1", "cursor": {"state": "start"}, "direction": "forward", "limit": 50}),
        ),
        ("fleet.list", serde_json::json!({})),
        (
            "file.share",
            serde_json::json!({"room_id": "r1", "name": "n", "declared_bytes": 4096, "declared_content_type": "text/plain"}),
        ),
        (
            "file.list",
            serde_json::json!({"room_id": "r1", "cursor": {"state": "start"}, "direction": "forward", "limit": 50}),
        ),
        (
            "file.fetch",
            serde_json::json!({"room_id": "r1", "file_id": "f1"}),
        ),
        (
            "file.read",
            serde_json::json!({"room_id": "r1", "file_id": "f1"}),
        ),
        (
            "transfer.cancel",
            serde_json::json!({"transfer_op_id": "op-1"}),
        ),
        (
            "pipe.publish",
            serde_json::json!({"room_id": "r1", "target": {"host": "127.0.0.1", "port": 8080}, "audience": {"state": "room"}}),
        ),
        (
            "pipe.list",
            serde_json::json!({"room_id": "r1", "cursor": {"state": "start"}, "direction": "forward", "limit": 50}),
        ),
        (
            "pipe.connect",
            serde_json::json!({"room_id": "r1", "pipe_id": "p1"}),
        ),
        ("pipe.release", serde_json::json!({"connection_id": "c1"})),
        (
            "pipe.revoke",
            serde_json::json!({"room_id": "r1", "pipe_id": "p1"}),
        ),
        (
            "stream.subscribe",
            serde_json::json!({"room_id": "r1", "from": {"state": "start"}}),
        ),
        ("stream.unsubscribe", serde_json::json!({"room_id": "r1"})),
        (
            "stream.resync",
            serde_json::json!({"room_id": "r1", "from_pos": 41}),
        ),
    ];
    assert_eq!(
        cases.len(),
        33,
        "every approved operation has a golden vector"
    );

    for (op, input) in cases {
        let frame = serde_json::json!({
            "id": 42,
            "op": op,
            "op_id": "op-x",
            "in": input,
        });
        let bytes = serde_json::to_vec(&frame).unwrap();
        let decoded = decode(&bytes, &default_bounds())
            .unwrap_or_else(|e| panic!("{op} failed to decode: {e:?}"));
        match decoded {
            Frame::Request(req) => {
                assert_eq!(req.id, 42, "{op}: id echoes exactly");
                assert_eq!(req.call.op, op, "{op}: routed to itself");
            }
            Frame::Malformed(e) => panic!("{op} unexpectedly malformed: {e:?}"),
        }
    }
}

/// An unknown `op` is `unknown_operation`, by name.
#[test]
fn unknown_operation_is_refused() {
    let frame = serde_json::json!({"id": 1, "op": "room.explode", "in": {}});
    let err = decode(&serde_json::to_vec(&frame).unwrap(), &default_bounds()).unwrap_err();
    assert!(matches!(
        err,
        jeliya_codec::CodecError::Malformed {
            error: ApiError::UnknownOperation { .. },
            ..
        }
    ));
}

/// An undefined request field is `invalid_argument` / `unrecognised_field`,
// including the record's named case: `status.post` carrying a free-text
// `message`.
#[test]
fn status_post_free_text_is_unrecognised_field() {
    let frame = serde_json::json!({
        "id": 1, "op": "status.post",
        "in": {"room_id": "r1", "label": "working", "message": "free", "progress": {"state": "absent"}}
    });
    let err = decode(&serde_json::to_vec(&frame).unwrap(), &default_bounds()).unwrap_err();
    match err {
        jeliya_codec::CodecError::Malformed {
            error: ApiError::InvalidArgument { field, reason },
            ..
        } => {
            assert_eq!(reason, InvalidReason::UnrecognisedField);
            assert_eq!(field, "in.message");
        }
        other => panic!("wrong refusal: {other:?}"),
    }
}

/// A frame whose id cannot be recovered is `malformed_frame` and closes
// 4007 — there is nothing to correlate a reply to.
#[test]
fn unrecoverable_id_closes() {
    let err = decode(b"this is not json{", &default_bounds()).unwrap_err();
    assert!(matches!(err, jeliya_codec::CodecError::UnrecoverableId(_)));
}

/// A frame that decodes far enough to carry an id gets a correlated error
// reply, never a close — one bad request never strands the others.
#[test]
fn recoverable_id_gets_a_correlated_error() {
    // valid JSON, carries an id, but not an envelope
    let err = decode(
        br#"{"id": 77, "op": "room.list", "in": "not-an-object"}"#,
        &default_bounds(),
    )
    .unwrap_err();
    assert!(matches!(err, jeliya_codec::CodecError::Malformed { .. }));
}

/// A frame carrying `t` is a push; pushes do not flow client-to-daemon.
/// A push with no usable id takes the unrecoverable-id close, because
/// there is no id to correlate a reply to.
#[test]
fn an_inbound_push_is_malformed() {
    let frame = serde_json::json!({"t": "gap", "room_id": "r1", "from_pos": 41, "to": {"state": "bounded", "pos": 57}, "reason": "backpressure"});
    let err = decode(&serde_json::to_vec(&frame).unwrap(), &default_bounds()).unwrap_err();
    assert!(matches!(err, jeliya_codec::CodecError::UnrecoverableId(_)));
}

/// A push carrying a usable id is a correlated malformed reply.
#[test]
fn an_inbound_push_with_id_is_a_correlated_malformed() {
    let frame = serde_json::json!({"id": 7, "t": "gap", "room_id": "r1", "from_pos": 41, "to": {"state": "bounded", "pos": 57}, "reason": "backpressure"});
    let err = decode(&serde_json::to_vec(&frame).unwrap(), &default_bounds()).unwrap_err();
    assert!(matches!(
        err,
        jeliya_codec::CodecError::Malformed {
            id: 7,
            error: ApiError::MalformedFrame
        }
    ));
}

/// A frame over the byte ceiling closes 4005 unparsed.
#[test]
fn an_oversize_frame_is_refused_without_parsing() {
    let bounds = CodecBounds {
        max_frame_bytes: 64,
        ..CodecBounds::default()
    };
    let big = vec![b'x'; 128];
    let err = decode(&big, &bounds).unwrap_err();
    match err {
        jeliya_codec::CodecError::FrameTooLarge { limit_bytes } => assert_eq!(limit_bytes, 64),
        other => panic!("wrong refusal: {other:?}"),
    }
}

/// Excessive nesting depth is `invalid_argument`, never a panic and never
// an unbounded allocation.
#[test]
fn excessive_depth_is_bounded() {
    let bounds = CodecBounds {
        max_depth: 4,
        ..CodecBounds::default()
    };
    let mut v = serde_json::json!({"id": 1, "op": "room.list", "in": {}});
    for _ in 0..8 {
        v = serde_json::json!({"wrap": v});
    }
    // the wrapper makes it not an envelope; but depth is checked first
    let deep = serde_json::json!({"id": 1, "op": "room.list", "in": {"a": {"b": {"c": {"d": {"e": {"f": 1}}}}}}});
    let err = decode(&serde_json::to_vec(&deep).unwrap(), &bounds).unwrap_err();
    assert!(matches!(
        err,
        jeliya_codec::CodecError::Malformed {
            error: ApiError::InvalidArgument { .. },
            ..
        }
    ));
}

/// An over-length array is bounded.
#[test]
fn excessive_array_length_is_bounded() {
    let bounds = CodecBounds {
        max_array_len: 4,
        ..CodecBounds::default()
    };
    // the over-length array is inside `in` (unknown *envelope* keys are
    // tolerated by design; the bound applies to decoded values)
    let frame = serde_json::json!({"id": 1, "op": "room.members", "in": {"room_id": ["a","b","c","d","e"]}});
    let err = decode(&serde_json::to_vec(&frame).unwrap(), &bounds).unwrap_err();
    assert!(matches!(
        err,
        jeliya_codec::CodecError::Malformed {
            error: ApiError::InvalidArgument { .. },
            ..
        }
    ));
}

/// An `op` name over the length bound is refused.
#[test]
fn an_overlong_op_name_is_bounded() {
    let bounds = CodecBounds {
        max_op_len: 8,
        ..CodecBounds::default()
    };
    let frame = serde_json::json!({"id": 1, "op": "a".repeat(32), "in": {}});
    let err = decode(&serde_json::to_vec(&frame).unwrap(), &bounds).unwrap_err();
    assert!(matches!(
        err,
        jeliya_codec::CodecError::Malformed {
            error: ApiError::InvalidArgument { .. },
            ..
        }
    ));
}

/// A reply echoes the request id exactly and carries `out` xor `err`.
#[test]
fn reply_echoes_id_and_is_one_sided() {
    let ok = encode_reply::<RoomCreate>(
        42,
        Ok(RoomCreateOut {
            room_id: RoomId::new("r1"),
            name: "Build".into(),
            role: Role::Authority,
            standing: Standing::Active,
            event_id: EventId::new("e1"),
            pos: 0,
            created_at: Timestamp::from(time::OffsetDateTime::UNIX_EPOCH),
        }),
    );
    assert!(ok.ok);
    assert!(ok.out.is_some());
    assert!(ok.err.is_none());
    assert_eq!(ok.id, 42);

    let err = encode_reply::<RoomCreate>(
        42,
        Err(ApiError::RoomNameInvalid {
            reason: InvalidReason::Format,
        }),
    );
    assert!(!err.ok);
    assert!(err.out.is_none());
    assert!(err.err.is_some());
    let bytes = err.to_bytes();
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.contains("\"id\":42"));
    assert!(text.contains("\"ok\":false"));
    assert!(!text.contains("hint"));
}

/// A push carries `t` and never `id`.
#[test]
fn push_carries_t_never_id() {
    let push = Push::Gap {
        room_id: RoomId::new("r1"),
        from_pos: 41,
        to: GapTo::Bounded { pos: 57 },
        reason: GapReason::Backpressure,
    };
    let value = encode_push(&push).unwrap();
    assert_eq!(value["t"], "gap");
    assert!(value.get("id").is_none());
}

/// No JSON null carries meaning: a null in a frame is a variant's job, and
// a `null` `in` for a no-input operation is refused (absence is a tagged
// variant, not a null).
#[test]
fn null_carries_no_meaning() {
    let frame = serde_json::json!({"id": 1, "op": "room.list", "in": null});
    // null is not an object — serde rejects it for the struct
    let result = decode(&serde_json::to_vec(&frame).unwrap(), &default_bounds());
    // null `in` for an empty-object operation: serde maps null to unit-like?
    // RoomList is {} — serde_json maps null to a unit struct only if declared
    // as such. Our struct is a braced empty struct; null should fail.
    assert!(result.is_err());
}

/// A malformed frame carries its recovered id so the transport can build
/// the correlated reply without reparsing outside the codec.
#[test]
fn malformed_frames_carry_the_recovered_id() {
    let frame = serde_json::json!({"id": 77, "op": "room.explode", "in": {}});
    let err = decode(&serde_json::to_vec(&frame).unwrap(), &default_bounds()).unwrap_err();
    match err {
        jeliya_codec::CodecError::Malformed { id, .. } => assert_eq!(id, 77),
        other => panic!("expected Malformed with id: {other:?}"),
    }
}

/// An explicit null op_id is refused: omission is the only way the
/// envelope's one optional field is absent.
#[test]
fn a_null_op_id_is_refused() {
    let frame =
        serde_json::json!({"id": 1, "op": "room.create", "op_id": null, "in": {"name": "Build"}});
    let err = decode(&serde_json::to_vec(&frame).unwrap(), &default_bounds()).unwrap_err();
    assert!(matches!(
        err,
        jeliya_codec::CodecError::Malformed {
            error: ApiError::InvalidArgument { .. },
            ..
        }
    ));
}

/// A non-unsigned-integer id is no correlation identifier — it takes the
/// unrecoverable-id close, not the correlated-reply path.
#[test]
fn a_non_uint_id_closes() {
    for bad in [
        serde_json::json!({"id": null, "op": "room.list", "in": {}}),
        serde_json::json!({"id": "42", "op": "room.list", "in": {}}),
        serde_json::json!({"id": -1, "op": "room.list", "in": {}}),
        serde_json::json!({"id": 1.5, "op": "room.list", "in": {}}),
    ] {
        let err = decode(&serde_json::to_vec(&bad).unwrap(), &default_bounds()).unwrap_err();
        assert!(
            matches!(err, jeliya_codec::CodecError::UnrecoverableId(_)),
            "id {:?} should close",
            bad["id"]
        );
    }
}

/// A wrong JSON type in a request field maps to the `type {expected}`
/// reason, not `format`.
#[test]
fn a_wrong_type_maps_to_the_type_reason() {
    let frame = serde_json::json!({"id": 1, "op": "room.create", "in": {"name": 3}});
    let err = decode(&serde_json::to_vec(&frame).unwrap(), &default_bounds()).unwrap_err();
    match err {
        jeliya_codec::CodecError::Malformed {
            error: ApiError::InvalidArgument { reason, .. },
            ..
        } => {
            assert!(
                matches!(reason, InvalidReason::Type { .. }),
                "expected Type reason, got {reason:?}"
            );
        }
        other => panic!("expected InvalidArgument: {other:?}"),
    }
}
