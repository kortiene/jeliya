//! The authoritative room/session reconciler (#169): one bounded,
//! generation-fenced coordinator so that **every** detectable push gap,
//! reconnect, local fan-out overflow, and Android process-resume produces the
//! **same** authoritative re-baseline, and nothing else does.
//!
//! The reconciler sits *above* the seam: it consumes the seam's
//! [`EventSubscription`](crate::EventSubscription) and issues reads/resyncs
//! through [`ClientHandle::call`](crate::ClientHandle::call), whereas the kernel
//! (#168) sits *below* it. It is transport-independent — the four adapters
//! (#171/#172/#173) differ only in *which* lifecycle inputs occur, not in how
//! reconciliation runs — and, like the kernel, it is a **sans-IO core**
//! ([`core`]) wrapped by a thin async [`driver`].
//!
//! The public surface added here is small and additive: [`Reconciler`],
//! [`ReconcileConfig`]/[`ReconcileLimits`], [`ResyncReason`], [`ResyncRequired`],
//! [`RoomView`], [`RoomUpdate`], and [`ReconcileError`]. The seam's public
//! surface ([`ClientHandle`](crate::ClientHandle),
//! [`ClientEvent`](crate::ClientEvent), `EventSubscription`,
//! [`State`](crate::State), [`CallError`](crate::CallError)) remains
//! transport-independent and consumes the seam without changing its public
//! event vocabulary or call semantics. Adapter-private decode failures may be
//! routed to the internal `Input::DecodeFailed` path without changing the seam.
//!
//! This is the **only** gap/resync path for v2 clients; there is no legacy
//! `room.activate`-again bootstrap fallback (architecture Decision 4).

// The reconciler's internal machinery (the sans-IO `core`, `room`, and `buffer`
// state) is consumed by the `driver` and the reconciler's own fault suite. Some
// diagnostic accessors are used only by tests; rather than scatter per-item
// attributes, allow dead code within the reconciler module (mirroring the
// kernel module's allowance). Genuine dead-code linting returns for the whole
// module the moment an adapter consumes the last unexercised item.
#![allow(dead_code)]

mod buffer;
mod core;
mod diag;
mod driver;
mod reason;
mod room;
mod view;

pub use driver::{ReconcileError, Reconciler};
pub use reason::{ResyncReason, ResyncRequired};
pub use view::{RoomUpdate, RoomUpdateSubscription, RoomView};

/// The reconciler's hard bounds. Every field is explicit; none defaults to
/// "unbounded". The adapter/host chooses them, with the documented [`Default`]
/// as a conservative starting point.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ReconcileLimits {
    /// Max buffered live pushes per room during a baseline read. Overflow forces
    /// a fresh baseline (§R5), never a silent drop.
    pub buffer_depth: u32,
    /// Max buffered live-push bytes per room during a baseline read. A
    /// count-only bound is insufficient; the buffer is byte-bounded (§R5).
    pub buffer_bytes: u64,
    /// Minimum size of the per-room recent-`event_id` dedup FIFO. The actual
    /// window is at least `timeline_depth`, so every rendered id stays exact.
    pub dedup_window: u32,
    /// Maximum UTF-8 bytes accepted for any opaque room, event, subject, device,
    /// invite, file, pipe, or operation identifier retained by the reconciler.
    /// This turns every identifier-keyed count bound into a byte bound.
    pub max_identifier_bytes: u32,
    /// Maximum unique event identities in the supported history of one room
    /// and in one authoritative scan. Reaching it fails closed.
    pub max_baseline_events: u32,
    /// Maximum estimated bytes retained for each exact per-room history index
    /// and authoritative scan.
    pub baseline_dedup_bytes: u64,
    /// Max rooms the reconciler tracks at once; activation beyond it is refused
    /// with [`ReconcileError::TooManyRooms`], never silently dropped (§R15).
    pub max_active_rooms: u32,
    /// Maximum backend reads concurrently admitted by the driver across all
    /// rooms. Additional core requests wait in a bounded, deterministic queue;
    /// zero is normalized to one so an accepted room cannot deadlock forever.
    pub max_concurrent_reads: u32,
    /// Preferred page size for `room.timeline`; requests use the smaller of
    /// this value and `max_read_page_events`.
    pub read_page_size: u64,
    /// Maximum decoded events accepted in one `room.timeline` or
    /// `stream.resync` reply. This is distinct from `read_page_size` because
    /// `stream.resync` has no caller-supplied page limit.
    pub max_read_page_events: u32,
    /// Maximum serialized success-output bytes accepted from the backend for
    /// an authoritative read, checked before typed vector decoding. Concrete
    /// transports must separately cap frames before materializing `RawJson`.
    pub max_read_reply_bytes: u64,
    /// Maximum structural/scalar/string JSON tokens accepted before typed
    /// decoding, bounding allocation amplification from tiny nested elements.
    pub max_read_reply_tokens: u32,
    /// Max events retained in each room's rendered timeline window. The
    /// watermark remains independent, so trimming cannot hide a position gap.
    pub timeline_depth: u32,
    /// Max estimated bytes retained by a room's durable timeline or transient
    /// authoritative baseline tail. Both count and byte limits apply.
    pub timeline_bytes: u64,
    /// Max membership rows retained when live signed membership events arrive
    /// between authoritative roster reads.
    pub member_capacity: u32,
    /// Max per-device peer rows retained between authoritative presence reads.
    pub peer_capacity: u32,
    /// Max estimated ordinary payload bytes queued by each `RoomUpdate`
    /// subscription. One oversized latest authority may occupy a shared-`Arc`
    /// recovery allowance; repeated oversized updates replace it with an
    /// in-order loss marker rather than stacking.
    pub update_mailbox_bytes: u64,
}

impl Default for ReconcileLimits {
    /// Conservative operational defaults: at most 16 tracked rooms, four
    /// concurrent reads, sub-multi-MiB per-room buffers, and 256-event timeline
    /// requests. Every value is finite; none is "unbounded".
    fn default() -> Self {
        Self {
            buffer_depth: 1024,
            buffer_bytes: 256 * 1024,
            dedup_window: 256,
            max_identifier_bytes: 1024,
            max_baseline_events: 16_384,
            baseline_dedup_bytes: 1024 * 1024,
            max_active_rooms: 16,
            max_concurrent_reads: 4,
            read_page_size: 256,
            max_read_page_events: 1024,
            max_read_reply_bytes: 2 * 1024 * 1024,
            max_read_reply_tokens: 65_536,
            timeline_depth: 2048,
            timeline_bytes: 1024 * 1024,
            member_capacity: 256,
            peer_capacity: 256,
            update_mailbox_bytes: 2 * 1024 * 1024,
        }
    }
}

/// Reconciler construction inputs that are not limits.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ReconcileConfig {
    /// The reconciler's hard bounds.
    pub limits: ReconcileLimits,
}
