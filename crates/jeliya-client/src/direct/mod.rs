//! The Android in-process DirectClient adapter (#173) — the fourth kernel
//! adapter.
//!
//! DirectClient gives Android a [`ClientHandle`] whose backend calls **typed
//! `jeliya-core` in-process** through one bounded, serialized actor. The direct
//! data path contains no daemon-envelope JSON (`Engine::handle_frame`), no Dart,
//! no C ABI, no socket, and no token/portfile. It reuses the shared kernel
//! (#168), the reconciler (#169), the state model, and the error taxonomy so a
//! Dioxus component renders and reasons about the direct backend exactly as it
//! does the WebSocket backends. It is the production replacement for
//! `crates/jeliya-ffi/src/host.rs`.
//!
//! ## Design (records its choice, per the spec)
//!
//! This lands the **kernel path (D1)**: it reuses the bounded kernel [`Core`]
//! (#168) unchanged and adds the generic driver runtime (§5.1) in its
//! DirectClient shape — a degenerate [`DirectDriver`](driver::DirectDriverTask)
//! that dials once and never reconnects. Reusing `Core` makes the bounded queue,
//! in-flight throttle, per-call deadline, cancel-on-drop, exactly-once
//! settlement, and total-stop drain ordering *structural* rather than
//! re-derived, so the direct adapter's view-level outcomes match the WebSocket
//! adapters' by construction.
//!
//! ## The "no JSON" scope (§4)
//!
//! The engine is called only through `Engine::execute_with(TypedCall, …)` and
//! observed only through `subscribe_pushes() -> Push`. No bytes cross any
//! transport. The one residual `serde` is the in-process struct↔struct router
//! ([`bridge`]) the shared `ClientHandle` already forces at its edge — it
//! produces no bytes and is the same routing the daemon runs, which is what
//! guarantees WS parity.
//!
//! ## Boundaries
//!
//! Native-only and behind the default-off `direct` feature. Everything `tokio`,
//! clock, or engine lives here, never under `src/kernel/**` or
//! `src/reconcile/**`, so the sans-IO boundary scans stay green and the wasm /
//! `--no-default-features` dependency tree never pulls `tokio` or `jeliya-core`
//! (which transitively links `iroh-rooms`).

mod actor;
mod bridge;
mod driver;
mod ownership;
mod runtime;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{mpsc, watch};

use crate::handle::ClientHandle;
use crate::KernelLimits;

pub use ownership::OwnershipError;

/// The default depth of the bounded actor request channel (§9). Sized to the
/// kernel's default in-flight throttle so the kernel's admission bounds — not
/// this hand-off buffer — are the primary backpressure surface: the kernel never
/// has more than `in_flight` sends outstanding, so the channel cannot overflow
/// in correct operation.
const DEFAULT_REQUEST_CHANNEL_DEPTH: usize = 256;

/// The default depth of the bounded push forward channel. Matches the engine's
/// fixed broadcast capacity so a momentarily slow loop lags the broadcast (a
/// `ClientEvent::Lagged` → resync) rather than growing an unbounded buffer.
const DEFAULT_PUSH_CHANNEL_DEPTH: usize = 1024;

/// Construction inputs for a [`DirectClient`](connect_direct).
///
/// The `data_dir` is the canonical **new-generation** state directory
/// DirectClient opens (it never imports Flutter/Dart/C-ABI data — that removal is
/// #202). `limits` are the kernel's hard bounds; `jitter_seed` is carried for
/// contract uniformity with the reconnecting adapters and is unused here (the
/// driver never arms a backoff). The channel depths bound the actor's request
/// and push hand-off lanes (§9).
#[derive(Clone, Debug)]
pub struct DirectConfig {
    /// The canonical data directory the engine opens.
    pub data_dir: PathBuf,
    /// The kernel's hard bounds.
    pub limits: KernelLimits,
    /// Deterministic jitter seed — unused by the never-reconnecting driver, kept
    /// for uniformity with the socket adapters.
    pub jitter_seed: u64,
    /// The bounded actor request channel depth.
    pub request_channel_depth: usize,
    /// The bounded push forward channel depth.
    pub push_channel_depth: usize,
}

impl DirectConfig {
    /// A config for `data_dir` with conservative defaults.
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            limits: KernelLimits::default(),
            jitter_seed: 0,
            request_channel_depth: DEFAULT_REQUEST_CHANNEL_DEPTH,
            push_channel_depth: DEFAULT_PUSH_CHANNEL_DEPTH,
        }
    }
}

/// Open an in-process DirectClient over `config.data_dir`, returning the shared
/// [`ClientHandle`] every Dioxus component uses.
///
/// The returned handle is `Idle`; the caller drives its lifecycle with
/// [`ClientHandle::start`] / [`ClientHandle::stop`], exactly as for the socket
/// adapters. `start` opens the engine **once** (the serialized first-open); a
/// second live DirectClient over the same directory is refused (its `start`
/// lands `Failed`). Resume across app suspension is the reconciler's
/// [`Reconciler::resume`](crate::Reconciler) — DirectClient never fabricates a
/// reconnect.
///
/// **Runtime.** This spawns the driver loop onto the ambient tokio runtime, so it
/// must be called from within one (Android hosts the engine on its own runtime,
/// exactly as `FfiHost` does). The loop tears any live engine down and exits when
/// the last clone of the returned handle is dropped.
pub fn connect_direct(config: DirectConfig) -> ClientHandle {
    let origin = Instant::now();
    // The driver-message channel carries the core's side-effecting actions; its
    // occupancy is capped by the kernel's own queue + in-flight bounds, so this
    // size cannot be exceeded in correct operation.
    let driver_channel_depth =
        config.limits.queue_depth as usize + config.limits.in_flight as usize + 32;
    let (driver_tx, driver_rx) = mpsc::channel(driver_channel_depth);
    let (torn_down_tx, torn_down_rx) = watch::channel(false);

    let runtime = Arc::new(runtime::DirectRuntime::new(
        config.limits,
        config.jitter_seed,
        driver_tx,
        origin,
    ));

    let task = driver::DirectDriverTask::new(
        Arc::downgrade(&runtime),
        config.data_dir,
        config.request_channel_depth,
        config.push_channel_depth,
        driver_rx,
        origin,
        torn_down_tx,
    );
    tokio::spawn(task.run());

    ClientHandle::from_backend(Arc::new(runtime::DirectBackend::new(runtime, torn_down_rx)))
}

#[cfg(test)]
mod tests;
