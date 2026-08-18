//! The browser WebSocket/session adapter (`WsWeb`, #171).
//!
//! `WsWeb` binds the shared client kernel (#168) and codec (#164) to the
//! browser `WebSocket` and `fetch` APIs, producing a real
//! [`ClientHandle`](crate::ClientHandle) the shared Dioxus UI drives on
//! `wasm32-unknown-unknown` — with **no Iroh, no `jeliya-core`, no native
//! transport, no desktop supervision** in the wasm dependency graph. It is the
//! sole browser client for the v2 stack (clean-slate cutover); React transport
//! compatibility is not a goal.
//!
//! The whole module is gated `#[cfg(all(target_arch = "wasm32", feature =
//! "ws-web"))]` at its declaration in `lib.rs`, so it never enters the native
//! library tree the `cargo tree` boundary test scans, and its `!Send` browser
//! concerns are confined here (§4.2, §5).
//!
//! Flipping `jeliya-ui::compose::web_composition` from the mock to
//! [`connect_ws_web`] is the production UI cutover — an explicit non-goal of
//! this slice.

mod diag;
mod session;
mod socket;
mod timers;

use std::rc::Rc;

use crate::handle::ClientHandle;
use crate::KernelConfig;

pub use session::{Endpoint, ExplicitResolver, GetTokenResolver, SessionError, SessionResolver};

use socket::WsWebDriver;

/// Configuration for [`connect_ws_web`].
pub struct WsWebConfig {
    /// Where the daemon lives and how its browser endpoints are derived.
    pub endpoint: Endpoint,
    /// How the connect credential is obtained and named on the URL. The
    /// production default is [`GetTokenResolver`]; the #166 ticket flow swaps a
    /// `?ct=` resolver in here with no transport change.
    pub session: Box<dyn SessionResolver>,
    /// The kernel bounds and jitter/replay policy. `stable_principal` is forced
    /// off and `jitter_seed` is seeded from `crypto.getRandomValues` when left
    /// at zero (§6.8).
    pub kernel: KernelConfig,
    /// How long to wait for a valid `hello` after opening the socket before
    /// treating the attempt as failed (§6.1 step 4).
    pub hello_timeout_ms: u32,
}

impl WsWebConfig {
    /// A same-origin production configuration: derive endpoints from
    /// `window.location` and fetch a fresh `/api/session` token per attempt.
    pub fn same_origin() -> Self {
        Self {
            endpoint: Endpoint::SameOrigin,
            session: Box::new(GetTokenResolver),
            kernel: KernelConfig::default(),
            hello_timeout_ms: 10_000,
        }
    }
}

/// Build a browser-backed [`ClientHandle`] and start its pump.
///
/// Returns immediately with a live handle the UI can `start()`/`call()`; the
/// `!Send` IO pump is `spawn_local`'d and drives the transport thereafter.
///
/// `stable_principal` is forced to `false` (no cross-reconnect mutation replay
/// until the daemon-incarnation fence, #270); a zero `jitter_seed` is replaced
/// with `crypto.getRandomValues` entropy so reconnect storms decorrelate
/// (§6.8).
pub fn connect_ws_web(config: WsWebConfig) -> ClientHandle {
    let mut kernel = config.kernel;
    kernel.stable_principal = false;
    if kernel.jitter_seed == 0 {
        kernel.jitter_seed = timers::random_u64();
    }

    let session: Rc<dyn SessionResolver> = Rc::from(config.session);
    let driver = WsWebDriver::new(config.endpoint, session, config.hello_timeout_ms);
    // `now_tick` reads `performance.now()` transiently; the fn value is
    // `Send + Sync`, so the backend can read the clock fresh on every step.
    let (handle, pump) = crate::kernel::runtime::build(kernel, driver, Box::new(timers::now_tick));
    wasm_bindgen_futures::spawn_local(pump);
    handle
}
