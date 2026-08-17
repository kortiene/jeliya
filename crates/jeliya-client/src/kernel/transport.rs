//! The [`Driver`] seam the adapters implement (§K13) and the frame types that
//! cross it.
//!
//! The kernel drives an abstract transport: it emits [`crate::kernel::core`]
//! `Action`s and the driver performs them, converting between the seam's erased
//! JSON text ([`RawJson`]) and the codec (#164) byte form at the driver
//! boundary. This module **defines** the seam; it implements **no** real
//! socket. `WsWeb` (#171), `WsNative` (#172), and `DirectClient` (#173) each
//! provide a concrete [`Driver`] the generic runtime (`kernel/runtime.rs`)
//! binds to the core; #168 provides only the deterministic in-memory
//! controller used by tests (see [`crate::kernel`]).

use std::task::{Context, Poll};

use jeliya_api::{ApiError, OpId, RequestId};

use crate::backend::RawJson;
use crate::kernel::diag::Redacted;
use crate::kernel::timing::{Tick, TimerId};

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

/// One event a [`Driver`] surfaces to the runtime (`kernel/runtime.rs`), each
/// mapping one-to-one onto a core [`Input`](crate::kernel::core::Input). Every
/// dial outcome is token-fenced and every inbound frame is generation-tagged by
/// the driver **before** it becomes an event, so the core drops stragglers from
/// retired attempts and replaced connections (§K7).
pub(crate) enum DriverEvent {
    /// One inbound frame, already generation-tagged by the driver.
    Inbound(Inbound),
    /// The outstanding dial completed and passed the driver's own protocol
    /// validation (for `WsWeb`: a valid `hello`). Echoes the dial token.
    Connected {
        /// The completing dial attempt's token.
        token: u64,
    },
    /// The outstanding dial failed before connecting (recoverable). Echoes the
    /// dial token.
    DialFailed {
        /// The failing dial attempt's token.
        token: u64,
    },
    /// The outstanding dial was refused terminally (protocol/generation gate).
    /// Echoes the dial token.
    GateRefused {
        /// The refused dial attempt's token.
        token: u64,
    },
    /// A live connection was lost, tagged with the generation it was on.
    Interrupted {
        /// The lost transport's connection generation.
        generation: u64,
    },
    /// A driver timer fired.
    TimerFired(TimerId),
}

/// What an adapter provides so the generic runtime (`kernel/runtime.rs`) can
/// bind it to the sans-IO core: a combined **event source** ([`poll_event`])
/// and **action sink** (`send`/`dial`/`arm_timer`/…), plus the injected clock.
/// `WsWeb` (#171) is the first implementor; `WsNative` (#172) and
/// `DirectClient` (#173) reuse the same runtime.
///
/// **Not `Send`.** The bound is deliberately relaxed from the seam's
/// `ClientBackend: Send + Sync`: on `wasm32-unknown-unknown` a driver holds
/// `!Send` browser handles (`web_sys::WebSocket`, JS `Closure`s). The runtime
/// confines the driver to a `spawn_local`'d pump and keeps the `Send + Sync`
/// backend half free of it (§5, the mailbox split); native drivers remain
/// `Send` in practice. The contract is still **spawn-free at this seam**: the
/// platform spawns the pump (`spawn_local` on wasm; the supervisor runtime on
/// native), never this library.
///
/// [`poll_event`]: Driver::poll_event
pub(crate) trait Driver: 'static {
    /// The next event from the transport/dialer/timer, or `Pending`. The
    /// runtime registers `cx`'s waker so a JS callback (or a native read) can
    /// wake the pump when the next event is ready.
    fn poll_event(&mut self, cx: &mut Context<'_>) -> Poll<DriverEvent>;

    /// Push one already-encoded frame toward the peer. Non-blocking. Returns
    /// `Err(TransportClosed)` if the pipe is broken, which the runtime turns
    /// into an `Interrupted` input (the send/close race, §K14).
    fn send(&mut self, frame: WireFrame) -> Result<(), TransportClosed>;

    /// Begin one dial identified by `token`. Every outcome the driver later
    /// surfaces ([`DriverEvent::Connected`] / [`DriverEvent::DialFailed`] /
    /// [`DriverEvent::GateRefused`]) echoes this token, so the core can fence a
    /// straggler from a retired attempt.
    fn dial(&mut self, token: u64);

    /// Cancel any in-progress dial/backoff (total stop, §K11).
    fn cancel_dial(&mut self);

    /// Arm a driver timer to fire at `at` (logical time), feeding
    /// [`DriverEvent::TimerFired`] when it does.
    fn arm_timer(&mut self, id: TimerId, at: Tick);

    /// Cancel a previously-armed driver timer.
    fn cancel_timer(&mut self, id: TimerId);

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
