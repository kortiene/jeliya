//! Protocol-v2 byte-stream records.
//!
//! This module owns only the structural Binary-message format. Stream
//! binding, sender direction, offset continuity, cumulative credit state,
//! cancellation ordering, and transfer lifecycle remain transport/runtime
//! concerns.

use crate::CodecBounds;
use jeliya_api::{RequestId, MAX_REQUEST_ID};
use std::num::NonZeroU128;
use thiserror::Error;

/// The fixed `JBS2` header length in bytes.
pub const STREAM_HEADER_BYTES: usize = 48;

/// The protocol-wide ceiling on one DATA payload.
pub const MAX_STREAM_DATA_BYTES: usize = 65_536;

const MAGIC: [u8; 4] = *b"JBS2";

/// The complete structural identity of one connection-local byte stream.
///
/// This type validates only the browser-safe request-id range and nonzero
/// stream id. Active lookup, connection ownership, uniqueness, and reuse are
/// runtime responsibilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StreamIdentity {
    request_id: RequestId,
    stream_id: NonZeroU128,
}

impl StreamIdentity {
    /// Constructs a structurally valid identity.
    pub fn new(request_id: u64, stream_id: u128) -> Result<Self, StreamCodecError> {
        let request_id =
            RequestId::new(request_id).map_err(|error| StreamCodecError::RequestIdOutOfRange {
                request_id: error.value,
            })?;
        let stream_id = NonZeroU128::new(stream_id).ok_or(StreamCodecError::ZeroStreamId)?;
        Ok(Self {
            request_id,
            stream_id,
        })
    }

    /// Returns the browser-safe request id.
    pub const fn request_id(self) -> RequestId {
        self.request_id
    }

    /// Returns the nonzero connection-local stream id.
    pub const fn stream_id(self) -> NonZeroU128 {
        self.stream_id
    }
}

/// Returns the largest DATA payload permitted by a served frame limit.
///
/// The subtraction is checked before use. Protocol-v2 byte streaming requires
/// room for the 48-byte header and at least one DATA byte, so limits at or
/// below 48 are rejected.
pub fn max_stream_data_bytes(max_frame_bytes: usize) -> Result<usize, StreamCodecError> {
    let payload_capacity = max_frame_bytes
        .checked_sub(STREAM_HEADER_BYTES)
        .filter(|capacity| *capacity > 0)
        .ok_or(StreamCodecError::FrameLimitTooSmall { max_frame_bytes })?;
    Ok(payload_capacity.min(MAX_STREAM_DATA_BYTES))
}

/// A decoded or to-be-encoded byte-stream record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamRecord {
    /// The request-id/stream-id pair this record belongs to.
    pub identity: StreamIdentity,
    /// The record's kind-specific structural fields.
    pub body: StreamRecordBody,
}

impl StreamRecord {
    /// Returns this record's closed kind.
    pub fn kind(&self) -> StreamRecordKind {
        self.body.kind()
    }
}

/// A decoded byte-stream record borrowing any DATA payload from its input.
///
/// This is the stateful runtime's full structural-decode surface after it has
/// validated the identity, bound the exact active pair, and checked the
/// record's kind against stream state and endpoint direction. Unlike
/// [`StreamRecord`], decoding this view never copies or allocates a DATA
/// payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamRecordView<'a> {
    /// The request-id/stream-id pair this record belongs to.
    pub identity: StreamIdentity,
    /// The record's kind-specific structural fields.
    pub body: StreamRecordBodyView<'a>,
}

impl StreamRecordView<'_> {
    /// Returns this record's closed kind.
    pub fn kind(&self) -> StreamRecordKind {
        self.body.kind()
    }
}

/// The kind-specific body of a byte-stream record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamRecordBody {
    /// Admission, carrying the expected total byte count.
    Open {
        /// Expected total bytes for the admitted transfer.
        total: u64,
    },
    /// A nonempty contiguous payload beginning at `offset`.
    Data {
        /// Zero-based offset of the payload's first byte.
        offset: u64,
        /// File bytes. Encoding enforces the served and protocol bounds.
        payload: Vec<u8>,
    },
    /// Cumulative receiver credit.
    Credit {
        /// Exclusive end of bytes accepted by the receiver's sink.
        accepted_through: u64,
        /// Exclusive end through which the producer may send.
        send_through: u64,
    },
    /// The producer's explicit end marker.
    End {
        /// Total bytes sent by the producer.
        total: u64,
    },
    /// A stream abort.
    Abort {
        /// Sender's accepted-byte high-water mark.
        accepted_through: u64,
        /// The sender's local abort reason.
        reason: BinaryAbortReason,
    },
    /// Acknowledgement of an ABORT.
    Ack {
        /// Final receiver-accepted byte count.
        accepted_through: u64,
    },
}

impl StreamRecordBody {
    fn kind(&self) -> StreamRecordKind {
        match self {
            Self::Open { .. } => StreamRecordKind::Open,
            Self::Data { .. } => StreamRecordKind::Data,
            Self::Credit { .. } => StreamRecordKind::Credit,
            Self::End { .. } => StreamRecordKind::End,
            Self::Abort { .. } => StreamRecordKind::Abort,
            Self::Ack { .. } => StreamRecordKind::Ack,
        }
    }
}

/// The kind-specific body of a borrowed byte-stream record.
///
/// Every control record is represented by the same values as
/// [`StreamRecordBody`]. DATA instead borrows its opaque bytes directly from
/// the complete Binary message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamRecordBodyView<'a> {
    /// Admission, carrying the expected total byte count.
    Open {
        /// Expected total bytes for the admitted transfer.
        total: u64,
    },
    /// A nonempty contiguous payload beginning at `offset`.
    Data {
        /// Zero-based offset of the payload's first byte.
        offset: u64,
        /// File bytes borrowed from the complete Binary message.
        payload: &'a [u8],
    },
    /// Cumulative receiver credit.
    Credit {
        /// Exclusive end of bytes accepted by the receiver's sink.
        accepted_through: u64,
        /// Exclusive end through which the producer may send.
        send_through: u64,
    },
    /// The producer's explicit end marker.
    End {
        /// Total bytes sent by the producer.
        total: u64,
    },
    /// A stream abort.
    Abort {
        /// Sender's accepted-byte high-water mark.
        accepted_through: u64,
        /// The sender's local abort reason.
        reason: BinaryAbortReason,
    },
    /// Acknowledgement of an ABORT.
    Ack {
        /// Final receiver-accepted byte count.
        accepted_through: u64,
    },
}

impl StreamRecordBodyView<'_> {
    fn kind(&self) -> StreamRecordKind {
        match self {
            Self::Open { .. } => StreamRecordKind::Open,
            Self::Data { .. } => StreamRecordKind::Data,
            Self::Credit { .. } => StreamRecordKind::Credit,
            Self::End { .. } => StreamRecordKind::End,
            Self::Abort { .. } => StreamRecordKind::Abort,
            Self::Ack { .. } => StreamRecordKind::Ack,
        }
    }

    fn into_owned(self) -> StreamRecordBody {
        match self {
            Self::Open { total } => StreamRecordBody::Open { total },
            Self::Data { offset, payload } => StreamRecordBody::Data {
                offset,
                payload: payload.to_vec(),
            },
            Self::Credit {
                accepted_through,
                send_through,
            } => StreamRecordBody::Credit {
                accepted_through,
                send_through,
            },
            Self::End { total } => StreamRecordBody::End { total },
            Self::Abort {
                accepted_through,
                reason,
            } => StreamRecordBody::Abort {
                accepted_through,
                reason,
            },
            Self::Ack { accepted_through } => StreamRecordBody::Ack { accepted_through },
        }
    }
}

/// The closed record-kind byte vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum StreamRecordKind {
    /// `0x01` OPEN.
    Open = 0x01,
    /// `0x02` DATA.
    Data = 0x02,
    /// `0x03` CREDIT.
    Credit = 0x03,
    /// `0x04` END.
    End = 0x04,
    /// `0x05` ABORT.
    Abort = 0x05,
    /// `0x06` ACK.
    Ack = 0x06,
}

impl StreamRecordKind {
    fn from_wire(value: u8) -> Result<Self, StreamCodecError> {
        match value {
            0x01 => Ok(Self::Open),
            0x02 => Ok(Self::Data),
            0x03 => Ok(Self::Credit),
            0x04 => Ok(Self::End),
            0x05 => Ok(Self::Abort),
            0x06 => Ok(Self::Ack),
            kind => Err(StreamCodecError::UnknownKind { kind }),
        }
    }

    fn wire(self) -> u8 {
        self as u8
    }
}

/// The closed numeric reason vocabulary carried by a Binary ABORT record.
///
/// This deliberately differs from `jeliya_api::StreamAbortReason`:
/// `OperationError` exists only on the wire, while `transport_lost` is
/// synthesized locally and therefore has no Binary value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u64)]
pub enum BinaryAbortReason {
    /// `0x01`: local cancellation.
    Cancelled = 0x01,
    /// `0x02`: the source could not produce bytes.
    SourceFailed = 0x02,
    /// `0x03`: the sink could not accept bytes.
    SinkFailed = 0x03,
    /// `0x04`: the sender detected a protocol violation.
    ProtocolError = 0x04,
    /// `0x05`: daemon-only; the following typed operation error is authoritative.
    OperationError = 0x05,
}

impl BinaryAbortReason {
    fn from_wire(value: u64) -> Result<Self, StreamCodecError> {
        match value {
            0x01 => Ok(Self::Cancelled),
            0x02 => Ok(Self::SourceFailed),
            0x03 => Ok(Self::SinkFailed),
            0x04 => Ok(Self::ProtocolError),
            0x05 => Ok(Self::OperationError),
            value => Err(StreamCodecError::InvalidAbortReason { value }),
        }
    }

    fn wire(self) -> u64 {
        self as u64
    }
}

/// A fixed numeric field in the `JBS2` header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StreamHeaderField {
    /// Header bytes 32 through 39.
    Offset,
    /// Header bytes 40 through 47.
    Value,
}

/// A structural byte-stream encode/decode failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum StreamCodecError {
    /// The served frame limit cannot carry a header plus one DATA byte.
    #[error("max_frame_bytes {max_frame_bytes} cannot frame a DATA record")]
    FrameLimitTooSmall {
        /// The rejected served limit.
        max_frame_bytes: usize,
    },
    /// The complete Binary message exceeds `max_frame_bytes`.
    #[error("Binary record is {actual_bytes} bytes, exceeding limit {limit_bytes}")]
    FrameTooLarge {
        /// Actual complete-message bytes.
        actual_bytes: usize,
        /// Served complete-message limit.
        limit_bytes: usize,
    },
    /// The Binary message ends before the fixed header does.
    #[error("Binary record is {actual_bytes} bytes, shorter than the 48-byte header")]
    HeaderTooShort {
        /// Actual complete-message bytes.
        actual_bytes: usize,
    },
    /// Header bytes 0 through 3 are not `JBS2`.
    #[error("Binary record has bad JBS2 magic")]
    BadMagic,
    /// At least one of the three reserved bytes is nonzero.
    #[error("Binary record has nonzero reserved bytes")]
    ReservedBytesNonzero,
    /// The kind byte is outside the closed `0x01..=0x06` vocabulary.
    #[error("Binary record has unknown kind {kind:#04x}")]
    UnknownKind {
        /// Rejected kind byte.
        kind: u8,
    },
    /// The request id exceeds the browser-safe JSON integer range.
    #[error("request id {request_id} exceeds {MAX_REQUEST_ID}")]
    RequestIdOutOfRange {
        /// Rejected request id.
        request_id: u64,
    },
    /// Stream ids are required to be nonzero.
    #[error("stream id is zero")]
    ZeroStreamId,
    /// A non-DATA record carried bytes after its fixed header.
    #[error("{kind:?} record carries a forbidden {payload_bytes}-byte payload")]
    UnexpectedPayload {
        /// Parsed control-record kind.
        kind: StreamRecordKind,
        /// Bytes present after the header.
        payload_bytes: usize,
    },
    /// DATA is nonempty by definition; END is the only empty terminator.
    #[error("DATA record has an empty payload")]
    EmptyData,
    /// A DATA payload exceeds the effective served/protocol bound.
    #[error("DATA payload is {actual_bytes} bytes, exceeding limit {limit_bytes}")]
    DataTooLarge {
        /// Actual DATA payload bytes.
        actual_bytes: usize,
        /// Effective payload limit.
        limit_bytes: usize,
    },
    /// A field required to be zero (or a fixed value) was not.
    #[error("{kind:?}.{field:?} has invalid value {value}; expected {expected}")]
    InvalidField {
        /// Parsed record kind.
        kind: StreamRecordKind,
        /// Invalid header field.
        field: StreamHeaderField,
        /// Rejected value.
        value: u64,
        /// The closed required numeric value.
        expected: u64,
    },
    /// CREDIT's accepted high-water mark exceeds its send limit.
    #[error("CREDIT accepted_through {accepted_through} exceeds send_through {send_through}")]
    InvalidCredit {
        /// Rejected accepted high-water mark.
        accepted_through: u64,
        /// Rejected send limit.
        send_through: u64,
    },
    /// ABORT's value is outside its closed reason vocabulary.
    #[error("ABORT has invalid reason {value}")]
    InvalidAbortReason {
        /// Rejected value field.
        value: u64,
    },
    /// DATA's exclusive end offset is not representable as `u64`.
    #[error("DATA offset {offset} plus payload length {payload_bytes} overflows u64")]
    OffsetOverflow {
        /// DATA start offset.
        offset: u64,
        /// DATA payload bytes.
        payload_bytes: usize,
    },
    /// Header-plus-payload length overflowed the platform's `usize`.
    #[error("Binary record length arithmetic overflowed")]
    RecordLengthOverflow,
}

/// Validates one complete Binary message through its stream identity.
///
/// Runtime code can call this first, perform the authoritative active-pair
/// lookup and direction/state checks, and only then call
/// [`decode_stream_record`]. This preserves the record's required validation
/// order without putting transport state in the codec and without allocating a
/// DATA payload before binding.
pub fn decode_stream_identity(
    bytes: &[u8],
    bounds: &CodecBounds,
) -> Result<StreamIdentity, StreamCodecError> {
    if bytes.len() > bounds.max_frame_bytes {
        return Err(StreamCodecError::FrameTooLarge {
            actual_bytes: bytes.len(),
            limit_bytes: bounds.max_frame_bytes,
        });
    }
    max_stream_data_bytes(bounds.max_frame_bytes)?;
    if bytes.len() < STREAM_HEADER_BYTES {
        return Err(StreamCodecError::HeaderTooShort {
            actual_bytes: bytes.len(),
        });
    }
    if bytes[..MAGIC.len()] != MAGIC {
        return Err(StreamCodecError::BadMagic);
    }

    let request_id = read_u64(bytes, 8);
    let stream_id = read_u128(bytes, 16);
    StreamIdentity::new(request_id, stream_id)
}

/// Validates one complete Binary message through its closed record kind.
///
/// Stateful runtimes call [`decode_stream_identity`] first, look up the exact
/// active `(request id, stream id)` pair, and only then call this helper. The
/// returned kind lets the runtime enforce stream state and sender direction
/// before [`decode_stream_record`] validates reserved bytes, fixed fields, and
/// payload rules. This helper deliberately repeats the size, header, magic, and
/// structural-identity checks so it is also safe to call independently.
///
/// No payload byte is inspected or allocated by this stage, including for a
/// DATA record.
pub fn decode_stream_kind(
    bytes: &[u8],
    bounds: &CodecBounds,
) -> Result<StreamRecordKind, StreamCodecError> {
    decode_stream_identity(bytes, bounds)?;
    StreamRecordKind::from_wire(bytes[4])
}

/// Decodes one complete Binary message into a borrowed byte-stream record.
///
/// No input-dependent allocation occurs. In particular, DATA payload bytes
/// remain a slice of `bytes`. Stateful runtimes must retain the result of
/// [`decode_stream_identity`], perform active binding lookup, call
/// [`decode_stream_kind`] for direction/state validation, and only then call
/// this full structural decoder. All fixed fields, payload bounds, and DATA
/// offset arithmetic are checked before the view is returned.
pub fn decode_stream_record_view<'a>(
    bytes: &'a [u8],
    bounds: &CodecBounds,
) -> Result<StreamRecordView<'a>, StreamCodecError> {
    let identity = decode_stream_identity(bytes, bounds)?;
    let max_payload = max_stream_data_bytes(bounds.max_frame_bytes)?;

    let kind = StreamRecordKind::from_wire(bytes[4])?;
    if bytes[5..8] != [0, 0, 0] {
        return Err(StreamCodecError::ReservedBytesNonzero);
    }

    let offset = read_u64(bytes, 32);
    let value = read_u64(bytes, 40);
    let payload = &bytes[STREAM_HEADER_BYTES..];

    let body = match kind {
        StreamRecordKind::Open => {
            require_no_payload(kind, payload)?;
            require_field(kind, StreamHeaderField::Offset, offset, 0)?;
            StreamRecordBodyView::Open { total: value }
        }
        StreamRecordKind::Data => {
            require_field(kind, StreamHeaderField::Value, value, 0)?;
            if payload.is_empty() {
                return Err(StreamCodecError::EmptyData);
            }
            if payload.len() > max_payload {
                return Err(StreamCodecError::DataTooLarge {
                    actual_bytes: payload.len(),
                    limit_bytes: max_payload,
                });
            }
            checked_data_end(offset, payload.len())?;
            StreamRecordBodyView::Data { offset, payload }
        }
        StreamRecordKind::Credit => {
            require_no_payload(kind, payload)?;
            if offset > value {
                return Err(StreamCodecError::InvalidCredit {
                    accepted_through: offset,
                    send_through: value,
                });
            }
            StreamRecordBodyView::Credit {
                accepted_through: offset,
                send_through: value,
            }
        }
        StreamRecordKind::End => {
            require_field(kind, StreamHeaderField::Value, value, 0)?;
            require_no_payload(kind, payload)?;
            StreamRecordBodyView::End { total: offset }
        }
        StreamRecordKind::Abort => {
            let reason = BinaryAbortReason::from_wire(value)?;
            require_no_payload(kind, payload)?;
            StreamRecordBodyView::Abort {
                accepted_through: offset,
                reason,
            }
        }
        StreamRecordKind::Ack => {
            require_field(kind, StreamHeaderField::Value, value, 0x05)?;
            require_no_payload(kind, payload)?;
            StreamRecordBodyView::Ack {
                accepted_through: offset,
            }
        }
    };

    Ok(StreamRecordView { identity, body })
}

/// Decodes one complete Binary message into an owned byte-stream record.
///
/// This uses [`decode_stream_record_view`] for all validation, then copies only
/// a valid DATA payload. The only input-dependent allocation is therefore a
/// DATA payload capped at 65,536 bytes. Stateful runtimes that can consume a
/// payload before the complete message is released should prefer the borrowed
/// decoder.
pub fn decode_stream_record(
    bytes: &[u8],
    bounds: &CodecBounds,
) -> Result<StreamRecord, StreamCodecError> {
    let view = decode_stream_record_view(bytes, bounds)?;
    Ok(StreamRecord {
        identity: view.identity,
        body: view.body.into_owned(),
    })
}

/// Encodes one typed byte-stream record as one complete Binary message.
///
/// Every structural invariant and the effective DATA bound is checked before
/// output allocation.
pub fn encode_stream_record(
    record: &StreamRecord,
    bounds: &CodecBounds,
) -> Result<Vec<u8>, StreamCodecError> {
    let max_payload = max_stream_data_bytes(bounds.max_frame_bytes)?;
    let kind = record.kind();
    let (offset, value, payload): (u64, u64, &[u8]) = match &record.body {
        StreamRecordBody::Open { total } => (0, *total, &[]),
        StreamRecordBody::Data { offset, payload } => {
            if payload.is_empty() {
                return Err(StreamCodecError::EmptyData);
            }
            let total_bytes = STREAM_HEADER_BYTES
                .checked_add(payload.len())
                .ok_or(StreamCodecError::RecordLengthOverflow)?;
            if total_bytes > bounds.max_frame_bytes {
                return Err(StreamCodecError::FrameTooLarge {
                    actual_bytes: total_bytes,
                    limit_bytes: bounds.max_frame_bytes,
                });
            }
            if payload.len() > max_payload {
                return Err(StreamCodecError::DataTooLarge {
                    actual_bytes: payload.len(),
                    limit_bytes: max_payload,
                });
            }
            checked_data_end(*offset, payload.len())?;
            (*offset, 0, payload)
        }
        StreamRecordBody::Credit {
            accepted_through,
            send_through,
        } => {
            if accepted_through > send_through {
                return Err(StreamCodecError::InvalidCredit {
                    accepted_through: *accepted_through,
                    send_through: *send_through,
                });
            }
            (*accepted_through, *send_through, &[])
        }
        StreamRecordBody::End { total } => (*total, 0, &[]),
        StreamRecordBody::Abort {
            accepted_through,
            reason,
        } => (*accepted_through, reason.wire(), &[]),
        StreamRecordBody::Ack { accepted_through } => (*accepted_through, 0x05, &[]),
    };

    let total_bytes = STREAM_HEADER_BYTES
        .checked_add(payload.len())
        .ok_or(StreamCodecError::RecordLengthOverflow)?;
    if total_bytes > bounds.max_frame_bytes {
        return Err(StreamCodecError::FrameTooLarge {
            actual_bytes: total_bytes,
            limit_bytes: bounds.max_frame_bytes,
        });
    }

    let mut bytes = Vec::with_capacity(total_bytes);
    bytes.extend_from_slice(&MAGIC);
    bytes.push(kind.wire());
    bytes.extend_from_slice(&[0, 0, 0]);
    bytes.extend_from_slice(&record.identity.request_id().get().to_be_bytes());
    bytes.extend_from_slice(&record.identity.stream_id().get().to_be_bytes());
    bytes.extend_from_slice(&offset.to_be_bytes());
    bytes.extend_from_slice(&value.to_be_bytes());
    bytes.extend_from_slice(payload);
    Ok(bytes)
}

fn require_no_payload(kind: StreamRecordKind, payload: &[u8]) -> Result<(), StreamCodecError> {
    if payload.is_empty() {
        Ok(())
    } else {
        Err(StreamCodecError::UnexpectedPayload {
            kind,
            payload_bytes: payload.len(),
        })
    }
}

fn require_field(
    kind: StreamRecordKind,
    field: StreamHeaderField,
    actual: u64,
    required: u64,
) -> Result<(), StreamCodecError> {
    if actual == required {
        Ok(())
    } else {
        Err(StreamCodecError::InvalidField {
            kind,
            field,
            value: actual,
            expected: required,
        })
    }
}

fn checked_data_end(offset: u64, payload_bytes: usize) -> Result<u64, StreamCodecError> {
    let payload_bytes_u64 =
        u64::try_from(payload_bytes).map_err(|_| StreamCodecError::OffsetOverflow {
            offset,
            payload_bytes,
        })?;
    offset
        .checked_add(payload_bytes_u64)
        .ok_or(StreamCodecError::OffsetOverflow {
            offset,
            payload_bytes,
        })
}

fn read_u64(bytes: &[u8], start: usize) -> u64 {
    let mut value = [0_u8; 8];
    value.copy_from_slice(&bytes[start..start + 8]);
    u64::from_be_bytes(value)
}

fn read_u128(bytes: &[u8], start: usize) -> u128 {
    let mut value = [0_u8; 16];
    value.copy_from_slice(&bytes[start..start + 16]);
    u128::from_be_bytes(value)
}
