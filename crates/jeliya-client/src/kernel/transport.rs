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
}
