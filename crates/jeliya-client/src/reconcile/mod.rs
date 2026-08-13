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
//! [`State`](crate::State), [`CallError`](crate::CallError)) is **unchanged** —
//! the same "sufficient without a breaking change" discipline #168 proved for
//! `ClientBackend`.
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
    /// Size of the per-room recent-`event_id` dedup FIFO (a constant window; the
    /// watermark bounds the rest, §R7).
    pub dedup_window: u32,
    /// Max rooms the reconciler tracks at once; activation beyond it is refused
    /// with [`ReconcileError::TooManyRooms`], never silently dropped (§R15).
    pub max_active_rooms: u32,
    /// Page size for paged baseline reads (`room.timeline` / `stream.resync`),
    /// within the daemon's `timeline_page_max`.
    pub read_page_size: u64,
}

impl Default for ReconcileLimits {
    /// Conservative defaults: a 1024-push / 4 MiB reconcile buffer, a 256-id
    /// dedup window, 256 tracked rooms, and 256-event read pages. Every value is
    /// a finite bound; none is "unbounded".
    fn default() -> Self {
        Self {
            buffer_depth: 1024,
            buffer_bytes: 4 * 1024 * 1024,
            dedup_window: 256,
            max_active_rooms: 256,
            read_page_size: 256,
        }
    }
}

/// Reconciler construction inputs that are not limits.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ReconcileConfig {
    /// The reconciler's hard bounds.
    pub limits: ReconcileLimits,
}
