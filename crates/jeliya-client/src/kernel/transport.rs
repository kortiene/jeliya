//! The Transport/driver seam the adapters implement (§K13) and the frame types
//! that cross it.
//!
//! The kernel drives an abstract transport: it emits [`crate::kernel::core`]
//! `Action`s and the driver performs them, converting between the seam's erased
//! JSON text ([`RawJson`]) and the codec (#164) byte form at the driver
//! boundary. This module **defines** the seam; it implements **no** real
//! socket. `WsWeb` (#171), `WsNative` (#172), and `DirectClient` (#173) each
//! provide a concrete [`Transport`] + [`Driver`]; #168 provides only the
//! deterministic in-memory driver used by tests (see [`crate::kernel`]).

use std::task::{Context, Poll};

use jeliya_api::{ApiError, OpId, RequestId};

use crate::backend::RawJson;
use crate::kernel::diag::Redacted;
use crate::kernel::timing::Tick;

/// One already-encoded outbound request, as the kernel hands it to the
/// transport. It carries the correlation id, the operation name, the caller's
/// `op_id` (forwarded verbatim), and the serialized `in` bytes.
///
/// The `Debug` impl deliberately renders **only** the id and op — never the
/// `op_id` or the payload bytes (§K15) — so a stray `{:?}` in a driver log
/// cannot leak a dedup key or request body.
pub(crate) struct WireFrame {
    /// The reply-correlation id (`RequestId`).
    pub(crate) id: RequestId,
    /// The operation's wire name.
    pub(crate) op: &'static str,
    /// The caller's envelope `op_id`, if any — forwarded, never rendered.
    pub(crate) op_id: Option<OpId>,
    /// The serialized `in` object — carried, never rendered.
    pub(crate) input: RawJson,
}

impl std::fmt::Debug for WireFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WireFrame")
            .field("id", &self.id)
            .field("op", &self.op)
            // op_id and input are secret-bearing; never rendered.
            .field("op_id", &Redacted(()))
            .field("input", &Redacted(()))
            .finish()
    }
}

/// A parsed inbound reply body: either the erased success `out` bytes or a
/// typed daemon error. The driver decodes the codec frame into this before
/// feeding it to the core; a frame that does not parse to an envelope at all is
/// delivered as [`Inbound::Malformed`] instead.
pub(crate) enum WireReply {
    /// The erased success `out` bytes, delivered to the awaiting call.
    Ok(RawJson),
    /// A typed daemon verdict, surfaced as `CallError::Wire`.
    Err(ApiError),
}

/// The client control plane's closed ABORT-reason vocabulary (§S3/§S12). It is
/// a **tag only** — no free string, no name, no digest — so rendering it in a
/// diagnostic leaks nothing. It mirrors the subset of
/// `jeliya_codec::BinaryAbortReason` a *client* can author; the byte value of
/// each is the codec's concern at the driver boundary (#233), not the core's.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum StreamAbortReason {
    /// The client is unilaterally abandoning the transfer (cancel, or a
    /// client-side deadline/stall give-up — §S7/§12.2).
    Cancelled,
    /// The client's byte source could not produce bytes (producer).
    SourceFailed,
    /// The client's sink could not accept bytes (receiver).
    SinkFailed,
    /// The client detected a protocol violation in a bound record (§S10).
    ProtocolError,
}

/// One client-sendable byte-stream control record, as the kernel hands it to the
/// driver (§S3). The client **never** sends OPEN (daemon-only) and **never**
/// frames DATA itself — a producer's DATA is moved by the media seam
/// ([`crate::kernel::core::Action::ProduceData`]), not authored here. The driver
/// converts this intent into `JBS2` bytes via `jeliya-codec` at the boundary,
/// exactly as it converts [`WireFrame`] for text; the byte layout stays owned by
/// the codec (#233/#242/#243), never re-implemented in the core.
///
/// The `Debug` impl renders only the kind, ids, and numeric offsets — never a
/// payload, name, or free reason string (§S12); the reason is a closed tag.
pub(crate) enum StreamRecordIntent {
    /// Cumulative receiver credit (the client is the receiver).
    Credit {
        /// The stream's reply-correlation id.
        id: RequestId,
        /// The connection-local stream id adopted from OPEN.
        stream_id: u128,
        /// Exclusive end of bytes the client's sink has accepted.
        accepted_through: u64,
        /// Exclusive end through which the producer may send.
        send_through: u64,
    },
    /// The producer's explicit end marker (the client is the producer).
    End {
        /// The stream's reply-correlation id.
        id: RequestId,
        /// The connection-local stream id adopted from OPEN.
        stream_id: u128,
        /// Total bytes the client sent.
        offset: u64,
    },
    /// A client-initiated abort of the stream.
    Abort {
        /// The stream's reply-correlation id.
        id: RequestId,
        /// The connection-local stream id adopted from OPEN.
        stream_id: u128,
        /// The client's accepted/sent high-water mark at abort.
        high_water: u64,
        /// The closed abort reason tag.
        reason: StreamAbortReason,
    },
    /// Acknowledgement of a peer ABORT (ACK is ABORT-only, never on the success
    /// path — §S4).
    Ack {
        /// The stream's reply-correlation id.
        id: RequestId,
        /// The connection-local stream id adopted from OPEN.
        stream_id: u128,
        /// The client's final accepted/sent high-water mark.
        high_water: u64,
    },
}

impl std::fmt::Debug for StreamRecordIntent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Only routing facts and numeric offsets — never payload bytes, a name,
        // or a digest (none of which enter the byte-free core anyway, §S2/§S12).
        match self {
            StreamRecordIntent::Credit {
                id,
                stream_id,
                accepted_through,
                send_through,
            } => f
                .debug_struct("StreamRecordIntent::Credit")
                .field("id", id)
                .field("stream_id", stream_id)
                .field("accepted_through", accepted_through)
                .field("send_through", send_through)
                .finish(),
            StreamRecordIntent::End {
                id,
                stream_id,
                offset,
            } => f
                .debug_struct("StreamRecordIntent::End")
                .field("id", id)
                .field("stream_id", stream_id)
                .field("offset", offset)
                .finish(),
            StreamRecordIntent::Abort {
                id,
                stream_id,
                high_water,
                reason,
            } => f
                .debug_struct("StreamRecordIntent::Abort")
                .field("id", id)
                .field("stream_id", stream_id)
                .field("high_water", high_water)
                .field("reason", reason)
                .finish(),
            StreamRecordIntent::Ack {
                id,
                stream_id,
                high_water,
            } => f
                .debug_struct("StreamRecordIntent::Ack")
                .field("id", id)
                .field("stream_id", stream_id)
                .field("high_water", high_water)
                .finish(),
        }
    }
}

/// One inbound byte-stream record the driver decoded and hands the core, tagged
/// with its arrival generation for fencing (§S10). It is **byte-free**: a DATA
/// record carries only `offset`+`len` — the payload has already been routed to
/// the driver's sink buffer, so the core sees a bound, never bytes (§S2). A
/// codec-undecodable or unbindable Binary message never reaches here; the driver
/// drops it as [`Inbound::Malformed`] (connection-fatal conditions arrive as a
/// transport loss), exactly as for text — the connection-fatal vs stream-local
/// distinction is the codec/#233 rule, surfaced through this tagging.
///
/// The `Debug` impl renders only kind, ids, and offsets (§S12).
pub(crate) enum StreamRecordMeta {
    /// Daemon admission of the stream, carrying the authoritative total.
    Open {
        /// The stream's reply-correlation id.
        id: RequestId,
        /// The connection-local stream id.
        stream_id: u128,
        /// Expected total bytes for the transfer.
        total: u64,
    },
    /// Cumulative credit from the daemon (the client is the producer).
    Credit {
        /// The stream's reply-correlation id.
        id: RequestId,
        /// The connection-local stream id.
        stream_id: u128,
        /// Exclusive end of bytes the receiver has accepted.
        accepted_through: u64,
        /// Exclusive end through which the client may send.
        send_through: u64,
    },
    /// A DATA record's bounds (the client is the receiver). The payload is
    /// already in the driver's sink buffer; the core sees offset+len only.
    Data {
        /// The stream's reply-correlation id.
        id: RequestId,
        /// The connection-local stream id.
        stream_id: u128,
        /// Zero-based offset of the payload's first byte.
        offset: u64,
        /// The payload length in bytes.
        len: u64,
    },
    /// The daemon's explicit end marker (the client is the receiver).
    End {
        /// The stream's reply-correlation id.
        id: RequestId,
        /// The connection-local stream id.
        stream_id: u128,
        /// Total bytes the daemon sent.
        offset: u64,
    },
    /// A daemon-initiated abort of the stream.
    Abort {
        /// The stream's reply-correlation id.
        id: RequestId,
        /// The connection-local stream id.
        stream_id: u128,
        /// The daemon's accepted-byte high-water mark.
        high_water: u64,
        /// The closed abort reason tag.
        reason: StreamAbortReason,
    },
    /// Acknowledgement of the client's ABORT.
    Ack {
        /// The stream's reply-correlation id.
        id: RequestId,
        /// The connection-local stream id.
        stream_id: u128,
        /// The daemon's final accepted high-water mark.
        high_water: u64,
    },
}

impl StreamRecordMeta {
    /// The record's reply-correlation id — the key the core routes it by.
    pub(crate) fn id(&self) -> RequestId {
        match self {
            StreamRecordMeta::Open { id, .. }
            | StreamRecordMeta::Credit { id, .. }
            | StreamRecordMeta::Data { id, .. }
            | StreamRecordMeta::End { id, .. }
            | StreamRecordMeta::Abort { id, .. }
            | StreamRecordMeta::Ack { id, .. } => *id,
        }
    }
}

impl std::fmt::Debug for StreamRecordMeta {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StreamRecordMeta::Open {
                id,
                stream_id,
                total,
            } => f
                .debug_struct("StreamRecordMeta::Open")
                .field("id", id)
                .field("stream_id", stream_id)
                .field("total", total)
                .finish(),
            StreamRecordMeta::Credit {
                id,
                stream_id,
                accepted_through,
                send_through,
            } => f
                .debug_struct("StreamRecordMeta::Credit")
                .field("id", id)
                .field("stream_id", stream_id)
                .field("accepted_through", accepted_through)
                .field("send_through", send_through)
                .finish(),
            StreamRecordMeta::Data {
                id,
                stream_id,
                offset,
                len,
            } => f
                .debug_struct("StreamRecordMeta::Data")
                .field("id", id)
                .field("stream_id", stream_id)
                .field("offset", offset)
                .field("len", len)
                .finish(),
            StreamRecordMeta::End {
                id,
                stream_id,
                offset,
            } => f
                .debug_struct("StreamRecordMeta::End")
                .field("id", id)
                .field("stream_id", stream_id)
                .field("offset", offset)
                .finish(),
            StreamRecordMeta::Abort {
                id,
                stream_id,
                high_water,
                reason,
            } => f
                .debug_struct("StreamRecordMeta::Abort")
                .field("id", id)
                .field("stream_id", stream_id)
                .field("high_water", high_water)
                .field("reason", reason)
                .finish(),
            StreamRecordMeta::Ack {
                id,
                stream_id,
                high_water,
            } => f
                .debug_struct("StreamRecordMeta::Ack")
                .field("id", id)
                .field("stream_id", stream_id)
                .field("high_water", high_water)
                .finish(),
        }
    }
}

/// One inbound frame the driver hands the core, tagged with the connection
/// generation it arrived on so the core can fence stale-generation traffic
/// (§K7).
///
/// **Runtime scope (§K13):** the kernel defines this seam and ships only the
/// deterministic in-memory driver behind `test-transport`; the async runtime
/// loop that binds a real `Driver`'s transport, clock, and dialer to the core
/// — and the public construction path for it — lands with the first adapter
/// slice (#171), which consumes these types. The kernel deliberately
/// implements none of the three real transports.
pub(crate) enum Inbound {
    /// A reply correlated by `id`.
    Reply {
        /// The generation the delivering connection is on.
        generation: u64,
        /// The correlation id the reply answers.
        id: RequestId,
        /// The reply body.
        result: WireReply,
    },
    /// A live push, still in its **protocol shape** (`jeliya_api::Push`). The
    /// lift into the seam's [`crate::event::ClientEvent`] model happens
    /// inside the core: the
    /// wire can only produce protocol pushes, so lifecycle transitions and
    /// local-overflow signals are unrepresentable through this path — a driver
    /// or test controller cannot fabricate a `StateChanged` or `Lagged` that
    /// contradicts the core's own state.
    Push {
        /// The generation the delivering connection is on.
        generation: u64,
        /// The wire push.
        push: jeliya_api::Push,
    },
    /// One bound byte-stream record, tagged with its arrival generation (§S10).
    /// The driver has already decoded it via `jeliya-codec` and routed any DATA
    /// payload to its sink buffer, so the core receives offset+len metadata
    /// only. A stale-generation record is fenced in the core exactly as a stale
    /// reply is.
    Record {
        /// The generation the delivering connection is on.
        generation: u64,
        /// The decoded, byte-free record.
        record: StreamRecordMeta,
    },
    /// A frame that could not be parsed to an envelope with a usable `id`. It
    /// correlates to nothing, so it is dropped with a diagnostic and can strand
    /// no call (§K4).
    Malformed,
}

/// A transport error: the byte pipe is broken. The driver turns this into a
/// connection-loss input to the core (which then settles or holds outstanding
/// calls per §K6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TransportClosed;

/// One connection attempt's byte pipe, opened by the driver after it
/// resolves/authenticates and passes the generation gate. The kernel never
/// dials directly — it emits an `Action::Dial` and the driver calls the
/// adapter.
///
/// Object-safe and I/O-shaped, mirroring how `ClientBackend` erases the four
/// adapters. #171/#172/#173 implement it; #168 ships only the in-memory driver.
pub(crate) trait Transport: Send + 'static {
    /// Push one already-encoded frame toward the peer. Non-blocking; the driver
    /// owns any real back-pressure/flush. Returns `Err(TransportClosed)` if the
    /// pipe is broken, which the driver turns into a connection-loss input.
    fn send(&mut self, frame: WireFrame) -> Result<(), TransportClosed>;

    /// The next inbound frame, or the connection's end. Adapters map their
    /// native read into this; the driver tags each frame with the current
    /// generation before handing it to the core.
    fn poll_inbound(&mut self, cx: &mut Context<'_>) -> Poll<Option<Inbound>>;
}

/// What an adapter provides to build a kernel-backed `ClientBackend`: a dialer
/// (opens a [`Transport`], performs the generation gate, yields a generation),
/// plus the injected clock the driver uses. `DirectClient` (#173) supplies a
/// dialer that is always-ready and never reconnects.
///
/// The contract is **spawn-free**: the driver owns the event loop (the test
/// driver steps manually; real adapters spawn via the platform/supervisor, not
/// this crate), which keeps `wasm32-unknown-unknown` clean and determinism
/// total (§3 boundary invariants).
pub(crate) trait Driver: Send + 'static {
    /// The transport this driver opens on a successful dial.
    type Transport: Transport;

    /// Begin one dial. On success the driver reports `Connected` to the core
    /// with the fresh generation; on failure it reports a connection loss.
    fn dial(&mut self);

    /// Cancel any in-progress dial/backoff (total stop, §K11).
    fn cancel_dial(&mut self);

    /// The current logical time, read from the platform clock **outside** this
    /// library — never from `std::time` inside it.
    fn now(&self) -> Tick;
}

#[cfg(test)]
mod tests {
    use super::*;
    use jeliya_api::RequestId;

    #[test]
    fn wire_frame_debug_redacts_op_id_and_payload() {
        let frame = WireFrame {
            id: RequestId::new(7).unwrap(),
            op: "room.create",
            op_id: Some(OpId::new("secret-dedup-key")),
            input: RawJson::from_string(String::from("{\"name\":\"secret-room\"}")),
        };
        let rendered = format!("{frame:?}");
        // The routing facts a diagnostic may name.
        assert!(rendered.contains("room.create"));
        assert!(rendered.contains('7'));
        // The secret-bearing fields must never appear.
        assert!(!rendered.contains("secret-dedup-key"));
        assert!(!rendered.contains("secret-room"));
    }

    /// §S12: the stream records render only kind, ids, and numeric offsets — no
    /// payload, name, digest, or free reason string. (No payload/name field even
    /// exists on these types, §S2; this guards the `Debug` shape.)
    #[test]
    fn stream_record_debug_renders_only_ids_and_offsets() {
        let intent = StreamRecordIntent::Abort {
            id: RequestId::new(3).unwrap(),
            stream_id: 42,
            high_water: 100,
            reason: StreamAbortReason::Cancelled,
        };
        let rendered = format!("{intent:?}");
        assert!(rendered.contains("Abort"));
        assert!(rendered.contains("Cancelled"));
        assert!(rendered.contains("100"));

        let meta = StreamRecordMeta::Data {
            id: RequestId::new(3).unwrap(),
            stream_id: 42,
            offset: 64,
            len: 16,
        };
        let rendered = format!("{meta:?}");
        assert!(rendered.contains("Data"));
        assert!(rendered.contains("64"));
        assert!(rendered.contains("16"));
    }
}
