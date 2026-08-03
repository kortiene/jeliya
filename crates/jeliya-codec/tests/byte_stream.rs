//! Spec-authored protocol-v2 Binary-record vectors and structural boundaries.

use jeliya_codec::{
    decode_stream_identity, decode_stream_record, encode_stream_record, max_stream_data_bytes,
    BinaryAbortReason, CodecBounds, StreamCodecError, StreamHeaderField, StreamIdentity,
    StreamRecord, StreamRecordBody, StreamRecordKind, MAX_REQUEST_ID, MAX_STREAM_DATA_BYTES,
    STREAM_HEADER_BYTES,
};

const REQUEST_ID: u64 = 0x0001_0203_0405_0607;
const STREAM_ID: u128 = 0x1011_1213_1415_1617_1819_1a1b_1c1d_1e1f;

fn bounds(max_frame_bytes: usize) -> CodecBounds {
    CodecBounds {
        max_frame_bytes,
        ..CodecBounds::default()
    }
}

fn record(body: StreamRecordBody) -> StreamRecord {
    StreamRecord {
        identity: StreamIdentity::new(REQUEST_ID, STREAM_ID).unwrap(),
        body,
    }
}

/// Constructs a raw vector directly from the normative field table. This is
/// deliberately independent of the codec's encoder.
fn wire(
    kind: u8,
    request_id: u64,
    stream_id: u128,
    offset: u64,
    value: u64,
    payload: &[u8],
) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(STREAM_HEADER_BYTES + payload.len());
    bytes.extend_from_slice(b"JBS2");
    bytes.push(kind);
    bytes.extend_from_slice(&[0, 0, 0]);
    bytes.extend_from_slice(&request_id.to_be_bytes());
    bytes.extend_from_slice(&stream_id.to_be_bytes());
    bytes.extend_from_slice(&offset.to_be_bytes());
    bytes.extend_from_slice(&value.to_be_bytes());
    bytes.extend_from_slice(payload);
    bytes
}

#[test]
fn header_is_exactly_48_bytes_and_big_endian() {
    // Hand-transcribed from docs/protocol-v2.md#the-binary-record. No codec
    // behavior was used to construct this expected vector.
    let expected = vec![
        0x4a, 0x42, 0x53, 0x32, // JBS2
        0x02, 0x00, 0x00, 0x00, // DATA + reserved
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, // request id
        0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, // stream id, high
        0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f, // stream id, low
        0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, // offset
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // DATA value
        0xaa, 0xbb, // payload
    ];
    let encoded = encode_stream_record(
        &record(StreamRecordBody::Data {
            offset: 0x2021_2223_2425_2627,
            payload: vec![0xaa, 0xbb],
        }),
        &bounds(1024),
    )
    .unwrap();
    assert_eq!(encoded, expected);
    assert_eq!(STREAM_HEADER_BYTES, 48);
}

#[test]
fn every_record_kind_matches_the_record_and_round_trips() {
    let cases = [
        (
            record(StreamRecordBody::Open { total: 12 }),
            wire(0x01, REQUEST_ID, STREAM_ID, 0, 12, &[]),
        ),
        (
            record(StreamRecordBody::Data {
                offset: 7,
                payload: vec![0xde, 0xad, 0xbe, 0xef],
            }),
            wire(0x02, REQUEST_ID, STREAM_ID, 7, 0, &[0xde, 0xad, 0xbe, 0xef]),
        ),
        (
            record(StreamRecordBody::Credit {
                accepted_through: 8,
                send_through: 21,
            }),
            wire(0x03, REQUEST_ID, STREAM_ID, 8, 21, &[]),
        ),
        (
            record(StreamRecordBody::End { total: 12 }),
            wire(0x04, REQUEST_ID, STREAM_ID, 12, 0, &[]),
        ),
        (
            record(StreamRecordBody::Abort {
                accepted_through: 8,
                reason: BinaryAbortReason::ProtocolError,
            }),
            wire(0x05, REQUEST_ID, STREAM_ID, 8, 0x04, &[]),
        ),
        (
            record(StreamRecordBody::Ack {
                accepted_through: 8,
            }),
            wire(0x06, REQUEST_ID, STREAM_ID, 8, 0x05, &[]),
        ),
    ];
    let bounds = bounds(1024);

    for (expected_record, expected_bytes) in cases {
        assert_eq!(
            encode_stream_record(&expected_record, &bounds).unwrap(),
            expected_bytes,
            "{:?}",
            expected_record.kind()
        );
        assert_eq!(
            decode_stream_record(&expected_bytes, &bounds).unwrap(),
            expected_record
        );
    }
}

#[test]
fn data_payload_helper_checks_subtraction_and_protocol_cap() {
    for invalid in [0, 1, 47, 48] {
        assert_eq!(
            max_stream_data_bytes(invalid),
            Err(StreamCodecError::FrameLimitTooSmall {
                max_frame_bytes: invalid
            })
        );
    }
    assert_eq!(max_stream_data_bytes(49), Ok(1));
    assert_eq!(max_stream_data_bytes(65_583), Ok(65_535));
    assert_eq!(max_stream_data_bytes(65_584), Ok(65_536));
    assert_eq!(max_stream_data_bytes(65_585), Ok(65_536));
    assert_eq!(max_stream_data_bytes(usize::MAX), Ok(65_536));

    let control = record(StreamRecordBody::Open { total: 0 });
    let control_wire = wire(0x01, REQUEST_ID, STREAM_ID, 0, 0, &[]);
    for invalid in [0, 48] {
        let invalid_bounds = bounds(invalid);
        assert_eq!(
            encode_stream_record(&control, &invalid_bounds),
            Err(StreamCodecError::FrameLimitTooSmall {
                max_frame_bytes: invalid,
            })
        );
        assert_eq!(
            decode_stream_record(
                if invalid == 0 { &[] } else { &control_wire },
                &invalid_bounds,
            ),
            Err(StreamCodecError::FrameLimitTooSmall {
                max_frame_bytes: invalid,
            })
        );
    }
}

#[test]
fn data_lengths_cover_empty_exact_and_one_past_boundaries() {
    let one_byte_bounds = bounds(49);
    let one = record(StreamRecordBody::Data {
        offset: 0,
        payload: vec![1],
    });
    assert_eq!(
        encode_stream_record(&one, &one_byte_bounds).unwrap().len(),
        49
    );
    assert_eq!(
        decode_stream_record(
            &wire(0x02, REQUEST_ID, STREAM_ID, 0, 0, &[1]),
            &one_byte_bounds
        )
        .unwrap(),
        one
    );

    let empty = record(StreamRecordBody::Data {
        offset: 0,
        payload: Vec::new(),
    });
    assert_eq!(
        encode_stream_record(&empty, &one_byte_bounds),
        Err(StreamCodecError::EmptyData)
    );
    assert_eq!(
        decode_stream_record(
            &wire(0x02, REQUEST_ID, STREAM_ID, 0, 0, &[]),
            &one_byte_bounds
        ),
        Err(StreamCodecError::EmptyData)
    );

    let full_payload = vec![0x5a; MAX_STREAM_DATA_BYTES];
    let at_limit = wire(0x02, REQUEST_ID, STREAM_ID, 0, 0, &full_payload);
    let protocol_bounds = bounds(STREAM_HEADER_BYTES + MAX_STREAM_DATA_BYTES + 1);
    assert!(decode_stream_record(&at_limit, &protocol_bounds).is_ok());
    assert_eq!(
        encode_stream_record(
            &record(StreamRecordBody::Data {
                offset: 0,
                payload: full_payload,
            }),
            &protocol_bounds,
        )
        .unwrap(),
        at_limit
    );

    let one_past_payload = vec![0x5a; MAX_STREAM_DATA_BYTES + 1];
    let one_past = wire(0x02, REQUEST_ID, STREAM_ID, 0, 0, &one_past_payload);
    assert_eq!(
        decode_stream_record(&one_past, &protocol_bounds),
        Err(StreamCodecError::DataTooLarge {
            actual_bytes: MAX_STREAM_DATA_BYTES + 1,
            limit_bytes: MAX_STREAM_DATA_BYTES,
        })
    );
    assert_eq!(
        encode_stream_record(
            &record(StreamRecordBody::Data {
                offset: 0,
                payload: one_past_payload,
            }),
            &protocol_bounds,
        ),
        Err(StreamCodecError::DataTooLarge {
            actual_bytes: MAX_STREAM_DATA_BYTES + 1,
            limit_bytes: MAX_STREAM_DATA_BYTES,
        })
    );
}

#[test]
fn complete_message_bound_is_checked_before_header_parsing() {
    let bounds = bounds(49);
    let at_limit = wire(0x02, REQUEST_ID, STREAM_ID, 0, 0, &[1]);
    assert!(decode_stream_record(&at_limit, &bounds).is_ok());

    let mut one_past_with_bad_magic = vec![0; 50];
    one_past_with_bad_magic[..4].copy_from_slice(b"NOPE");
    assert_eq!(
        decode_stream_record(&one_past_with_bad_magic, &bounds),
        Err(StreamCodecError::FrameTooLarge {
            actual_bytes: 50,
            limit_bytes: 49,
        })
    );

    let two_bytes = record(StreamRecordBody::Data {
        offset: 0,
        payload: vec![1, 2],
    });
    assert_eq!(
        encode_stream_record(&two_bytes, &bounds),
        Err(StreamCodecError::FrameTooLarge {
            actual_bytes: 50,
            limit_bytes: 49,
        })
    );
    assert_eq!(
        decode_stream_record(&wire(0x02, REQUEST_ID, STREAM_ID, 0, 0, &[1, 2]), &bounds,),
        Err(StreamCodecError::FrameTooLarge {
            actual_bytes: 50,
            limit_bytes: 49,
        })
    );
}

#[test]
fn every_short_header_is_rejected_without_parsing() {
    let full = wire(0x01, REQUEST_ID, STREAM_ID, 0, 0, &[]);
    for len in 0..STREAM_HEADER_BYTES {
        assert_eq!(
            decode_stream_record(&full[..len], &bounds(1024)),
            Err(StreamCodecError::HeaderTooShort { actual_bytes: len }),
            "prefix length {len}"
        );
    }
}

#[test]
fn bad_magic_reserved_bytes_and_unknown_kinds_are_closed() {
    let valid = wire(0x01, REQUEST_ID, STREAM_ID, 0, 0, &[]);

    for index in 0..4 {
        let mut bad_magic = valid.clone();
        bad_magic[index] ^= 0xff;
        assert_eq!(
            decode_stream_record(&bad_magic, &bounds(1024)),
            Err(StreamCodecError::BadMagic),
            "magic byte {index}"
        );
    }

    for index in 5..8 {
        let mut reserved = valid.clone();
        reserved[index] = 1;
        assert_eq!(
            decode_stream_record(&reserved, &bounds(1024)),
            Err(StreamCodecError::ReservedBytesNonzero),
            "reserved byte {index}"
        );
    }

    for kind in [0x00, 0x07, 0xff] {
        let unknown = wire(kind, REQUEST_ID, STREAM_ID, 0, 0, &[]);
        assert_eq!(
            decode_stream_record(&unknown, &bounds(1024)),
            Err(StreamCodecError::UnknownKind { kind })
        );
    }
}

#[test]
fn request_and_stream_id_boundaries_are_exact() {
    for request_id in [0, MAX_REQUEST_ID] {
        let bytes = wire(0x01, request_id, 1, 0, 0, &[]);
        let decoded = decode_stream_record(&bytes, &bounds(1024)).unwrap();
        assert_eq!(decoded.identity.request_id().get(), request_id);
        assert_eq!(decoded.identity.stream_id().get(), 1);
    }

    let request_one_past = wire(0x01, MAX_REQUEST_ID + 1, 1, 0, 0, &[]);
    assert_eq!(
        decode_stream_record(&request_one_past, &bounds(1024)),
        Err(StreamCodecError::RequestIdOutOfRange {
            request_id: MAX_REQUEST_ID + 1
        })
    );

    for stream_id in [1, u128::MAX] {
        let bytes = wire(0x01, 0, stream_id, 0, 0, &[]);
        assert_eq!(
            decode_stream_record(&bytes, &bounds(1024))
                .unwrap()
                .identity
                .stream_id()
                .get(),
            stream_id
        );
    }
    assert_eq!(
        decode_stream_record(&wire(0x01, 0, 0, 0, 0, &[]), &bounds(1024)),
        Err(StreamCodecError::ZeroStreamId)
    );

    assert_eq!(
        StreamIdentity::new(MAX_REQUEST_ID + 1, 1),
        Err(StreamCodecError::RequestIdOutOfRange {
            request_id: MAX_REQUEST_ID + 1
        })
    );
    assert_eq!(
        StreamIdentity::new(0, 0),
        Err(StreamCodecError::ZeroStreamId)
    );
}

#[test]
fn identity_can_be_validated_before_binding_and_body_parsing() {
    for mut bytes in [
        wire(0xff, REQUEST_ID, STREAM_ID, 0, 0, &[]),
        wire(0x02, REQUEST_ID, STREAM_ID, 0, 1, &[0xaa]),
    ] {
        let identity = decode_stream_identity(&bytes, &bounds(1024)).unwrap();
        assert_eq!(identity.request_id().get(), REQUEST_ID);
        assert_eq!(identity.stream_id().get(), STREAM_ID);

        // Reserved bytes are also intentionally after the binding stage.
        bytes[5] = 1;
        assert_eq!(
            decode_stream_identity(&bytes, &bounds(1024)).unwrap(),
            identity
        );
        assert!(decode_stream_record(&bytes, &bounds(1024)).is_err());
    }
}

#[test]
fn control_records_are_exactly_header_length() {
    for kind in [0x01, 0x03, 0x04, 0x05, 0x06] {
        let value = match kind {
            0x05 => 0x01,
            0x06 => 0x05,
            _ => 0,
        };
        let with_payload = wire(kind, REQUEST_ID, STREAM_ID, 0, value, &[0xaa]);
        assert_eq!(
            decode_stream_record(&with_payload, &bounds(1024)),
            Err(StreamCodecError::UnexpectedPayload {
                kind: match kind {
                    0x01 => StreamRecordKind::Open,
                    0x03 => StreamRecordKind::Credit,
                    0x04 => StreamRecordKind::End,
                    0x05 => StreamRecordKind::Abort,
                    0x06 => StreamRecordKind::Ack,
                    _ => unreachable!(),
                },
                payload_bytes: 1,
            })
        );
    }
}

#[test]
fn fixed_fields_credit_abort_and_ack_are_validated() {
    assert!(matches!(
        decode_stream_record(&wire(0x01, REQUEST_ID, STREAM_ID, 1, 0, &[]), &bounds(1024)),
        Err(StreamCodecError::InvalidField {
            kind: StreamRecordKind::Open,
            field: StreamHeaderField::Offset,
            ..
        })
    ));
    assert!(matches!(
        decode_stream_record(
            &wire(0x02, REQUEST_ID, STREAM_ID, 0, 1, &[1]),
            &bounds(1024)
        ),
        Err(StreamCodecError::InvalidField {
            kind: StreamRecordKind::Data,
            field: StreamHeaderField::Value,
            ..
        })
    ));
    assert!(matches!(
        decode_stream_record(&wire(0x04, REQUEST_ID, STREAM_ID, 0, 1, &[]), &bounds(1024)),
        Err(StreamCodecError::InvalidField {
            kind: StreamRecordKind::End,
            field: StreamHeaderField::Value,
            ..
        })
    ));

    let equal_credit = wire(0x03, REQUEST_ID, STREAM_ID, 7, 7, &[]);
    assert!(decode_stream_record(&equal_credit, &bounds(1024)).is_ok());
    assert_eq!(
        decode_stream_record(&wire(0x03, REQUEST_ID, STREAM_ID, 8, 7, &[]), &bounds(1024)),
        Err(StreamCodecError::InvalidCredit {
            accepted_through: 8,
            send_through: 7,
        })
    );
    assert_eq!(
        encode_stream_record(
            &record(StreamRecordBody::Credit {
                accepted_through: 8,
                send_through: 7,
            }),
            &bounds(1024),
        ),
        Err(StreamCodecError::InvalidCredit {
            accepted_through: 8,
            send_through: 7,
        })
    );

    for (value, expected) in [
        (1, BinaryAbortReason::Cancelled),
        (2, BinaryAbortReason::SourceFailed),
        (3, BinaryAbortReason::SinkFailed),
        (4, BinaryAbortReason::ProtocolError),
        (5, BinaryAbortReason::OperationError),
    ] {
        let expected_wire = wire(0x05, REQUEST_ID, STREAM_ID, 0, value, &[]);
        assert_eq!(
            decode_stream_record(&expected_wire, &bounds(1024))
                .unwrap()
                .body,
            StreamRecordBody::Abort {
                accepted_through: 0,
                reason: expected,
            }
        );
        assert_eq!(
            encode_stream_record(
                &record(StreamRecordBody::Abort {
                    accepted_through: 0,
                    reason: expected,
                }),
                &bounds(1024),
            )
            .unwrap(),
            expected_wire
        );
    }
    for value in [0, 6, u64::MAX] {
        assert_eq!(
            decode_stream_record(
                &wire(0x05, REQUEST_ID, STREAM_ID, 0, value, &[]),
                &bounds(1024)
            ),
            Err(StreamCodecError::InvalidAbortReason { value })
        );
    }

    assert!(decode_stream_record(
        &wire(0x06, REQUEST_ID, STREAM_ID, 0, 0x05, &[]),
        &bounds(1024)
    )
    .is_ok());
    for value in [0, 0x04, 0x06, u64::MAX] {
        assert!(matches!(
            decode_stream_record(
                &wire(0x06, REQUEST_ID, STREAM_ID, 0, value, &[]),
                &bounds(1024)
            ),
            Err(StreamCodecError::InvalidField {
                kind: StreamRecordKind::Ack,
                field: StreamHeaderField::Value,
                ..
            })
        ));
    }
}

#[test]
fn data_offset_arithmetic_is_checked_at_exact_u64_boundary() {
    let exact = record(StreamRecordBody::Data {
        offset: u64::MAX - 1,
        payload: vec![0xaa],
    });
    let exact_wire = wire(0x02, REQUEST_ID, STREAM_ID, u64::MAX - 1, 0, &[0xaa]);
    assert_eq!(
        decode_stream_record(&exact_wire, &bounds(1024)).unwrap(),
        exact
    );
    assert!(encode_stream_record(&exact, &bounds(1024)).is_ok());

    let overflow_wire = wire(0x02, REQUEST_ID, STREAM_ID, u64::MAX, 0, &[0xaa]);
    assert_eq!(
        decode_stream_record(&overflow_wire, &bounds(1024)),
        Err(StreamCodecError::OffsetOverflow {
            offset: u64::MAX,
            payload_bytes: 1,
        })
    );
    assert_eq!(
        encode_stream_record(
            &record(StreamRecordBody::Data {
                offset: u64::MAX,
                payload: vec![0xaa],
            }),
            &bounds(1024),
        ),
        Err(StreamCodecError::OffsetOverflow {
            offset: u64::MAX,
            payload_bytes: 1,
        })
    );
}

#[test]
fn data_payload_is_opaque_and_does_not_imply_concatenated_records_or_resume() {
    let payload = b"prefixJBS2\x06suffix".to_vec();
    let expected = record(StreamRecordBody::Data {
        offset: 913,
        payload: payload.clone(),
    });
    let decoded = decode_stream_record(
        &wire(0x02, REQUEST_ID, STREAM_ID, 913, 0, &payload),
        &bounds(1024),
    )
    .unwrap();
    assert_eq!(decoded, expected);
}
