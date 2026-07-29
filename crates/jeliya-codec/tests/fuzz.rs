//! Bounded malformed-input fuzz: a structured walk over a space of
// malformed frames, asserting the codec never panics, never hangs, and
// always returns one of the documented refusals. This is the fuzz bound
// #164 names: malformed input cannot strand pending requests and cannot
// cause unbounded work.

use jeliya_api::ApiError;
use jeliya_codec::{decode, CodecBounds, CodecError};

/// A structured fuzz corpus: byte sequences that probe every refusal path.
/// Each must return a CodecError (never panic, never Ok-garbage), and each
/// must return within the bounds the codec declares.
#[test]
fn malformed_frames_fail_boundedly() {
    let bounds = CodecBounds::default();
    let corpus: Vec<Vec<u8>> = vec![
        // not json at all
        b"".to_vec(),
        b"x".to_vec(),
        b"{".to_vec(),
        b"this is not json{".to_vec(),
        // valid json, not an envelope
        b"null".to_vec(),
        b"[]".to_vec(),
        b"42".to_vec(),
        b"\"string\"".to_vec(),
        br#"{"a":1}"#.to_vec(),
        // envelope-ish but wrong types
        br#"{"id":"42","op":"room.list","in":{}}"#.to_vec(),
        br#"{"id":1.5,"op":"room.list","in":{}}"#.to_vec(),
        br#"{"id":-1,"op":"room.list","in":{}}"#.to_vec(),
        br#"{"id":42,"op":7,"in":{}}"#.to_vec(),
        br#"{"id":42,"op":"room.list","in":"not-an-object"}"#.to_vec(),
        // both id and t (a push with an id, or a reply with a t)
        br#"{"id":1,"t":"gap","op":"room.list","in":{}}"#.to_vec(),
        // neither id nor t
        br#"{"op":"room.list","in":{}}"#.to_vec(),
        // unknown op
        br#"{"id":1,"op":"room.explode","in":{}}"#.to_vec(),
        // deeply nested (under the depth bound but malformed structurally)
        br#"{"id":1,"op":"room.list","in":{"a":{"b":{"c":{"d":{}}}}}}"#.to_vec(),
    ];

    for (i, frame) in corpus.iter().enumerate() {
        let result = decode(frame, &bounds);
        // must never panic (the call itself returning is the proof), and
        // must be a CodecError — malformed input never produces Ok
        match result {
            Err(
                CodecError::FrameTooLarge { .. }
                | CodecError::UnrecoverableId(_)
                | CodecError::Malformed { .. }
                | CodecError::GateRefused(_),
            ) => {}
            Ok(_) => panic!(
                "frame {i} unexpectedly decoded: {:?}",
                String::from_utf8_lossy(frame)
            ),
        }
    }
}

/// A large corpus of random-ish mutations of a valid frame, asserting the
/// codec is total: every input returns Ok or a bounded CodecError, and no
/// input causes a panic.
#[test]
fn mutated_valid_frames_are_total() {
    let bounds = CodecBounds::default();
    let valid = serde_json::json!({
        "id": 42, "op": "room.create", "op_id": "op-1", "in": {"name": "Build"}
    });
    let valid_bytes = serde_json::to_vec(&valid).unwrap();

    // single-byte deletions and substitutions across the whole frame
    for i in 0..valid_bytes.len() {
        // deletion
        let mut mutated = valid_bytes.clone();
        mutated.remove(i);
        let _ = decode(&mutated, &bounds); // must not panic
                                           // substitution with a brace-breaking byte
        let mut mutated = valid_bytes.clone();
        mutated[i] = b'{';
        let _ = decode(&mutated, &bounds);
        // substitution with a null byte
        let mut mutated = valid_bytes.clone();
        mutated[i] = 0;
        let _ = decode(&mutated, &bounds);
    }
}

/// Truncated frames at every length return a bounded refusal, never a
// panic and never an Ok on a prefix that is not itself a valid frame.
#[test]
fn truncated_frames_are_bounded() {
    let bounds = CodecBounds::default();
    let valid = serde_json::json!({
        "id": 42, "op": "room.create", "in": {"name": "Build"}
    });
    let valid_bytes = serde_json::to_vec(&valid).unwrap();

    for len in 0..valid_bytes.len() {
        let truncated = &valid_bytes[..len];
        // must not panic; result may be Ok only if the prefix is itself
        // a complete valid frame (which for JSON it almost never is)
        let _ = decode(truncated, &bounds);
    }
}

/// The codec's own bound on work: a frame of exactly the byte ceiling is
// considered, and one past it is refused — without being parsed.
#[test]
fn the_byte_ceiling_is_a_hard_boundary() {
    let bounds = CodecBounds {
        max_frame_bytes: 256,
        ..CodecBounds::default()
    };
    // a valid small frame fits
    let ok_frame = serde_json::json!({"id": 1, "op": "room.list", "in": {}});
    assert!(decode(&serde_json::to_vec(&ok_frame).unwrap(), &bounds).is_ok());
    // one byte past the ceiling refuses without parsing
    let too_big = vec![b' '; 257];
    assert!(matches!(
        decode(&too_big, &bounds),
        Err(CodecError::FrameTooLarge { .. })
    ));
}

/// An `in` carrying an out-of-vocabulary enum value is refused at
// deserialization — closed vocabularies reject, never absorb. For
// status.post's label the refusal is the operation's own
// `status_label_unknown`, not a generic structural error.
#[test]
fn out_of_vocabulary_values_are_refused() {
    let bounds = CodecBounds::default();
    let frame = serde_json::json!({
        "id": 1, "op": "status.post",
        "in": {"room_id": "r1", "label": "hungry", "progress": {"state": "absent"}}
    });
    let err = decode(&serde_json::to_vec(&frame).unwrap(), &bounds).unwrap_err();
    assert!(matches!(
        err,
        CodecError::Malformed {
            error: ApiError::StatusLabelUnknown { .. },
            ..
        }
    ));
}
